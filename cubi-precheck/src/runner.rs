use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use globset::{Glob, GlobSetBuilder};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tokio::sync::{Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

use crate::config::{Check, substitute};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Pending,
    Running,
    Ok,
    Failed(String),
    Skipped(String),
    Missing(String),
    Cancelled,
}

impl Status {
    pub fn label(&self) -> &'static str {
        match self {
            Status::Pending => "wartet",
            Status::Running => "läuft",
            Status::Ok => "ok",
            Status::Failed(_) => "fehlgeschlagen",
            Status::Skipped(_) => "übersprungen",
            Status::Missing(_) => "Voraussetzung fehlt",
            Status::Cancelled => "abgebrochen",
        }
    }

    pub fn detail(&self) -> Option<&str> {
        match self {
            Status::Failed(s) | Status::Skipped(s) | Status::Missing(s) => Some(s),
            _ => None,
        }
    }

    pub fn is_final(&self) -> bool {
        !matches!(self, Status::Pending | Status::Running)
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Status {
        idx: usize,
        status: Status,
        elapsed: Option<Duration>,
    },
    Log {
        idx: usize,
        line: String,
    },
    AllDone,
}

#[derive(Debug, Clone)]
pub struct RunContext {
    pub repo: PathBuf,
    pub base: String,
    pub head: String,
    pub config_dir: PathBuf,
    pub changed: Vec<String>,
    pub os: String,
}

impl RunContext {
    fn vars(&self, tmp: &std::path::Path) -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            ("repo", self.repo.display().to_string()),
            ("base", self.base.clone()),
            ("head", self.head.clone()),
            ("tmp", tmp.display().to_string()),
            ("config_dir", self.config_dir.display().to_string()),
        ])
    }
}

pub async fn prepare(
    repo: &std::path::Path,
    base_branch: &str,
    config_dir: &std::path::Path,
) -> anyhow::Result<(Option<String>, RunContext)> {
    let warning = crate::git::fetch_base(repo, base_branch)
        .await
        .err()
        .map(|e| {
            format!(
                "Warnung: git fetch fehlgeschlagen, nutze lokalen Stand von origin/{base_branch}: {e:#}"
            )
        });
    let base = crate::git::merge_base(repo, base_branch).await?;
    let head = crate::git::git(repo, &["rev-parse", "HEAD"])
        .await?
        .trim()
        .to_string();
    let changed = crate::git::changed_files(repo, &base).await?;
    Ok((
        warning,
        RunContext {
            repo: repo.to_path_buf(),
            base,
            head,
            config_dir: config_dir.to_path_buf(),
            changed,
            os: current_os().into(),
        },
    ))
}

pub fn path_filter(check: &Check, changed: &[String]) -> Option<String> {
    if !check.paths.is_empty() {
        let set = build_set(&check.paths);
        if !changed.iter().any(|f| set.is_match(f)) {
            return Some(format!(
                "keine Änderungen unter {} (wie in der CI)",
                check.paths.join(", ")
            ));
        }
    }
    if !check.paths_ignore.is_empty() && !changed.is_empty() {
        let set = build_set(&check.paths_ignore);
        if changed.iter().all(|f| set.is_match(f)) {
            return Some(format!(
                "nur Änderungen in {} (wie in der CI)",
                check.paths_ignore.join(", ")
            ));
        }
    }
    None
}

fn build_set(patterns: &[String]) -> globset::GlobSet {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        if let Ok(g) = Glob::new(p) {
            b.add(g);
        }
    }
    b.build().unwrap_or_else(|_| globset::GlobSet::empty())
}

pub async fn missing_prerequisite(check: &Check, os: &str) -> Option<String> {
    if !check.os.is_empty() && !check.os.iter().any(|o| o == os) {
        return Some(format!(
            "benötigt Betriebssystem {} (hier: {os})",
            check.os.join("/")
        ));
    }
    let missing: Vec<&str> = check
        .needs
        .iter()
        .filter(|t| which::which(t).is_err())
        .map(String::as_str)
        .collect();
    if !missing.is_empty() {
        return Some(format!("Tool nicht installiert: {}", missing.join(", ")));
    }
    if check.needs_docker && !command_succeeds(&["docker", "info"], Duration::from_secs(15)).await {
        return Some("Docker läuft nicht oder ist nicht erreichbar (docker info)".into());
    }
    for probe in &check.probes {
        let args: Vec<&str> = probe.cmd.iter().map(String::as_str).collect();
        if !command_succeeds(&args, Duration::from_secs(30)).await {
            return Some(probe.hint.clone());
        }
    }
    None
}

