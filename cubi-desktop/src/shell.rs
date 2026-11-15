use std::process::Stdio;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.code == 0
    }
}

pub fn require(tool: &str) -> Result<()> {
    if which::which(tool).is_err() {
        bail!("{tool} ist nicht installiert oder nicht im PATH");
    }
    Ok(())
}

pub async fn capture<S: AsRef<str>>(argv: &[S], timeout_secs: u64) -> Result<Output> {
    capture_in(argv, timeout_secs, None, &[]).await
}

pub async fn capture_in<S: AsRef<str>>(
    argv: &[S],
    timeout_secs: u64,
    cwd: Option<&std::path::Path>,
    env: &[(&str, &str)],
) -> Result<Output> {
    let args: Vec<&str> = argv.iter().map(AsRef::as_ref).collect();
    let (prog, rest) = args
        .split_first()
        .ok_or_else(|| anyhow!("leeres Kommando"))?;
    require(prog)?;
    let mut cmd = Command::new(prog);
    cmd.args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = tokio::time::timeout(Duration::from_secs(timeout_secs), cmd.output())
        .await
        .map_err(|_| anyhow!("Zeitüberschreitung bei: {}", args.join(" ")))??;
    Ok(Output {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

pub fn ensure(out: Output, fallback: &str) -> Result<Output> {
    if !out.ok() {
        let line = first_line(&out.stderr);
        bail!(
            "{}",
            if line.is_empty() {
                fallback.to_string()
            } else {
                line
            }
        );
    }
    Ok(out)
}

pub async fn capture_ok<S: AsRef<str>>(
    argv: &[S],
    timeout_secs: u64,
    fallback: &str,
) -> Result<Output> {
    ensure(capture(argv, timeout_secs).await?, fallback)
}

pub async fn capture_json<S: AsRef<str>>(
    argv: &[S],
    timeout_secs: u64,
) -> Result<serde_json::Value> {
    let joined = argv.iter().map(AsRef::as_ref).collect::<Vec<_>>().join(" ");
    let out = capture_ok(
        argv,
        timeout_secs,
        &format!("Befehl fehlgeschlagen: {joined}"),
    )
    .await?;
    serde_json::from_str(&out.stdout).map_err(|_| anyhow!("Ungültige Ausgabe von: {joined}"))
}

pub fn is_log_noise(line: &str) -> bool {
    let b = line.as_bytes();
    b.len() >= 14
        && b"EWIF".contains(&b[0])
        && b[1..5].iter().all(u8::is_ascii_digit)
        && b[5] == b' '
        && b[6..8].iter().all(u8::is_ascii_digit)
        && b[8] == b':'
        && b[9..11].iter().all(u8::is_ascii_digit)
        && b[11] == b':'
        && b[12..14].iter().all(u8::is_ascii_digit)
}

pub fn clean_line(line: &str) -> String {
    let t = line.trim();
    if t.starts_with('{')
        && t.ends_with('}')
        && let Ok(serde_json::Value::Object(m)) = serde_json::from_str::<serde_json::Value>(t)
        && let Some(msg) = m.get("msg")
    {
        return match msg {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
    }
    line.to_string()
}

pub fn first_line(text: &str) -> String {
    let lines: Vec<String> = text
        .lines()
        .map(clean_line)
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    lines
        .iter()
        .find(|l| !is_log_noise(l))
        .or(lines.first())
        .cloned()
        .unwrap_or_default()
}

pub fn merge_output(out: &Output) -> String {
    let mut parts = vec![out.stdout.trim_end().to_string()];
    let errors: Vec<String> = out
        .stderr
        .lines()
        .map(clean_line)
        .filter(|l| !is_log_noise(l.trim()))
        .collect();
    let errors = errors.join("\n").trim_end().to_string();
    if !errors.is_empty() {
        parts.push(errors);
    }
    parts.retain(|p| !p.is_empty());
    parts.join("\n")
}

const TERMINALS: &[&[&str]] = &[
    &["x-terminal-emulator", "-e"],
    &["gnome-terminal", "--"],
    &["konsole", "-e"],
    &["xfce4-terminal", "-x"],
    &["kitty"],
    &["alacritty", "-e"],
    &["xterm", "-e"],
];

pub fn terminal_prefix(configured: &[String]) -> Result<Vec<String>> {
    if let Some(first) = configured.first() {
        require(first)?;
        return Ok(configured.to_vec());
    }
    TERMINALS
        .iter()
        .find(|t| which::which(t[0]).is_ok())
        .map(|t| t.iter().map(|s| s.to_string()).collect())
        .ok_or_else(|| anyhow!("Kein Terminal gefunden (terminal in desktop.toml setzen)"))
}

const HOLD_SCRIPT: &str =
    "\"$@\"; s=$?; printf '\\n[beendet mit Code %s] Enter schließt das Fenster ' \"$s\"; read -r _";

pub fn terminal_argv(prefix: &[String], argv: &[String]) -> Vec<String> {
    let mut full = prefix.to_vec();
    full.extend(["sh".into(), "-c".into(), HOLD_SCRIPT.into(), "cubi".into()]);
    full.extend(argv.iter().cloned());
    full
}

pub fn open_terminal(configured: &[String], argv: &[String], env: &[(&str, &str)]) -> Result<()> {
    let first = argv.first().ok_or_else(|| anyhow!("leeres Kommando"))?;
    require(first)?;
    let full = terminal_argv(&terminal_prefix(configured)?, argv);
    let mut cmd = std::process::Command::new(&full[0]);
    cmd.args(&full[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (k, v) in env {
        cmd.env(k, v);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn()
        .map_err(|e| anyhow!("Terminal {} konnte nicht starten: {e}", full[0]))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_noise_and_first_line() {
        assert!(is_log_noise("E0101 12:00:00.123 foo"));
        assert!(!is_log_noise("Error: kaputt"));
        let text = "\nE0101 12:00:00.1 klog\n{\"level\":\"error\",\"msg\":\"token expired\"}\n";
        assert_eq!(first_line(text), "token expired");
        assert_eq!(
            first_line("E0101 12:00:00.1 nur rauschen"),
            "E0101 12:00:00.1 nur rauschen"
        );
    }

    #[test]
    fn merge_drops_noise() {
        let out = Output {
            code: 1,
            stdout: "a\n".into(),
            stderr: "E0101 12:00:00.1 x\nb\n".into(),
        };
        assert_eq!(merge_output(&out), "a\nb");
    }

    #[test]
    fn terminal_argv_passes_arguments_unquoted() {
        let argv = terminal_argv(
            &["xterm".into(), "-e".into()],
            &["ssh".into(), "host; rm -rf /".into()],
        );
        assert_eq!(&argv[..4], &["xterm", "-e", "sh", "-c"]);
        assert_eq!(argv[5], "cubi");
        assert_eq!(argv.last().unwrap(), "host; rm -rf /");
    }

    #[tokio::test]
    async fn capture_reports_missing_tool_and_exit_code() {
        let err = capture(&["gibt-es-nicht-xyz"], 5).await.unwrap_err();
        assert!(err.to_string().contains("nicht installiert"));
        let out = capture(&["sh", "-c", "echo hi; echo err >&2; exit 3"], 5)
            .await
            .unwrap();
        assert_eq!(
            (out.code, out.stdout.trim(), out.stderr.trim()),
            (3, "hi", "err")
        );
        assert!(ensure(out, "x").unwrap_err().to_string().contains("err"));
    }
}
