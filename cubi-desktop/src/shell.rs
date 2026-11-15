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

const MAC_APP_BINARIES: &[(&str, &str)] = &[(
    "tailscale",
    "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
)];

pub fn resolve(tool: &str) -> Option<std::path::PathBuf> {
    if let Ok(p) = which::which(tool) {
        return Some(p);
    }
    if cfg!(target_os = "macos") {
        return MAC_APP_BINARIES
            .iter()
            .find(|(n, _)| *n == tool)
            .map(|(_, p)| std::path::PathBuf::from(p))
            .filter(|p| p.is_file());
    }
    None
}

pub fn require(tool: &str) -> Result<std::path::PathBuf> {
    resolve(tool).ok_or_else(|| anyhow!("{tool} ist nicht installiert oder nicht im PATH"))
}

pub fn merge_path(current: &str, extra: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for part in current.split(':').chain(extra.split(':')) {
        if !part.is_empty() && !out.contains(&part) {
            out.push(part);
        }
    }
    out.join(":")
}

pub const IMPORTED_VARS: &[&str] = &["KUBECONFIG", "SSH_AUTH_SOCK", "DOCKER_HOST", "CUBI_CONFIG"];
pub const IMPORTED_PREFIXES: &[&str] = &[
    "ARGOCD_",
    "GH_",
    "GITHUB_",
    "HELM_",
    "KUBE",
    "TAILSCALE_",
    "HOMEBREW_",
];

pub fn parse_env0(bytes: &[u8]) -> Vec<(String, String)> {
    bytes
        .split(|b| *b == 0)
        .filter_map(|entry| {
            let s = String::from_utf8_lossy(entry);
            let (k, v) = s.split_once('=')?;
            (!k.is_empty() && !k.contains('\n')).then(|| (k.to_string(), v.to_string()))
        })
        .collect()
}

pub fn shell_env() -> Vec<(String, String)> {
    let Some(shell) = std::env::var("SHELL").ok().filter(|s| !s.is_empty()) else {
        return vec![];
    };
    let mut cmd = std::process::Command::new(shell);
    cmd.args(["-l", "-i", "-c", "env -0"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid ist async-signal-safe; die Shell bekommt so kein Steuerterminal
        // und kann ein aufrufendes Terminal nicht übernehmen.
        unsafe {
            cmd.pre_exec(|| {
                nix::unistd::setsid()
                    .map(|_| ())
                    .map_err(|e| std::io::Error::from_raw_os_error(e as i32))
            });
        }
    }
    let Ok(mut child) = cmd.spawn() else {
        return vec![];
    };
    let Some(mut out) = child.stdout.take() else {
        return vec![];
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = std::io::Read::read_to_end(&mut out, &mut buf);
        let _ = tx.send(buf);
    });
    let result = rx.recv_timeout(Duration::from_secs(4));
    let _ = child.kill();
    let _ = child.wait();
    result.map(|b| parse_env0(&b)).unwrap_or_default()
}

pub fn env_updates(
    shell: &[(String, String)],
    current: impl Fn(&str) -> Option<String>,
    home: &std::path::Path,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut extra = shell
        .iter()
        .find(|(k, _)| k == "PATH")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    for p in [
        "/opt/homebrew/bin".to_string(),
        "/usr/local/bin".into(),
        home.join(".local/bin").display().to_string(),
        home.join(".cargo/bin").display().to_string(),
    ] {
        extra.push(':');
        extra.push_str(&p);
    }
    out.push((
        "PATH".into(),
        merge_path(&current("PATH").unwrap_or_default(), &extra),
    ));
    for (k, v) in shell {
        let wanted = IMPORTED_VARS.contains(&k.as_str())
            || IMPORTED_PREFIXES.iter().any(|p| k.starts_with(p));
        if wanted && current(k).is_none_or(|c| c.is_empty()) {
            out.push((k.clone(), v.clone()));
        }
    }
    out
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
    let mut cmd = Command::new(require(prog)?);
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
    if cfg!(target_os = "macos") {
        return Ok(vec!["open".into(), "-a".into(), "Terminal".into()]);
    }
    TERMINALS
        .iter()
        .find(|t| which::which(t[0]).is_ok())
        .map(|t| t.iter().map(|s| s.to_string()).collect())
        .ok_or_else(|| anyhow!("Kein Terminal gefunden (terminal in desktop.toml setzen)"))
}

const HOLD_SCRIPT: &str =
    "\"$@\"; s=$?; printf '\\n[beendet mit Code %s] Enter schließt das Fenster ' \"$s\"; read -r _";

pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

pub fn in_dir(dir: &std::path::Path, argv: Vec<String>) -> Vec<String> {
    let mut v = vec![
        "sh".into(),
        "-c".into(),
        r#"cd "$1" && shift && exec "$@""#.into(),
        "cubi".into(),
        dir.display().to_string(),
    ];
    v.extend(argv);
    v
}

