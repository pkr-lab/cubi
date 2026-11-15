use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use serde_yaml_ng::Value;

use crate::cubi_config::{CubiConfig, as_str, cubi_root};
use crate::precheck::config::expand_home;

pub const REGISTER: &str = "ops/token-register.yaml";
pub const DOCS_PREFIX: &str = "pkr-lab-docs:";
pub const SOON: i64 = 60;
pub const URGENT: i64 = 7;
pub const RULES: &[&str] = &[
    "Neues Token anlegen, das alte bleibt gültig (Überlappung).",
    "Alle Verbraucher auf das neue Token umstellen.",
    "Prüfen, dass alles funktioniert.",
    "Erst dann das alte Token widerrufen.",
];

#[derive(Debug, Clone)]
pub struct Token {
    pub id: String,
    pub service: String,
    pub kind: String,
    pub stored_in: Vec<String>,
    pub mode: String,
    pub runbook: String,
    pub command: Vec<String>,
    pub hint: String,
    pub overlap: bool,
    pub blast_radius: String,
    pub issued: Option<NaiveDate>,
    pub expires: Option<NaiveDate>,
    pub max_age_days: Option<i64>,
    pub live_check: Option<String>,
}

impl Token {
    pub fn due(&self) -> Option<NaiveDate> {
        self.expires
            .or_else(|| Some(self.issued? + chrono::Duration::days(self.max_age_days?)))
    }
    pub fn days_left(&self, today: NaiveDate) -> Option<i64> {
        self.due().map(|d| (d - today).num_days())
    }
    pub fn urgency(&self, today: NaiveDate) -> u8 {
        match self.days_left(today) {
            None => 1,
            Some(l) if l <= URGENT => 0,
            Some(l) if l <= SOON => 2,
            _ => 3,
        }
    }
}

pub fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

pub fn repo_root(cfg: Option<&CubiConfig>) -> Result<PathBuf> {
    let root = cfg
        .and_then(|c| c.str_in("tokens", "repo"))
        .map(|r| expand_home(&r))
        .unwrap_or_else(|| {
            cubi_root()
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default()
                .join("capulus-core")
        });
    if !root.join(REGISTER).is_file() {
        bail!(
            "Token-Register nicht gefunden: {} (tokens.repo in der Config setzen)",
            root.join(REGISTER).display()
        );
    }
    Ok(root)
}

pub fn docs_root(cfg: Option<&CubiConfig>) -> Result<PathBuf> {
    let root = cfg
        .and_then(|c| c.str_in("tokens", "docs_repo"))
        .map(|r| expand_home(&r))
        .unwrap_or_else(|| {
            cubi_root()
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default()
                .join("pkr-lab-docs")
        });
    if !root.is_dir() {
        bail!(
            "Docs-Repo nicht gefunden: {} (tokens.docs_repo in der Config setzen)",
            root.display()
        );
    }
    Ok(root)
}

pub fn resolve_runbook(cfg: Option<&CubiConfig>, value: &str) -> Result<PathBuf> {
    match value.strip_prefix(DOCS_PREFIX) {
        Some(rest) => Ok(docs_root(cfg)?.join(rest)),
        None => Ok(repo_root(cfg)?.join(value)),
    }
}

fn parse_date(v: Option<&Value>) -> Result<Option<NaiveDate>> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(other) => {
            let s = as_str(other).unwrap_or_default();
            NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                .map(Some)
                .with_context(|| format!("Ungültiges Datum im Register: {s:?}"))
        }
    }
}

pub fn split_command(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut has = false;
    let mut chars = cmd.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                has = true;
            }
            (None, '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                    has = true;
                }
            }
            (None, c) if c.is_whitespace() => {
                if has || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if has || !cur.is_empty() {
        out.push(cur);
    }
    out
}

pub fn parse_register(text: &str) -> Result<Vec<Token>> {
    let entries: Value = serde_yaml_ng::from_str(text).context("Ungültiges Register")?;
    let list = match entries {
        Value::Null => vec![],
        Value::Sequence(s) => s,
        _ => bail!("Ungültiges Register: Liste erwartet"),
    };
    let mut tokens = Vec::new();
    for e in list {
        let rot = e.get("rotation").cloned().unwrap_or(Value::Null);
        let get = |v: &Value, k: &str| v.get(k).and_then(as_str);
        tokens.push(Token {
            id: get(&e, "id").context("Eintrag ohne id im Register")?,
            service: get(&e, "service").unwrap_or_else(|| "-".into()),
            kind: get(&e, "kind").unwrap_or_else(|| "-".into()),
            stored_in: e
                .get("stored_in")
                .and_then(Value::as_sequence)
                .map(|s| s.iter().filter_map(as_str).collect())
                .unwrap_or_default(),
            mode: get(&rot, "mode").unwrap_or_else(|| "manual".into()),
            runbook: get(&rot, "runbook").unwrap_or_default(),
            command: split_command(&get(&rot, "command").unwrap_or_default()),
            hint: get(&rot, "hint").unwrap_or_default(),
            overlap: rot.get("overlap").and_then(Value::as_bool).unwrap_or(true),
            blast_radius: get(&e, "blast_radius").unwrap_or_default(),
            issued: parse_date(e.get("issued"))?,
            expires: parse_date(e.get("expires"))?,
            max_age_days: e.get("max_age_days").and_then(Value::as_i64),
            live_check: get(&e, "live_check"),
        });
    }
    Ok(tokens)
}

