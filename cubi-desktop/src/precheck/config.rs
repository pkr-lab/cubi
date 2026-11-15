use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

pub const PLACEHOLDERS: &[&str] = &["repo", "base", "head", "tmp", "config_dir"];

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    #[serde(default = "default_parallel")]
    pub parallel: usize,
    #[serde(default)]
    pub repos: Vec<String>,
}

fn default_parallel() -> usize {
    2
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub name: String,
    pub remote: String,
    #[serde(default = "default_base")]
    pub base_branch: String,
    #[serde(default = "default_protected")]
    pub protected_branches: Vec<String>,
    #[serde(default)]
    pub checks: Vec<Check>,
}

fn default_base() -> String {
    "main".into()
}

fn default_protected() -> Vec<String> {
    vec!["main".into()]
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub source: String,
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default)]
    pub needs: Vec<String>,
    #[serde(default)]
    pub needs_docker: bool,
    #[serde(default)]
    pub os: Vec<String>,
    #[serde(default)]
    pub probes: Vec<Probe>,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub paths_ignore: Vec<String>,
    #[serde(default)]
    pub skip_if_no_commits: bool,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    pub steps: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Probe {
    pub cmd: Vec<String>,
    pub hint: String,
}

fn default_true() -> bool {
    true
}

fn default_timeout() -> u64 {
    3600
}

#[derive(Debug, Clone)]
pub struct Config {
    pub dir: PathBuf,
    pub app: AppConfig,
    pub profiles: Vec<Profile>,
}

impl Config {
    pub fn load(dir: &Path) -> Result<Self> {
        let app_path = dir.join("config.toml");
        let app = if app_path.exists() {
            parse_app(&std::fs::read_to_string(&app_path)?)
                .with_context(|| format!("{} ungültig", app_path.display()))?
        } else {
            parse_app("")?
        };
        let profile_dir = dir.join("profiles");
        let mut files: Vec<PathBuf> = std::fs::read_dir(&profile_dir)
            .with_context(|| format!("Profilordner {} fehlt", profile_dir.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        files.sort();
        let mut profiles = Vec::new();
        for f in files {
            let text = std::fs::read_to_string(&f)?;
            profiles
                .push(parse_profile(&text).with_context(|| format!("{} ungültig", f.display()))?);
        }
        let mut names: Vec<&str> = profiles.iter().map(|p| p.name.as_str()).collect();
        names.sort();
        if let Some(w) = names.windows(2).find(|w| w[0] == w[1]) {
            bail!("Profilname doppelt: {}", w[0]);
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            app,
            profiles,
        })
    }

    pub fn profile_for_remote(&self, slug: &str) -> Option<&Profile> {
        self.profiles
            .iter()
            .find(|p| p.remote.eq_ignore_ascii_case(slug))
    }

    pub fn repo_paths(&self) -> Vec<PathBuf> {
        self.app.repos.iter().map(|r| expand_home(r)).collect()
    }
}

pub fn default_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CUBI_DESKTOP_CONFIG") {
        return PathBuf::from(d);
    }
    config_home().join("cubi-desktop")
}

pub fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
}

pub fn expand_home(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
        None => PathBuf::from(p),
    }
}

pub fn parse_app(text: &str) -> Result<AppConfig> {
    let app: AppConfig = toml::from_str(text)?;
    if app.parallel == 0 {
        bail!("parallel muss mindestens 1 sein");
    }
    Ok(app)
}

pub fn parse_profile(text: &str) -> Result<Profile> {
    let p: Profile = toml::from_str(text)?;
    validate_profile(&p)?;
    Ok(p)
}

fn validate_profile(p: &Profile) -> Result<()> {
    if p.name.trim().is_empty() {
        bail!("name fehlt");
    }
    if p.remote.split('/').count() != 2 {
        bail!("remote muss die Form owner/repo haben: {}", p.remote);
    }
    let mut ids = std::collections::BTreeSet::new();
    for c in &p.checks {
        if !ids.insert(c.id.as_str()) {
            bail!("Check-ID doppelt: {}", c.id);
        }
        if c.id.is_empty()
            || !c
                .id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || "-_:.".contains(ch))
        {
            bail!("Check-ID ungültig: {:?}", c.id);
        }
        if c.steps.is_empty() {
            bail!("Check {}: steps ist leer", c.id);
        }
        for step in &c.steps {
            let Some(prog) = step.first() else {
                bail!("Check {}: leerer Schritt", c.id);
            };
            if prog.contains('{') {
                bail!(
                    "Check {}: Programmname darf keinen Platzhalter enthalten",
                    c.id
                );
            }
            for arg in step {
                check_placeholders(arg).with_context(|| format!("Check {}", c.id))?;
            }
        }
        for probe in &c.probes {
            if probe.cmd.is_empty() {
                bail!("Check {}: Probe ohne Kommando", c.id);
            }
        }
        for os in &c.os {
            if !["linux", "macos", "windows"].contains(&os.as_str()) {
                bail!("Check {}: unbekanntes os {:?}", c.id, os);
            }
        }
        for pat in c.paths.iter().chain(&c.paths_ignore) {
            globset::Glob::new(pat).with_context(|| format!("Check {}: Muster {pat:?}", c.id))?;
        }
        if c.timeout_secs == 0 {
            bail!("Check {}: timeout_secs muss > 0 sein", c.id);
        }
    }
    Ok(())
}