async fn command_succeeds(args: &[&str], timeout: Duration) -> bool {
    let Some((prog, rest)) = args.split_first() else {
        return false;
    };
    let fut = Command::new(prog)
        .args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status();
    matches!(tokio::time::timeout(timeout, fut).await, Ok(Ok(s)) if s.success())
}

pub async fn run_all(
    checks: Vec<(usize, Check)>,
    ctx: RunContext,
    parallel: usize,
    tx: mpsc::UnboundedSender<Event>,
    cancel: CancellationToken,
) {
    let sem = Arc::new(Semaphore::new(parallel.max(1)));
    let ctx = Arc::new(ctx);
    let mut handles = Vec::new();
    for (idx, check) in checks {
        let _ = tx.send(Event::Status {
            idx,
            status: Status::Pending,
            elapsed: None,
        });
        let sem = sem.clone();
        let ctx = ctx.clone();
        let tx = tx.clone();
        let cancel = cancel.clone();
        handles.push(tokio::spawn(async move {
            let permit = tokio::select! {
                p = sem.acquire_owned() => p.ok(),
                _ = cancel.cancelled() => None,
            };
            if permit.is_none() {
                let _ = tx.send(Event::Status {
                    idx,
                    status: Status::Cancelled,
                    elapsed: None,
                });
                return;
            }
            let start = Instant::now();
            let status = run_one(idx, &check, &ctx, &tx, &cancel).await;
            let _ = tx.send(Event::Status {
                idx,
                status,
                elapsed: Some(start.elapsed()),
            });
        }));
    }
    for h in handles {
        let _ = h.await;
    }
    let _ = tx.send(Event::AllDone);
}

async fn run_one(
    idx: usize,
    check: &Check,
    ctx: &RunContext,
    tx: &mpsc::UnboundedSender<Event>,
    cancel: &CancellationToken,
) -> Status {
    let log = |line: String| {
        let _ = tx.send(Event::Log { idx, line });
    };
    if check.skip_if_no_commits && ctx.base == ctx.head {
        let reason = "keine Commits gegenüber dem Basis-Branch".to_string();
        log(format!("übersprungen: {reason}"));
        return Status::Skipped(reason);
    }
    if let Some(reason) = path_filter(check, &ctx.changed) {
        log(format!("übersprungen: {reason}"));
        return Status::Skipped(reason);
    }
    if let Some(reason) = missing_prerequisite(check, &ctx.os).await {
        log(format!("Voraussetzung fehlt: {reason}"));
        return Status::Missing(reason);
    }
    if cancel.is_cancelled() {
        return Status::Cancelled;
    }
    let _ = tx.send(Event::Status {
        idx,
        status: Status::Running,
        elapsed: None,
    });
    let tmp = match tempfile::Builder::new().prefix("cubi-precheck-").tempdir() {
        Ok(t) => t,
        Err(e) => return Status::Failed(format!("Temp-Verzeichnis: {e}")),
    };
    let vars = ctx.vars(tmp.path());
    let deadline = Instant::now() + Duration::from_secs(check.timeout_secs);
    for (n, step) in check.steps.iter().enumerate() {
        let argv: Vec<String> = step.iter().map(|a| substitute(a, &vars)).collect();
        log(format!("$ {}", display_argv(&argv)));
        let remaining = deadline.saturating_duration_since(Instant::now());
        match run_step(idx, &argv, check, ctx, tx, cancel, remaining).await {
            StepResult::Ok => {}
            StepResult::Exit(code) => {
                return Status::Failed(format!("Schritt {} endete mit Code {code}", n + 1));
            }
            StepResult::NotFound => {
                return Status::Missing(format!("Programm nicht gefunden: {}", argv[0]));
            }
            StepResult::Error(e) => return Status::Failed(e),
            StepResult::Timeout => {
                return Status::Failed(format!("Zeitlimit {}s überschritten", check.timeout_secs));
            }
            StepResult::Cancelled => return Status::Cancelled,
        }
    }
    Status::Ok
}