pub fn uses_script(prefix: &[String]) -> bool {
    prefix.first().is_some_and(|p| p == "open")
}

pub fn command_script(argv: &[String], path_env: &str) -> String {
    let cmd = argv
        .iter()
        .map(|a| sh_quote(a))
        .collect::<Vec<_>>()
        .join(" ");
    let path = sh_quote(path_env);
    format!(
        "#!/bin/sh\nrm -f \"$0\"\nPATH={path}\nexport PATH\n{cmd}\ns=$?\nprintf '\\n[beendet mit Code %s] Enter schließt das Fenster ' \"$s\"\nread -r _\n"
    )
}

pub fn terminal_argv(prefix: &[String], argv: &[String]) -> Vec<String> {
    let mut full = prefix.to_vec();
    full.extend(["sh".into(), "-c".into(), HOLD_SCRIPT.into(), "cubi".into()]);
    full.extend(argv.iter().cloned());
    full
}

pub fn open_terminal(configured: &[String], argv: &[String], env: &[(&str, &str)]) -> Result<()> {
    let first = argv.first().ok_or_else(|| anyhow!("leeres Kommando"))?;
    let mut argv = argv.to_vec();
    argv[0] = require(first)?.display().to_string();
    let prefix = terminal_prefix(configured)?;
    let full = if uses_script(&prefix) {
        use std::io::Write;
        let mut f = tempfile::Builder::new()
            .prefix("cubi-")
            .suffix(".command")
            .tempfile()?;
        f.write_all(command_script(&argv, &std::env::var("PATH").unwrap_or_default()).as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o700))?;
        }
        let (_, path) = f.keep()?;
        let mut v = prefix.clone();
        v.push(path.display().to_string());
        v
    } else {
        terminal_argv(&prefix, &argv)
    };
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

    #[test]
    fn quoting_and_paths() {
        assert_eq!(sh_quote("a'b"), r"'a'\''b'");
        let s = command_script(&["ssh".into()], "/bin:/usr/bin");
        assert!(s.starts_with("#!/bin/sh\nrm -f \"$0\"\nPATH='/bin:/usr/bin'\n"));
        assert!(uses_script(&[
            "open".into(),
            "-a".into(),
            "Terminal".into()
        ]));
        assert!(!uses_script(&["xterm".into()]));
        assert_eq!(merge_path("/a:/b", "/b:/c::/a"), "/a:/b:/c");
    }

    #[test]
    fn env_import_whitelist() {
        let shell = parse_env0(b"PATH=/x/bin\0KUBECONFIG=/h/.kube/k.yaml\0SECRET_FOO=1\0GH_HOST=g\0PWD=/tmp\0KUBE_X=a=b\0");
        assert_eq!(shell.len(), 6);
        let current = |k: &str| match k {
            "PATH" => Some("/usr/bin".to_string()),
            "GH_HOST" => Some("eigen".to_string()),
            _ => None,
        };
        let up = env_updates(&shell, current, std::path::Path::new("/h"));
        assert!(up[0].1.starts_with("/usr/bin:/x/bin:"));
        assert!(up.contains(&("KUBECONFIG".into(), "/h/.kube/k.yaml".into())));
        assert!(up.contains(&("KUBE_X".into(), "a=b".into())));
        assert!(
            !up.iter()
                .any(|(k, _)| k == "SECRET_FOO" || k == "PWD" || k == "GH_HOST")
        );
    }

    #[test]
    fn shell_env_reads_interactive_shell() {
        let env = shell_env();
        if std::env::var("SHELL").is_ok_and(|s| s.ends_with("bash")) {
            assert!(
                env.iter().any(|(k, _)| k == "PATH"),
                "keine Umgebung aus der Shell"
            );
        }
    }

    #[test]
    fn command_script_runs_quoted_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let argv: Vec<String> = vec![
            "sh".into(),
            "-c".into(),
            r#"printf %s "$1" > "$2""#.into(),
            "x".into(),
            "a'b; c $(id)".into(),
            out.display().to_string(),
        ];
        let script =
            command_script(&argv, &std::env::var("PATH").unwrap()).replace("read -r _\n", "");
        let path = dir.path().join("t.command");
        std::fs::write(&path, script).unwrap();
        assert!(
            std::process::Command::new("sh")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "a'b; c $(id)");
        assert!(!path.exists());
    }

    #[test]
    fn in_dir_changes_directory() {
        let dir = tempfile::tempdir().unwrap();
        let argv = in_dir(dir.path(), vec!["pwd".into()]);
        let out = std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .output()
            .unwrap();
        let got = std::path::PathBuf::from(String::from_utf8_lossy(&out.stdout).trim().to_string());
        assert_eq!(
            got.canonicalize().unwrap(),
            dir.path().canonicalize().unwrap()
        );
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