pub fn load(cfg: Option<&CubiConfig>) -> Result<Vec<Token>> {
    let path = repo_root(cfg)?.join(REGISTER);
    let text = std::fs::read_to_string(&path)?;
    parse_register(&text).with_context(|| path.display().to_string())
}

pub fn validate_date(text: &str) -> Result<NaiveDate> {
    let t = text.trim();
    let ok = t.len() == 10
        && t.bytes().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        });
    if !ok {
        bail!("Datum muss das Format JJJJ-MM-TT haben: {t:?}");
    }
    NaiveDate::parse_from_str(t, "%Y-%m-%d").with_context(|| format!("Ungültiges Datum: {t:?}"))
}

pub fn set_dates_text(
    text: &str,
    token_id: &str,
    issued: NaiveDate,
    expires: Option<NaiveDate>,
) -> Result<String> {
    let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_string).collect();
    let header = format!("- id: {token_id}");
    let start = lines
        .iter()
        .position(|l| l.trim_end() == header)
        .with_context(|| format!("{token_id} steht nicht im Register"))?;
    let end = (start + 1..lines.len())
        .find(|&i| lines[i].starts_with("- id:"))
        .unwrap_or(lines.len());
    let values = [
        ("issued", issued.to_string()),
        (
            "expires",
            expires.map_or_else(|| "null".into(), |d| d.to_string()),
        ),
    ];
    for (key, value) in values {
        let prefix = format!("  {key}:");
        let pos = (start..end)
            .find(|&i| {
                lines[i].starts_with(&prefix)
                    && lines[i][prefix.len()..].starts_with(|c: char| c.is_whitespace())
            })
            .with_context(|| format!("{token_id}: Feld {key} fehlt im Register"))?;
        lines[pos] = format!("{prefix} {value}\n");
    }
    Ok(lines.concat())
}

pub fn set_dates(
    cfg: Option<&CubiConfig>,
    token_id: &str,
    issued: NaiveDate,
    expires: Option<NaiveDate>,
) -> Result<PathBuf> {
    let path = repo_root(cfg)?.join(REGISTER);
    let text = std::fs::read_to_string(&path)?;
    let new = set_dates_text(&text, token_id, issued, expires)?;
    let dir = path.parent().context("Registerpfad ohne Ordner")?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".token-register-")
        .tempfile_in(dir)?;
    tmp.write_all(new.as_bytes())?;
    let perms = std::fs::metadata(&path)?.permissions();
    std::fs::set_permissions(tmp.path(), perms)?;
    tmp.persist(&path)?;
    Ok(path)
}

pub fn slug(heading: &str) -> String {
    let t: String = heading
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || c.is_whitespace() || *c == '-')
        .collect();
    t.chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .collect()
}

pub fn runbook_text(text: &str, anchor: &str) -> String {
    if anchor.is_empty() {
        return text.to_string();
    }
    let lines: Vec<&str> = text.lines().collect();
    match lines
        .iter()
        .position(|l| l.starts_with('#') && slug(l.trim_start_matches('#')) == anchor)
    {
        Some(i) => lines[i..].join("\n"),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REG: &str = "- id: gh-pat\n  service: GitHub\n  kind: PAT\n  stored_in:\n    - sealed-secret\n  issued: 2026-01-01\n  expires: null\n  max_age_days: 90\n  rotation:\n    mode: scripted\n    command: scripts/seal-github-token.sh --ns \"a b\"\n    overlap: false\n  blast_radius: CI\n- id: other\n  issued: null\n  expires: 2026-02-01\n";

    #[test]
    fn parses_register() {
        let t = parse_register(REG).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(
            t[0].command,
            vec!["scripts/seal-github-token.sh", "--ns", "a b"]
        );
        assert!(!t[0].overlap);
        assert_eq!(t[0].due(), NaiveDate::from_ymd_opt(2026, 4, 1));
        let today = NaiveDate::from_ymd_opt(2026, 3, 30).unwrap();
        assert_eq!(t[0].days_left(today), Some(2));
        assert_eq!(t[0].urgency(today), 0);
        assert_eq!(t[1].mode, "manual");
        assert_eq!(t[1].urgency(today), 0);
    }

    #[test]
    fn sets_dates_only_in_block() {
        let d = NaiveDate::from_ymd_opt(2026, 5, 5).unwrap();
        let out = set_dates_text(REG, "gh-pat", d, None).unwrap();
        assert!(out.contains("  issued: 2026-05-05\n  expires: null\n"));
        assert!(out.contains("- id: other\n  issued: null\n  expires: 2026-02-01"));
        assert!(set_dates_text(REG, "fehlt", d, None).is_err());
    }

    #[test]
    fn dates_and_slugs() {
        assert!(validate_date("2026-1-1").is_err());
        assert!(validate_date("2026-02-30").is_err());
        assert!(validate_date("2026-02-28").is_ok());
        assert_eq!(slug(" Rotation (GitHub-PAT)"), "rotation-github-pat");
        assert_eq!(
            runbook_text("x\n## Rotation\ny", "rotation"),
            "## Rotation\ny"
        );
    }
}