fn display_argv(argv: &[String]) -> String {
    argv.iter()
        .map(|a| {
            if a.is_empty() || a.contains(char::is_whitespace) {
                format!("'{a}'")
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

enum StepResult {
    Ok,
    Exit(i32),
    NotFound,
    Error(String),
    Timeout,
    Cancelled,
}

async fn run_step(
    idx: usize,
    argv: &[String],
    check: &Check,
    ctx: &RunContext,
    tx: &mpsc::UnboundedSender<Event>,
    cancel: &CancellationToken,
    timeout: Duration,
) -> StepResult {
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .current_dir(&ctx.repo)
        .envs(&check.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return StepResult::NotFound,
        Err(e) => return StepResult::Error(format!("Start fehlgeschlagen: {e}")),
    };
    let out = child.stdout.take().map(|s| pump(idx, s, tx.clone()));
    let err = child.stderr.take().map(|s| pump(idx, s, tx.clone()));

    let result = tokio::select! {
        r = child.wait() => match r {
            Ok(s) if s.success() => StepResult::Ok,
            Ok(s) => StepResult::Exit(s.code().unwrap_or(-1)),
            Err(e) => StepResult::Error(e.to_string()),
        },
        _ = cancel.cancelled() => {
            terminate(&mut child).await;
            StepResult::Cancelled
        }
        _ = tokio::time::sleep(timeout) => {
            terminate(&mut child).await;
            StepResult::Timeout
        }
    };
    for h in [out, err].into_iter().flatten() {
        let _ = tokio::time::timeout(Duration::from_secs(2), h).await;
    }
    result
}

fn pump<R: AsyncRead + Unpin + Send + 'static>(
    idx: usize,
    reader: R,
    tx: mpsc::UnboundedSender<Event>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let _ = tx.send(Event::Log {
                        idx,
                        line: strip_ansi(&line),
                    });
                }
                Ok(None) => break,
                Err(_) => continue,
            }
        }
    })
}

async fn terminate(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        use nix::sys::signal::{Signal, killpg};
        use nix::unistd::Pid;
        let pgid = Pid::from_raw(pid as i32);
        let _ = killpg(pgid, Signal::SIGTERM);
        if tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .is_ok()
        {
            return;
        }
        let _ = killpg(pgid, Signal::SIGKILL);
    }
    let _ = child.kill().await;
}

pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for n in chars.by_ref() {
                    if ('@'..='~').contains(&n) {
                        break;
                    }
                }
            }
        } else if c != '\r' {
            out.push(c);
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    Green,
    Blocked(Vec<String>),
}

pub fn gate(checks: &[Check], statuses: &[Status]) -> Gate {
    let blockers: Vec<String> = checks
        .iter()
        .zip(statuses)
        .filter(|(c, _)| c.required)
        .filter(|(_, s)| !matches!(s, Status::Ok | Status::Skipped(_)))
        .map(|(c, s)| format!("{} ({})", c.name, s.label()))
        .collect();
    if blockers.is_empty() {
        Gate::Green
    } else {
        Gate::Blocked(blockers)
    }
}

