use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_yaml_ng::{Mapping, Value};

#[derive(Debug, Clone)]
pub struct CubiConfig {
    pub path: PathBuf,
    root: Mapping,
}

pub fn cubi_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

pub fn candidates() -> Vec<PathBuf> {
    let mut c = Vec::new();
    if let Some(p) = std::env::var_os("CUBI_CONFIG") {
        c.push(crate::precheck::config::expand_home(&p.to_string_lossy()));
    }
    if let Some(h) = dirs::home_dir() {
        c.push(h.join(".config/cubi/targets.yaml"));
    }
    c.push(cubi_root().join("config/targets.yaml"));
    c
}

impl CubiConfig {
    pub fn load() -> Result<Self> {
        let path = candidates()
            .into_iter()
            .find(|p| p.is_file())
            .context("Keine targets.yaml gefunden")?;
        let text = std::fs::read_to_string(&path)?;
        Self::parse(path, &text)
    }

    pub fn parse(path: PathBuf, text: &str) -> Result<Self> {
        let value: Value = serde_yaml_ng::from_str(text)
            .with_context(|| format!("Ungültige Config {}", path.display()))?;
        let root = match value {
            Value::Null => Mapping::new(),
            Value::Mapping(m) => m,
            _ => bail!(
                "Ungültige Config {}: oberste Ebene muss ein Mapping sein",
                path.display()
            ),
        };
        Ok(Self { path, root })
    }

    pub fn section(&self, name: &str) -> Mapping {
        match self.root.get(name) {
            Some(Value::Mapping(m)) => m.clone(),
            _ => Mapping::new(),
        }
    }

    pub fn str_in(&self, section: &str, key: &str) -> Option<String> {
        match self.section(section).get(key) {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Number(n)) => Some(n.to_string()),
            _ => None,
        }
    }
}

pub fn as_str(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections() {
        let c = CubiConfig::parse(
            "x".into(),
            "github:\n  default_repo: a/b\nconnect:\n  vm:\n    - name: x\n      ssh: y\n",
        )
        .unwrap();
        assert_eq!(c.str_in("github", "default_repo").as_deref(), Some("a/b"));
        assert!(c.section("fehlt").is_empty());
        assert!(CubiConfig::parse("x".into(), "- a").is_err());
        assert!(
            CubiConfig::parse("x".into(), "")
                .unwrap()
                .section("a")
                .is_empty()
        );
    }

    #[test]
    fn bundled_targets_yaml_parses() {
        let p = cubi_root().join("config/targets.yaml");
        let c = CubiConfig::parse(p.clone(), &std::fs::read_to_string(p).unwrap()).unwrap();
        assert!(!c.section("connect").is_empty());
    }
}