fn check_placeholders(arg: &str) -> Result<()> {
    let mut rest = arg;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            bail!("nicht geschlossener Platzhalter in {arg:?}");
        };
        let key = &after[..end];
        if !PLACEHOLDERS.contains(&key) {
            bail!("unbekannter Platzhalter {{{key}}} in {arg:?}");
        }
        rest = &after[end + 1..];
    }
    Ok(())
}

pub fn substitute(arg: &str, vars: &BTreeMap<&str, String>) -> String {
    let mut out = arg.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROFILE: &str = r#"
name = "demo"
remote = "pkr-lab/demo"

[[checks]]
id = "lint"
name = "make lint"
needs = ["make"]
steps = [["make", "lint"]]

[[checks]]
id = "range"
name = "range"
required = false
paths = ["src/"]
env = { FOO = "1" }
probes = [{ cmd = ["python3", "-c", "import yaml"], hint = "pip install pyyaml" }]
steps = [["git", "log", "{base}..{head}"]]
"#;

    #[test]
    fn parses_profile_with_defaults() {
        let p = parse_profile(PROFILE).unwrap();
        assert_eq!(p.base_branch, "main");
        assert_eq!(p.protected_branches, vec!["main"]);
        assert_eq!(p.checks.len(), 2);
        assert!(p.checks[0].required);
        assert_eq!(p.checks[0].timeout_secs, 3600);
        assert!(!p.checks[1].required);
        assert_eq!(p.checks[1].env["FOO"], "1");
        assert_eq!(p.checks[1].probes[0].hint, "pip install pyyaml");
    }

    #[test]
    fn rejects_unknown_field() {
        let err = parse_profile("name='x'\nremote='a/b'\nfoo=1").unwrap_err();
        assert!(format!("{err:#}").contains("foo"));
    }

    #[test]
    fn rejects_unknown_placeholder() {
        let t = "name='x'\nremote='a/b'\n[[checks]]\nid='a'\nname='a'\nsteps=[['echo','{evil}']]";
        assert!(format!("{:#}", parse_profile(t).unwrap_err()).contains("unbekannter Platzhalter"));
    }

    #[test]
    fn rejects_placeholder_as_program() {
        let t = "name='x'\nremote='a/b'\n[[checks]]\nid='a'\nname='a'\nsteps=[['{repo}/x']]";
        assert!(parse_profile(t).is_err());
    }

    #[test]
    fn rejects_duplicate_ids_and_empty_steps() {
        let dup = "name='x'\nremote='a/b'\n[[checks]]\nid='a'\nname='a'\nsteps=[['true']]\n[[checks]]\nid='a'\nname='b'\nsteps=[['true']]";
        assert!(parse_profile(dup).is_err());
        let empty = "name='x'\nremote='a/b'\n[[checks]]\nid='a'\nname='a'\nsteps=[]";
        assert!(parse_profile(empty).is_err());
    }

    #[test]
    fn rejects_bad_remote_and_os() {
        assert!(parse_profile("name='x'\nremote='nur-repo'").is_err());
        let t =
            "name='x'\nremote='a/b'\n[[checks]]\nid='a'\nname='a'\nos=['beos']\nsteps=[['true']]";
        assert!(parse_profile(t).is_err());
    }

    #[test]
    fn substitutes_placeholders() {
        let mut vars = BTreeMap::new();
        vars.insert("base", "abc".to_string());
        vars.insert("head", "def".to_string());
        assert_eq!(substitute("{base}..{head}", &vars), "abc..def");
        assert_eq!(substitute("plain", &vars), "plain");
    }

    #[test]
    fn app_config_defaults_and_validation() {
        let a = parse_app("").unwrap();
        assert_eq!(a.parallel, 2);
        assert!(parse_app("parallel = 0").is_err());
        let a = parse_app("repos = ['~/x']").unwrap();
        assert_eq!(a.repos, vec!["~/x"]);
    }

    #[test]
    fn bundled_profiles_are_valid() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("config")
            .join("precheck");
        let cfg = Config::load(&dir).unwrap();
        for name in ["capulus-core", "capulus-ansible", "capulus-ios"] {
            assert!(cfg.profiles.iter().any(|p| p.name == name), "{name} fehlt");
        }
        assert!(cfg.profile_for_remote("PKR-LAB/capulus-core").is_some());
    }
}