pub fn current_os() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        _ => "linux",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse_profile;

    fn check(extra: &str, steps: &str) -> Check {
        let t =
            format!("name='t'\nremote='a/b'\n[[checks]]\nid='c'\nname='c'\n{extra}\nsteps={steps}");
        parse_profile(&t).unwrap().checks.remove(0)
    }

    fn ctx() -> RunContext {
        RunContext {
            repo: std::env::temp_dir(),
            base: "BASE".into(),
            head: "HEAD".into(),
            config_dir: PathBuf::from("/cfg"),
            changed: vec!["src/a.rs".into()],
            os: current_os().into(),
        }
    }

    async fn run(
        checks: Vec<Check>,
        ctx: RunContext,
        cancel: CancellationToken,
    ) -> (Vec<Status>, Vec<Vec<String>>) {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let n = checks.len();
        let list = checks.into_iter().enumerate().collect();
        run_all(list, ctx, 2, tx, cancel).await;
        let mut statuses = vec![Status::Pending; n];
        let mut logs = vec![Vec::new(); n];
        while let Ok(ev) = rx.try_recv() {
            match ev {
                Event::Status { idx, status, .. } => statuses[idx] = status,
                Event::Log { idx, line } => logs[idx].push(line),
                Event::AllDone => {}
            }
        }
        (statuses, logs)
    }

    #[tokio::test]
    async fn ok_failed_and_logs() {
        let checks = vec![
            check("", "[['echo','hallo {base}..{head}']]"),
            check("", "[['true'],['false']]"),
        ];
        let (st, logs) = run(checks, ctx(), CancellationToken::new()).await;
        assert_eq!(st[0], Status::Ok);
        assert!(logs[0].iter().any(|l| l == "hallo BASE..HEAD"));
        assert_eq!(st[1], Status::Failed("Schritt 2 endete mit Code 1".into()));
    }

    #[tokio::test]
    async fn missing_tool_is_not_green() {
        let checks = vec![
            check("needs=['gibt-es-nicht-xyz']", "[['true']]"),
            check("", "[['gibt-es-nicht-xyz']]"),
            check("probes=[{cmd=['false'], hint='Modul fehlt'}]", "[['true']]"),
            check("os=['windows']", "[['true']]"),
        ];
        let (st, _) = run(checks.clone(), ctx(), CancellationToken::new()).await;
        assert!(matches!(&st[0], Status::Missing(m) if m.contains("gibt-es-nicht-xyz")));
        assert!(matches!(&st[1], Status::Missing(m) if m.contains("nicht gefunden")));
        assert_eq!(st[2], Status::Missing("Modul fehlt".into()));
        assert!(matches!(&st[3], Status::Missing(m) if m.contains("windows")));
        assert!(matches!(gate(&checks, &st), Gate::Blocked(b) if b.len() == 4));
    }

    #[tokio::test]
    async fn path_filter_skips() {
        let checks = vec![
            check("paths=['docs/**']", "[['false']]"),
            check("paths=['src/**']", "[['true']]"),
            check("paths_ignore=['**/*.rs']", "[['false']]"),
        ];
        let (st, _) = run(checks.clone(), ctx(), CancellationToken::new()).await;
        assert!(matches!(st[0], Status::Skipped(_)));
        assert_eq!(st[1], Status::Ok);
        assert!(matches!(st[2], Status::Skipped(_)));
        assert_eq!(gate(&checks, &st), Gate::Green);
    }

    #[tokio::test]
    async fn skips_without_commits() {
        let checks = vec![check("skip_if_no_commits=true", "[['false']]")];
        let mut c = ctx();
        c.head = c.base.clone();
        let (st, _) = run(checks.clone(), c, CancellationToken::new()).await;
        assert!(matches!(st[0], Status::Skipped(_)));
        let (st, _) = run(checks, ctx(), CancellationToken::new()).await;
        assert!(matches!(st[0], Status::Failed(_)));
    }

    #[tokio::test]
    async fn cancel_kills_running_step() {
        let checks = vec![check("", "[['sleep','30']]")];
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            c2.cancel();
        });
        let start = Instant::now();
        let (st, _) = run(checks, ctx(), cancel).await;
        assert_eq!(st[0], Status::Cancelled);
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[tokio::test]
    async fn timeout_fails() {
        let checks = vec![check("timeout_secs=1", "[['sleep','30']]")];
        let (st, _) = run(checks, ctx(), CancellationToken::new()).await;
        assert!(matches!(&st[0], Status::Failed(m) if m.contains("Zeitlimit")));
    }

    #[tokio::test]
    async fn env_and_tmp_placeholder() {
        let checks = vec![check(
            "env={GREETING='moin'}",
            "[['printenv','GREETING'],['test','-d','{tmp}']]",
        )];
        let (st, logs) = run(checks, ctx(), CancellationToken::new()).await;
        assert_eq!(st[0], Status::Ok);
        assert!(logs[0].contains(&"moin".to_string()));
    }

    #[test]
    fn strips_ansi() {
        assert_eq!(strip_ansi("\u{1b}[31mrot\u{1b}[0m ok\r"), "rot ok");
    }

    #[test]
    fn gate_ignores_optional_and_blocks_pending() {
        let checks = vec![
            check("", "[['true']]"),
            check("required=false", "[['true']]"),
        ];
        assert_eq!(
            gate(&checks, &[Status::Ok, Status::Failed("x".into())]),
            Gate::Green
        );
        assert!(matches!(
            gate(&checks, &[Status::Pending, Status::Ok]),
            Gate::Blocked(_)
        ));
        assert!(matches!(
            gate(&checks, &[Status::Cancelled, Status::Ok]),
            Gate::Blocked(_)
        ));
    }
}
