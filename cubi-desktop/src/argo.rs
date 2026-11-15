use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};

use crate::cubi_config::{CubiConfig, as_str};
use crate::shell;
use crate::ui::Tone;

pub const APP_RESOURCE: &str = "applications.argoproj.io";
pub const APPSET_RESOURCE: &str = "applicationsets.argoproj.io";
pub const STATE_CONFIGMAP: &str = "cubi-state";
pub const SYNC_POINTER: &str = "/spec/syncPolicy";
pub const TRACKING_ANNOTATION: &str = "argocd.argoproj.io/tracking-id";
pub const DEFAULT_BRANCH: &str = "main";
pub const SETTLE_SECONDS: u64 = 8;
pub const AUTH_HINTS: &[&str] = &[
    "unauthenticated",
    "token is expired",
    "expired",
    "please log in",
    "login",
];

pub type Policy = Map<String, Value>;

#[derive(Debug, Clone, PartialEq)]
pub struct ScaleMemory {
    pub replicas: BTreeMap<String, i64>,
    pub auto_sync: Option<Policy>,
}

#[derive(Debug, Clone)]
pub struct App {
    pub name: String,
    pub project: String,
    pub sync: String,
    pub health: String,
    pub repo: String,
    pub path: String,
    pub revision: String,
    pub sources: Vec<Value>,
    pub multi: bool,
    pub automated: Option<Policy>,
    pub stored: Option<Policy>,
    pub scale: Option<ScaleMemory>,
    pub appset: Option<String>,
    pub managed_by: Option<String>,
    pub raw: Value,
}

fn s(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

pub fn namespace(cfg: Option<&CubiConfig>) -> String {
    cfg.and_then(|c| c.str_in("argocd", "namespace"))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "argocd".into())
}

pub fn sources_of(spec: &Value) -> Vec<Value> {
    if let Some(list) = spec.get("sources").and_then(Value::as_array)
        && !list.is_empty()
    {
        return list.iter().filter(|v| v.is_object()).cloned().collect();
    }
    match spec.get("source") {
        Some(v) if v.is_object() => vec![v.clone()],
        _ => vec![],
    }
}

pub fn parse_sync(value: Option<&str>) -> Option<Policy> {
    match serde_json::from_str::<Value>(value?) {
        Ok(Value::Object(m)) => Some(m),
        _ => None,
    }
}

pub fn parse_scale(value: Option<&str>) -> Option<ScaleMemory> {
    let data = parse_sync(value)?;
    let reps = data.get("replicas")?.as_object()?;
    if reps.is_empty() {
        return None;
    }
    let mut replicas = BTreeMap::new();
    for (k, v) in reps {
        let n = v
            .as_i64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))?;
        replicas.insert(k.clone(), n);
    }
    let auto_sync = data.get("autoSync").and_then(Value::as_object).cloned();
    Some(ScaleMemory {
        replicas,
        auto_sync,
    })
}

pub fn appset_of(meta: &Value) -> Option<String> {
    meta.get("ownerReferences")?
        .as_array()?
        .iter()
        .find(|o| o.get("kind").and_then(Value::as_str) == Some("ApplicationSet"))
        .and_then(|o| o.get("name").and_then(Value::as_str))
        .filter(|n| !n.is_empty())
        .map(str::to_string)
}

pub fn managed_by(meta: &Value) -> Option<String> {
    let owners: Vec<&Value> = meta
        .get("ownerReferences")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|o| {
                    o.is_object() && o.get("kind").and_then(Value::as_str) != Some("ApplicationSet")
                })
                .collect()
        })
        .unwrap_or_default();
    if let Some(o) = owners.first() {
        let kind = o.get("kind").and_then(Value::as_str).unwrap_or("Owner");
        return Some(format!("{kind} {}", s(o, "name")).trim().to_string());
    }
    if meta
        .get("annotations")
        .and_then(Value::as_object)
        .is_some_and(|a| a.contains_key(TRACKING_ANNOTATION))
    {
        return Some("einer übergeordneten App".into());
    }
    None
}

pub fn normalize(item: &Value, state: &BTreeMap<String, String>) -> App {
    let meta = item.get("metadata").cloned().unwrap_or(Value::Null);
    let spec = item.get("spec").cloned().unwrap_or(Value::Null);
    let status = item.get("status").cloned().unwrap_or(Value::Null);
    let name = meta
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_string();
    let sources = sources_of(&spec);
    let first = sources.first().cloned().unwrap_or(Value::Null);
    let automated = spec
        .get("syncPolicy")
        .and_then(|p| p.get("automated"))
        .and_then(Value::as_object)
        .cloned();
    App {
        project: spec
            .get("project")
            .and_then(Value::as_str)
            .unwrap_or("default")
            .into(),
        sync: status
            .pointer("/sync/status")
            .and_then(Value::as_str)
            .unwrap_or("Unknown")
            .into(),
        health: status
            .pointer("/health/status")
            .and_then(Value::as_str)
            .unwrap_or("Unknown")
            .into(),
        repo: s(&first, "repoURL"),
        path: s(&first, "path"),
        revision: s(&first, "targetRevision"),
        multi: spec
            .get("sources")
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty()),
        sources,
        automated,
        stored: parse_sync(state.get(&format!("{name}.sync")).map(String::as_str)),
        scale: parse_scale(state.get(&format!("{name}.scale")).map(String::as_str)),
        appset: appset_of(&meta),
        managed_by: managed_by(&meta),
        raw: item.clone(),
        name,
    }
}

fn not_found(stderr: &str) -> bool {
    let l = stderr.to_lowercase();
    l.contains("not found") || l.contains("notfound")
}

pub async fn load_state(ns: &str) -> Result<BTreeMap<String, String>> {
    let out = shell::capture(
        &[
            "kubectl",
            "get",
            "configmap",
            STATE_CONFIGMAP,
            "-n",
            ns,
            "-o",
            "json",
        ],
        30,
    )
    .await?;
    if !out.ok() {
        if not_found(&out.stderr) {
            return Ok(BTreeMap::new());
        }
        bail!(
            "Zustand ({STATE_CONFIGMAP}) konnte nicht gelesen werden: {}",
            shell::first_line(&out.stderr)
        );
    }
    let v: Value = serde_json::from_str(&out.stdout).unwrap_or(Value::Null);
    Ok(v.get("data")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        v.as_str().map_or_else(|| v.to_string(), str::to_string),
                    )
                })
                .collect()
        })
        .unwrap_or_default())
}

pub async fn save_state(ns: &str, key: &str, value: Option<&str>) -> Result<()> {
    if value.is_some() {
        let created = shell::capture(
            &["kubectl", "create", "configmap", STATE_CONFIGMAP, "-n", ns],
            30,
        )
        .await?;
        if !created.ok()
            && !created
                .stderr
                .to_lowercase()
                .replace(' ', "")
                .contains("alreadyexists")
        {
            bail!(
                "Zustand ({STATE_CONFIGMAP}) konnte nicht angelegt werden: {}",
                shell::first_line(&created.stderr)
            );
        }
    }
    let patch = json!({"data": {key: value}}).to_string();
    let out = shell::capture(
        &[
            "kubectl",
            "patch",
            "configmap",
            STATE_CONFIGMAP,
            "-n",
            ns,
            "--type=merge",
            "-p",
            &patch,
        ],
        30,
    )
    .await?;
    if !out.ok() && !(value.is_none() && not_found(&out.stderr)) {
        bail!(
            "Zustand ({STATE_CONFIGMAP}) konnte nicht geschrieben werden: {}",
            shell::first_line(&out.stderr)
        );
    }
    Ok(())
}

pub async fn fetch_apps(ns: &str, with_state: bool) -> Result<Vec<App>> {
    let data = shell::capture_json(
        &["kubectl", "get", APP_RESOURCE, "-n", ns, "-o", "json"],
        30,
    )
    .await
    .map_err(|e| anyhow!("ArgoCD-Apps konnten nicht gelesen werden: {e:#}"))?;
    let state = if with_state {
        load_state(ns).await?
    } else {
        BTreeMap::new()
    };
    Ok(data
        .get("items")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|i| normalize(i, &state)).collect())
        .unwrap_or_default())
}

pub async fn patch_app(ns: &str, name: &str, patch: &Value, kind: &str) -> Result<()> {
    let p = patch.to_string();
    let t = format!("--type={kind}");
    shell::capture_ok(
        &[
            "kubectl",
            "patch",
            APP_RESOURCE,
            name,
            "-n",
            ns,
            &t,
            "-p",
            &p,
        ],
        30,
        &format!("{name}: Änderung fehlgeschlagen"),
    )
    .await?;
    Ok(())
}

pub async fn read_appset(ns: &str, appset: &str) -> Result<Value> {
    shell::capture_json(
        &[
            "kubectl",
            "get",
            APPSET_RESOURCE,
            appset,
            "-n",
            ns,
            "-o",
            "json",
        ],
        30,
    )
    .await
}

pub fn hold_rule(app: &str, pointer: &str) -> Value {
    json!({"name": app, "jsonPointers": [pointer]})
}

pub fn hold_operations(appset: &Value, app: &str, pointer: &str, hold: bool) -> Option<Vec<Value>> {
    let rules: Vec<Value> = appset
        .pointer("/spec/ignoreApplicationDifferences")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let rule = hold_rule(app, pointer);
    if hold == rules.contains(&rule) {
        return None;
    }
    let rules: Vec<Value> = if hold {
        rules.into_iter().chain([rule]).collect()
    } else {
        rules.into_iter().filter(|r| r != &rule).collect()
    };
    let version = appset
        .pointer("/metadata/resourceVersion")
        .cloned()
        .unwrap_or(Value::Null);
    let mut ops =
        vec![json!({"op": "test", "path": "/metadata/resourceVersion", "value": version})];
    if rules.is_empty() {
        ops.push(json!({"op": "remove", "path": "/spec/ignoreApplicationDifferences"}));
    } else {
        ops.push(
            json!({"op": "add", "path": "/spec/ignoreApplicationDifferences", "value": rules}),
        );
    }
    Some(ops)
}

pub async fn set_hold(
    ns: &str,
    appset: &str,
    app: &str,
    pointer: &str,
    hold: bool,
) -> Result<bool> {
    let data = read_appset(ns, appset).await?;
    let Some(ops) = hold_operations(&data, app, pointer, hold) else {
        return Ok(false);
    };
    let p = Value::Array(ops).to_string();
    shell::capture_ok(
        &["kubectl", "patch", APPSET_RESOURCE, appset, "-n", ns, "--type=json", "-p", &p],
        30,
        &format!("ApplicationSet {appset}: Regel für {app} konnte nicht geändert werden (parallel geändert?)"),
    )
    .await?;
    Ok(true)
}

pub async fn template_revision(
    ns: &str,
    appset: &str,
    multi: bool,
    index: usize,
) -> Option<String> {
    let data = read_appset(ns, appset).await.ok()?;
    let spec = data.pointer("/spec/template/spec")?;
    let src = if multi {
        spec.get("sources")?.as_array()?.get(index)?.clone()
    } else {
        spec.get("source")?.clone()
    };
    let v = src.get("targetRevision")?.as_str()?;
    (!v.contains("{{")).then(|| v.to_string())
}

pub async fn pause_auto_sync(ns: &str, app: &App) -> Result<()> {
    if let Some(a) = &app.appset {
        set_hold(ns, a, &app.name, SYNC_POINTER, true).await?;
    }
    patch_app(
        ns,
        &app.name,
        &json!({"spec": {"syncPolicy": {"automated": null}}}),
        "merge",
    )
    .await
}

pub async fn resume_auto_sync(ns: &str, app: &App, automated: &Policy) -> Result<()> {
    patch_app(
        ns,
        &app.name,
        &json!({"spec": {"syncPolicy": {"automated": automated}}}),
        "merge",
    )
    .await?;
    if let Some(a) = &app.appset {
        set_hold(ns, a, &app.name, SYNC_POINTER, false).await?;
    }
    Ok(())
}

pub fn appset_note(app: &App, what: &str) -> Vec<String> {
    if let Some(a) = &app.appset {
        return vec![format!(
            "ApplicationSet {a}: {what} wird per ignoreApplicationDifferences festgehalten"
        )];
    }
    if let Some(m) = &app.managed_by {
        return vec![format!(
            "wird von {m} verwaltet, die Änderung kann dort zurückgesetzt werden"
        )];
    }
    vec![]
}

pub async fn recheck(ns: &str, names: &[String]) -> Result<BTreeMap<String, App>> {
    tokio::time::sleep(std::time::Duration::from_secs(SETTLE_SECONDS)).await;
    Ok(fetch_apps(ns, true)
        .await?
        .into_iter()
        .filter(|a| names.contains(&a.name))
        .map(|a| (a.name.clone(), a))
        .collect())
}

pub fn check_lines(checks: &[(String, bool)], hint: &str) -> Vec<(Tone, String)> {
    checks
        .iter()
        .map(|(name, ok)| {
            if *ok {
                (Tone::Ok, format!("✔ {name}: Änderung hält"))
            } else {
                (Tone::Warn, format!("⚠ {name}: {hint}"))
            }
        })
        .collect()
}

pub fn policy_flags(automated: Option<&Policy>) -> String {
    let Some(a) = automated else {
        return String::new();
    };
    ["prune", "selfHeal", "allowEmpty"]
        .iter()
        .filter(|k| a.get(**k).and_then(Value::as_bool).unwrap_or(false))
        .copied()
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn policy_label(automated: Option<&Policy>) -> String {
    match automated {
        None => "manual".into(),
        Some(_) => {
            let f = policy_flags(automated);
            if f.is_empty() {
                "auto".into()
            } else {
                format!("auto ({f})")
            }
        }
    }
}

pub fn revision_of(source: &Value) -> String {
    source
        .get("targetRevision")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("HEAD")
        .to_string()
}

pub fn revision_label(app: &App) -> String {
    if app.sources.is_empty() {
        return "-".into();
    }
    app.sources
        .iter()
        .map(revision_of)
        .collect::<Vec<_>>()
        .join(" | ")
}

pub fn short_repo(url: &str) -> String {
    let t = url.trim_end_matches('/');
    let t = t.strip_suffix(".git").unwrap_or(t);
    let parts: Vec<&str> = t.split(['/', ':']).collect();
    if parts.len() >= 2 {
        parts[parts.len() - 2..].join("/")
    } else if t.is_empty() {
        "-".into()
    } else {
        t.into()
    }
}

pub fn is_problem(app: &App) -> bool {
    app.sync != "Synced" || app.health != "Healthy"
}

pub fn rank(app: &App) -> (u8, String) {
    let h = match app.health.as_str() {
        "Degraded" => 0,
        "Missing" => 2,
        "Progressing" => 3,
        "Suspended" => 5,
        "Healthy" => 9,
        _ => 4,
    };
    let s = match app.sync.as_str() {
        "OutOfSync" => 1,
        "Synced" => 9,
        _ => 4,
    };
    (h.min(s), app.name.clone())
}

pub fn sync_tone(s: &str) -> Tone {
    match s {
        "Synced" => Tone::Ok,
        "OutOfSync" => Tone::Warn,
        "Unknown" => Tone::Special,
        _ => Tone::Plain,
    }
}

pub fn health_tone(s: &str) -> Tone {
    match s {
        "Healthy" => Tone::Ok,
        "Progressing" => Tone::Warn,
        "Degraded" | "Missing" => Tone::Bad,
        "Suspended" => Tone::Info,
        "Unknown" => Tone::Special,
        _ => Tone::Plain,
    }
}

pub fn policy_tone(a: Option<&Policy>) -> Tone {
    if a.is_some() { Tone::Ok } else { Tone::Warn }
}

#[derive(Debug, Clone)]
pub struct Context {
    pub name: String,
    pub server: String,
    pub core: bool,
    pub plaintext: bool,
    pub insecure: bool,
    pub username: Option<String>,
}

pub fn configured_contexts(cfg: &CubiConfig) -> Result<Vec<Context>> {
    let section = cfg.section("argocd");
    let Some(serde_yaml_ng::Value::Sequence(entries)) = section.get("contexts") else {
        return Ok(vec![]);
    };
    let mut out = Vec::new();
    for e in entries {
        let get = |k: &str| e.get(k).and_then(as_str);
        let flag = |k: &str| {
            e.get(k)
                .and_then(serde_yaml_ng::Value::as_bool)
                .unwrap_or(false)
        };
        let name = match e {
            serde_yaml_ng::Value::String(s) => s.clone(),
            _ => get("name").filter(|n| !n.is_empty()).ok_or_else(|| {
                anyhow!("Ungültiger Eintrag in argocd.contexts: name ist Pflicht")
            })?,
        };
        out.push(Context {
            server: get("server").unwrap_or_else(|| name.clone()),
            core: flag("core"),
            plaintext: flag("plaintext"),
            insecure: flag("insecure"),
            username: get("username"),
            name,
        });
    }
    Ok(out)
}

pub fn argocd_config_file() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".config/argocd/config")
}

pub fn read_argocd_config() -> serde_yaml_ng::Value {
    std::fs::read_to_string(argocd_config_file())
        .ok()
        .and_then(|t| serde_yaml_ng::from_str(&t).ok())
        .unwrap_or(serde_yaml_ng::Value::Null)
}

pub fn current_context() -> String {
    read_argocd_config()
        .get("current-context")
        .and_then(as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "-".into())
}

#[derive(Debug, PartialEq, Eq)]
pub enum TokenState {
    None,
    Invalid,
    Valid(Option<i64>),
    Expired(i64),
}

fn b64url_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0;
    for c in input.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => continue,
            _ => return None,
        };
        buf = (buf << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

pub fn jwt_exp(token: &str) -> Option<Option<i64>> {
    let payload = token.split('.').nth(1)?;
    let bytes = b64url_decode(payload)?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    Some(v.get("exp").and_then(Value::as_i64))
}

pub fn token_status(config: &serde_yaml_ng::Value, name: &str, now: i64) -> TokenState {
    let find = |list: &str, key: &str, value: &str| {
        config
            .get(list)?
            .as_sequence()?
            .iter()
            .find(|i| i.get(key).and_then(as_str).as_deref() == Some(value))
            .cloned()
    };
    let Some(ctx) = find("contexts", "name", name) else {
        return TokenState::None;
    };
    let user_name = ctx.get("user").and_then(as_str).unwrap_or_default();
    let Some(token) = find("users", "name", &user_name)
        .and_then(|u| u.get("auth-token").and_then(as_str))
        .filter(|t| !t.is_empty())
    else {
        return TokenState::None;
    };
    match jwt_exp(&token) {
        None => TokenState::Invalid,
        Some(None) => TokenState::Valid(None),
        Some(Some(exp)) if exp > now => TokenState::Valid(Some(exp)),
        Some(Some(exp)) => TokenState::Expired(exp),
    }
}

pub async fn activate(name: &str) -> Result<()> {
    shell::capture_ok(
        &["argocd", "context", name],
        15,
        &format!("Kontext {name} konnte nicht aktiviert werden"),
    )
    .await?;
    Ok(())
}

// ---------------- Auto-Sync ----------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncAction {
    Disable,
    Enable,
}

#[derive(Debug, Clone)]
pub struct SyncChange {
    pub app: App,
    pub action: SyncAction,
    pub state_value: Option<String>,
    pub restore: Option<Policy>,
    pub before: String,
    pub after: String,
    pub notes: Vec<String>,
}

pub fn plan_sync(app: &App, action: SyncAction) -> Result<SyncChange, String> {
    let automated = app.automated.as_ref();
    if action == SyncAction::Disable {
        let Some(a) = automated else {
            return Err("bereits manual".into());
        };
        let flags = policy_flags(automated);
        let mut notes = vec![format!(
            "Einstellungen werden gemerkt ({})",
            if flags.is_empty() {
                "ohne prune/selfHeal".into()
            } else {
                flags
            }
        )];
        notes.extend(appset_note(app, "die Sync-Policy"));
        return Ok(SyncChange {
            app: app.clone(),
            action,
            state_value: Some(sorted_json(&Value::Object(a.clone()))),
            restore: None,
            before: policy_label(automated),
            after: "manual".into(),
            notes,
        });
    }
    if automated.is_some() {
        return Err("bereits auto".into());
    }
    let restored = app.stored.clone().unwrap_or_default();
    let flags = policy_flags(Some(&restored));
    let flags = if flags.is_empty() {
        "ohne prune/selfHeal".to_string()
    } else {
        flags
    };
    let mut notes = vec![if app.stored.is_some() {
        format!("gemerkte Einstellungen werden wiederhergestellt ({flags})")
    } else {
        "keine gemerkten Einstellungen, automated ohne prune/selfHeal".into()
    }];
    if let Some(a) = &app.appset {
        notes.push(format!(
            "ApplicationSet {a}: Regel wird entfernt, der Controller setzt die Policy aus dem Template"
        ));
    } else if let Some(m) = &app.managed_by {
        notes.push(format!(
            "wird von {m} verwaltet, die Änderung kann dort zurückgesetzt werden"
        ));
    }
    Ok(SyncChange {
        app: app.clone(),
        action,
        state_value: None,
        after: policy_label(Some(&restored)),
        restore: Some(restored),
        before: "manual".into(),
        notes,
    })
}

pub fn sorted_json(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            let sorted: BTreeMap<&String, &Value> = m.iter().collect();
            let parts: Vec<String> = sorted
                .into_iter()
                .map(|(k, v)| format!("{}: {}", Value::String(k.clone()), sorted_json(v)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(sorted_json).collect::<Vec<_>>().join(", ")
        ),
        other => other.to_string(),
    }
}

pub async fn apply_sync(ns: &str, changes: Vec<SyncChange>) -> Vec<(Tone, String)> {
    let mut lines = Vec::new();
    let mut done = Vec::new();
    for c in changes {
        let key = format!("{}.sync", c.app.name);
        let r = match c.action {
            SyncAction::Disable => {
                async {
                    save_state(ns, &key, c.state_value.as_deref()).await?;
                    pause_auto_sync(ns, &c.app).await
                }
                .await
            }
            SyncAction::Enable => {
                async {
                    resume_auto_sync(ns, &c.app, c.restore.as_ref().unwrap_or(&Policy::new()))
                        .await?;
                    save_state(ns, &key, None).await
                }
                .await
            }
        };
        match r {
            Ok(()) => {
                let verb = if c.action == SyncAction::Disable {
                    "deaktiviert"
                } else {
                    "aktiviert"
                };
                lines.push((
                    Tone::Ok,
                    format!("✔ {}: Auto-Sync {verb} ({})", c.app.name, c.after),
                ));
                done.push(c);
            }
            Err(e) => lines.push((Tone::Bad, format!("✖ {}: {e:#}", c.app.name))),
        }
    }
    if !done.is_empty() {
        lines.push((Tone::Muted, "Prüfe, ob die Änderung hält …".into()));
        let names: Vec<String> = done.iter().map(|c| c.app.name.clone()).collect();
        match recheck(ns, &names).await {
            Ok(cur) => {
                let checks: Vec<(String, bool)> = done
                    .iter()
                    .map(|c| {
                        let want_manual = c.action == SyncAction::Disable;
                        let ok = cur
                            .get(&c.app.name)
                            .is_some_and(|a| a.automated.is_none() == want_manual);
                        (c.app.name.clone(), ok)
                    })
                    .collect();
                lines.extend(check_lines(
                    &checks,
                    "wurde zurückgesetzt, ein Controller überschreibt die Sync-Policy",
                ));
            }
            Err(e) => lines.push((Tone::Bad, format!("✖ Nachprüfung fehlgeschlagen: {e:#}"))),
        }
    }
    lines
}

// ---------------- Feature-Testing ----------------

#[derive(Debug, Clone)]
pub struct BranchChange {
    pub app: App,
    pub index: usize,
    pub source: Value,
    pub previous: String,
    pub target: String,
    pub patch: Value,
    pub notes: Vec<String>,
    pub hold: Option<bool>,
    pub remote: Option<bool>,
}

pub fn source_pointer(app: &App, index: usize) -> String {
    if app.multi {
        format!("/spec/sources/{index}")
    } else {
        "/spec/source".into()
    }
}

pub fn branch_patch(app: &App, index: usize, target: &str) -> Value {
    let p = source_pointer(app, index);
    let repo = app
        .sources
        .get(index)
        .map(|s| s_or(s, "repoURL"))
        .unwrap_or_default();
    json!([
        {"op": "test", "path": format!("{p}/repoURL"), "value": repo},
        {"op": "add", "path": format!("{p}/targetRevision"), "value": target},
    ])
}

fn s_or(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

pub fn source_title(source: &Value) -> String {
    let detail = match (
        source.get("path").and_then(Value::as_str),
        source.get("ref").and_then(Value::as_str),
    ) {
        (Some(p), _) if !p.is_empty() => p.to_string(),
        (_, Some(r)) if !r.is_empty() => format!("ref {r}"),
        _ => String::new(),
    };
    format!(
        "{} {detail} @ {}",
        short_repo(&s_or(source, "repoURL")),
        revision_of(source)
    )
    .replace("  ", " ")
}

pub fn auto_sync_notes(app: &App) -> Vec<String> {
    let Some(a) = &app.automated else {
        return vec![];
    };
    let flags = policy_flags(Some(a));
    let mut notes = vec![format!(
        "Auto-Sync aktiv{}: ArgoCD synct sofort auf den neuen Branch",
        if flags.is_empty() {
            String::new()
        } else {
            format!(" ({flags})")
        }
    )];
    if a.get("prune").and_then(Value::as_bool).unwrap_or(false) {
        notes.push("prune aktiv: Ressourcen, die im neuen Branch fehlen, werden gelöscht".into());
    }
    notes
}

pub fn plan_branch(
    app: &App,
    index: usize,
    target: &str,
    template: Option<&str>,
) -> Result<BranchChange, String> {
    let source = app.sources.get(index).cloned().ok_or("keine Source")?;
    let previous = revision_of(&source);
    if previous == target {
        return Err(format!("steht bereits auf {target}"));
    }
    let mut notes = auto_sync_notes(app);
    let mut hold = None;
    if let Some(a) = &app.appset {
        let h = template != Some(target);
        hold = Some(h);
        notes.push(if h {
            format!("ApplicationSet {a}: targetRevision wird per ignoreApplicationDifferences festgehalten")
        } else {
            format!("ApplicationSet {a}: entspricht dem Template, die Regel wird entfernt")
        });
    } else if let Some(m) = &app.managed_by {
        notes.push(format!(
            "wird von {m} verwaltet, die Änderung kann dort zurückgesetzt werden"
        ));
    }
    Ok(BranchChange {
        patch: branch_patch(app, index, target),
        app: app.clone(),
        index,
        source,
        previous,
        target: target.into(),
        notes,
        hold,
        remote: None,
    })
}

pub fn off_target(app: &App) -> bool {
    app.sources.iter().any(|s| revision_of(s) != DEFAULT_BRANCH)
}

pub fn is_commit_id(s: &str) -> bool {
    (7..=40).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn repo_from_url(url: &str) -> Option<String> {
    let i = url.find("github.com")?;
    let rest = url[i + "github.com".len()..].trim_start_matches([':', '/']);
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let mut parts = rest.split('/');
    let (o, r) = (parts.next()?, parts.next()?);
    if parts.next().is_some() || o.is_empty() || r.is_empty() {
        return None;
    }
    Some(format!("{o}/{r}"))
}

fn quote_ref(r: &str) -> String {
    r.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub async fn ref_exists(repo_url: &str, r: &str) -> Option<bool> {
    if let Some(repo) = repo_from_url(repo_url) {
        for kind in ["heads", "tags"] {
            let path = format!("repos/{repo}/git/matching-refs/{kind}/{}", quote_ref(r));
            let data = shell::capture_json(&["gh", "api", &path], 20).await.ok()?;
            let want = format!("refs/{kind}/{r}");
            if data.as_array().is_some_and(|a| {
                a.iter()
                    .any(|e| e.get("ref").and_then(Value::as_str) == Some(&want))
            }) {
                return Some(true);
            }
        }
        return if is_commit_id(r) { None } else { Some(false) };
    }
    let heads = format!("refs/heads/{r}");
    let tags = format!("refs/tags/{r}");
    let out = shell::capture_in(
        &["git", "ls-remote", "--exit-code", repo_url, &heads, &tags],
        20,
        None,
        &[
            ("GIT_TERMINAL_PROMPT", "0"),
            ("GIT_ASKPASS", "true"),
            ("SSH_ASKPASS", "true"),
        ],
    )
    .await
    .ok()?;
    match out.code {
        0 => Some(true),
        2 if !is_commit_id(r) => Some(false),
        _ => None,
    }
}

pub async fn plan_branches(
    ns: &str,
    picks: Vec<(App, usize)>,
    target: String,
) -> Result<(Vec<BranchChange>, Vec<(String, String)>)> {
    let mut changes = Vec::new();
    let mut skipped = Vec::new();
    let mut templates: BTreeMap<(String, bool, usize), Option<String>> = BTreeMap::new();
    for (app, index) in picks {
        if app.sources.is_empty() {
            skipped.push((app.name.clone(), "keine Source".into()));
            continue;
        }
        let template = match &app.appset {
            Some(a) => {
                let key = (a.clone(), app.multi, index);
                if !templates.contains_key(&key) {
                    templates.insert(
                        key.clone(),
                        template_revision(ns, a, app.multi, index).await,
                    );
                }
                templates[&key].clone()
            }
            None => None,
        };
        match plan_branch(&app, index, &target, template.as_deref()) {
            Ok(c) => changes.push(c),
            Err(r) => skipped.push((app.name.clone(), r)),
        }
    }
    let mut remote: BTreeMap<(String, String), Option<bool>> = BTreeMap::new();
    for c in &mut changes {
        let key = (s_or(&c.source, "repoURL"), c.target.clone());
        if !remote.contains_key(&key) {
            remote.insert(key.clone(), ref_exists(&key.0, &key.1).await);
        }
        c.remote = remote[&key];
    }
    Ok((changes, skipped))
}

pub async fn apply_branches(ns: &str, changes: Vec<BranchChange>) -> Vec<(Tone, String)> {
    let mut lines = Vec::new();
    let mut done = Vec::new();
    for c in changes {
        let pointer = format!("{}/targetRevision", source_pointer(&c.app, c.index));
        let r = async {
            if let (Some(a), Some(true)) = (&c.app.appset, c.hold) {
                set_hold(ns, a, &c.app.name, &pointer, true).await?;
            }
            patch_app(ns, &c.app.name, &c.patch, "json").await?;
            if let (Some(a), Some(false)) = (&c.app.appset, c.hold) {
                set_hold(ns, a, &c.app.name, &pointer, false).await?;
            }
            anyhow::Ok(())
        }
        .await;
        match r {
            Ok(()) => {
                lines.push((
                    Tone::Ok,
                    format!("✔ {}: {} → {}", c.app.name, c.previous, c.target),
                ));
                done.push(c);
            }
            Err(e) => lines.push((Tone::Bad, format!("✖ {}: {e:#}", c.app.name))),
        }
    }
    if !done.is_empty() {
        lines.push((Tone::Muted, "Prüfe, ob die Änderung hält …".into()));
        let names: Vec<String> = done.iter().map(|c| c.app.name.clone()).collect();
        match recheck(ns, &names).await {
            Ok(cur) => {
                let checks: Vec<(String, bool)> = done
                    .iter()
                    .map(|c| {
                        let rev = cur
                            .get(&c.app.name)
                            .and_then(|a| a.sources.get(c.index))
                            .map(revision_of);
                        (
                            c.app.name.clone(),
                            rev.as_deref() == Some(c.target.as_str()),
                        )
                    })
                    .collect();
                lines.extend(check_lines(
                    &checks,
                    "wurde zurückgesetzt, ein Controller überschreibt targetRevision",
                ));
            }
            Err(e) => lines.push((Tone::Bad, format!("✖ Nachprüfung fehlgeschlagen: {e:#}"))),
        }
    }
    lines
}

// ---------------- Skalieren ----------------

pub const KINDS: &[(&str, &str)] = &[
    ("Deployment", "deployments.apps"),
    ("StatefulSet", "statefulsets.apps"),
];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Workload {
    pub kind: String,
    pub namespace: String,
    pub name: String,
}

impl Workload {
    pub fn key(&self) -> String {
        format!("{}/{}/{}", self.kind, self.namespace, self.name)
    }
    pub fn target(&self) -> String {
        let res = KINDS
            .iter()
            .find(|(k, _)| *k == self.kind)
            .map_or("deployments.apps", |(_, r)| r);
        format!("{res}/{}", self.name)
    }
    pub fn from_key(key: &str) -> Option<Self> {
        let mut p = key.splitn(3, '/');
        Some(Self {
            kind: p.next()?.into(),
            namespace: p.next()?.into(),
            name: p.next()?.into(),
        })
    }
}

pub fn workloads(app: &App) -> Vec<Workload> {
    let fallback = app
        .raw
        .pointer("/spec/destination/namespace")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("default")
        .to_string();
    let mut found: Vec<Workload> = Vec::new();
    for r in app
        .raw
        .pointer("/status/resources")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let kind = s_or(r, "kind");
        let name = s_or(r, "name");
        if s_or(r, "group") == "apps" && KINDS.iter().any(|(k, _)| *k == kind) && !name.is_empty() {
            let ns = Some(s_or(r, "namespace"))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| fallback.clone());
            let w = Workload {
                kind,
                namespace: ns,
                name,
            };
            if !found.contains(&w) {
                found.push(w);
            }
        }
    }
    found
}

pub async fn fetch_live(items: &[Workload]) -> Result<BTreeMap<String, i64>> {
    let mut live = BTreeMap::new();
    let mut namespaces: Vec<&str> = items.iter().map(|w| w.namespace.as_str()).collect();
    namespaces.sort();
    namespaces.dedup();
    let kinds = KINDS.iter().map(|(_, r)| *r).collect::<Vec<_>>().join(",");
    for ns in namespaces {
        let data =
            shell::capture_json(&["kubectl", "get", &kinds, "-n", ns, "-o", "json"], 30).await?;
        for e in data
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let w = Workload {
                kind: s_or(e, "kind"),
                namespace: ns.into(),
                name: e
                    .pointer("/metadata/name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
            };
            live.insert(
                w.key(),
                e.pointer("/spec/replicas")
                    .and_then(Value::as_i64)
                    .unwrap_or(1),
            );
        }
    }
    Ok(live)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleAction {
    Down,
    Set,
    Up,
}

#[derive(Debug, Clone)]
pub struct ScaleChange {
    pub app: App,
    pub action: ScaleAction,
    pub state_value: Option<String>,
    pub pause: bool,
    pub resume: Option<Policy>,
    pub ops: Vec<(Workload, i64)>,
    pub before: String,
    pub after: String,
    pub notes: Vec<String>,
}

pub fn plan_set(
    app: &App,
    live: &BTreeMap<String, i64>,
    count: i64,
    only: Option<&[String]>,
) -> Result<ScaleChange, String> {
    let available = workloads(app);
    if available.is_empty() {
        return Err("keine Deployments/StatefulSets".into());
    }
    let items: Vec<&Workload> = available
        .iter()
        .filter(|w| only.is_none_or(|o| o.contains(&w.key())))
        .collect();
    if items.is_empty() {
        return Err("kein Workload gewählt".into());
    }
    let changing: BTreeMap<String, i64> = items
        .iter()
        .filter_map(|w| {
            live.get(&w.key())
                .filter(|n| **n != count)
                .map(|n| (w.key(), *n))
        })
        .collect();
    if changing.is_empty() {
        return Err(format!("bereits {count} Pods"));
    }
    let memory = app.scale.clone().unwrap_or(ScaleMemory {
        replicas: BTreeMap::new(),
        auto_sync: None,
    });
    let mut replicas = changing.clone();
    replicas.extend(memory.replicas.clone());
    let automated = app.automated.clone().or(memory.auto_sync);
    let state_value = sorted_json(&json!({"replicas": replicas, "autoSync": automated}));
    let pause = app.automated.is_some();
    let mut notes = Vec::new();
    if pause {
        let flags = policy_flags(app.automated.as_ref());
        notes.push(format!(
            "Auto-Sync wird deaktiviert ({}), sonst setzt ArgoCD die Replikas aus Git zurück",
            if flags.is_empty() {
                "ohne prune/selfHeal".into()
            } else {
                flags
            }
        ));
        notes.extend(appset_note(app, "die Sync-Policy"));
    }
    let ops: Vec<(Workload, i64)> = changing
        .keys()
        .filter_map(|k| Workload::from_key(k))
        .map(|w| (w, count))
        .collect();
    Ok(ScaleChange {
        app: app.clone(),
        action: if count == 0 {
            ScaleAction::Down
        } else {
            ScaleAction::Set
        },
        state_value: Some(state_value),
        pause,
        resume: None,
        before: format!("{} Pods", changing.values().sum::<i64>()),
        after: format!("{} Pods", count * changing.len() as i64),
        ops,
        notes,
    })
}

pub fn plan_up(app: &App) -> Result<ScaleChange, String> {
    let Some(memory) = &app.scale else {
        return Err("keine gemerkten Replikas".into());
    };
    let resume = if app.automated.is_none() {
        memory.auto_sync.clone()
    } else {
        None
    };
    let mut notes = Vec::new();
    if let Some(r) = &resume {
        let flags = policy_flags(Some(r));
        notes.push(format!(
            "Auto-Sync wird wieder aktiviert ({})",
            if flags.is_empty() {
                "ohne prune/selfHeal".into()
            } else {
                flags
            }
        ));
        if let Some(a) = &app.appset {
            notes.push(format!("ApplicationSet {a}: Regel wird entfernt, der Controller setzt die Policy aus dem Template"));
        }
    }
    Ok(ScaleChange {
        app: app.clone(),
        action: ScaleAction::Up,
        state_value: None,
        pause: false,
        resume,
        ops: memory
            .replicas
            .iter()
            .filter_map(|(k, n)| Workload::from_key(k).map(|w| (w, *n)))
            .collect(),
        before: "0 Pods".into(),
        after: format!("{} Pods", memory.replicas.values().sum::<i64>()),
        notes,
    })
}

async fn scale_all(c: &ScaleChange, lines: &mut Vec<(Tone, String)>) -> bool {
    let mut ok = true;
    for (w, n) in &c.ops {
        let t = w.target();
        let r = format!("--replicas={n}");
        if let Err(e) = shell::capture_ok(
            &["kubectl", "scale", &t, "-n", &w.namespace, &r],
            30,
            &format!("{}/{}: Skalieren fehlgeschlagen", w.kind, w.name),
        )
        .await
        {
            ok = false;
            lines.push((Tone::Bad, format!("✖ {}: {e:#}", c.app.name)));
        }
    }
    ok
}

pub async fn apply_scale(ns: &str, changes: Vec<ScaleChange>) -> Vec<(Tone, String)> {
    let mut lines = Vec::new();
    let mut done = Vec::new();
    for c in changes {
        let key = format!("{}.scale", c.app.name);
        let r: Result<bool> = async {
            if c.action == ScaleAction::Up {
                let finished = scale_all(&c, &mut lines).await;
                if finished {
                    if let Some(p) = &c.resume {
                        resume_auto_sync(ns, &c.app, p).await?;
                    }
                    save_state(ns, &key, None).await?;
                }
                Ok(finished)
            } else {
                save_state(ns, &key, c.state_value.as_deref()).await?;
                if c.pause {
                    pause_auto_sync(ns, &c.app).await?;
                }
                Ok(scale_all(&c, &mut lines).await)
            }
        }
        .await;
        match r {
            Ok(true) => {
                lines.push((
                    Tone::Ok,
                    format!("✔ {}: {} → {}", c.app.name, c.before, c.after),
                ));
                done.push(c);
            }
            Ok(false) if c.action == ScaleAction::Up => lines.push((
                Tone::Warn,
                format!(
                    "  {}: gemerkte Replikas bleiben erhalten, erneut ausführen",
                    c.app.name
                ),
            )),
            Ok(false) => {}
            Err(e) => lines.push((Tone::Bad, format!("✖ {}: {e:#}", c.app.name))),
        }
    }
    if !done.is_empty() {
        lines.push((Tone::Muted, "Prüfe, ob die Änderung hält …".into()));
        let names: Vec<String> = done.iter().map(|c| c.app.name.clone()).collect();
        match recheck(ns, &names).await {
            Ok(cur) => {
                let mut checks = Vec::new();
                for c in &done {
                    let ok = if cur.contains_key(&c.app.name) {
                        let items: Vec<Workload> = c.ops.iter().map(|(w, _)| w.clone()).collect();
                        match fetch_live(&items).await {
                            Ok(live) => c
                                .ops
                                .iter()
                                .all(|(w, n)| live.get(&w.key()).copied().unwrap_or(0) == *n),
                            Err(_) => false,
                        }
                    } else {
                        false
                    };
                    checks.push((c.app.name.clone(), ok));
                }
                lines.extend(check_lines(
                    &checks,
                    "Replikas weichen ab, ein Controller überschreibt die Änderung",
                ));
            }
            Err(e) => lines.push((Tone::Bad, format!("✖ Nachprüfung fehlgeschlagen: {e:#}"))),
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(json: Value, state: &[(&str, &str)]) -> App {
        let st = state
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        normalize(&json, &st)
    }

    fn sample() -> Value {
        json!({
            "metadata": {"name": "web", "ownerReferences": [{"kind": "ApplicationSet", "name": "apps"}]},
            "spec": {
                "project": "tech",
                "source": {"repoURL": "https://github.com/pkr-lab/capulus-core.git", "path": "argocd/apps/web", "targetRevision": "main"},
                "destination": {"namespace": "web"},
                "syncPolicy": {"automated": {"prune": true, "selfHeal": true}}
            },
            "status": {
                "sync": {"status": "OutOfSync"}, "health": {"status": "Degraded"},
                "resources": [
                    {"group": "apps", "kind": "Deployment", "name": "web"},
                    {"group": "apps", "kind": "Deployment", "name": "web"},
                    {"group": "", "kind": "Service", "name": "web"}
                ]
            }
        })
    }

    #[test]
    fn normalizes_app() {
        let a = app(sample(), &[]);
        assert_eq!(a.project, "tech");
        assert_eq!(a.appset.as_deref(), Some("apps"));
        assert_eq!(a.managed_by, None);
        assert_eq!(policy_label(a.automated.as_ref()), "auto (prune, selfHeal)");
        assert_eq!(rank(&a).0, 0);
        assert!(is_problem(&a));
        assert_eq!(revision_label(&a), "main");
        assert_eq!(short_repo(&a.repo), "pkr-lab/capulus-core");
        assert_eq!(
            workloads(&a),
            vec![Workload {
                kind: "Deployment".into(),
                namespace: "web".into(),
                name: "web".into()
            }]
        );
    }

    #[test]
    fn managed_by_tracking() {
        let meta = json!({"annotations": {TRACKING_ANNOTATION: "x"}});
        assert_eq!(
            managed_by(&meta).as_deref(),
            Some("einer übergeordneten App")
        );
        let meta = json!({"ownerReferences": [{"kind": "Application", "name": "root"}]});
        assert_eq!(managed_by(&meta).as_deref(), Some("Application root"));
    }

    #[test]
    fn autosync_plans() {
        let a = app(sample(), &[]);
        let c = plan_sync(&a, SyncAction::Disable).unwrap();
        assert_eq!(c.after, "manual");
        assert_eq!(
            c.state_value.as_deref(),
            Some("{\"prune\": true, \"selfHeal\": true}")
        );
        assert!(c.notes[1].contains("ApplicationSet apps"));
        assert_eq!(
            plan_sync(&a, SyncAction::Enable).unwrap_err(),
            "bereits auto"
        );
        let mut m = sample();
        m["spec"]["syncPolicy"] = json!({});
        let b = app(m, &[("web.sync", "{\"prune\": true}")]);
        let c = plan_sync(&b, SyncAction::Enable).unwrap();
        assert_eq!(c.after, "auto (prune)");
        assert!(c.notes[0].contains("wiederhergestellt"));
    }

    #[test]
    fn hold_ops() {
        let set = json!({"metadata": {"resourceVersion": "7"}, "spec": {}});
        let ops = hold_operations(&set, "web", SYNC_POINTER, true).unwrap();
        assert_eq!(ops[1]["op"], "add");
        let held = json!({"metadata": {"resourceVersion": "8"}, "spec": {"ignoreApplicationDifferences": [hold_rule("web", SYNC_POINTER)]}});
        assert!(hold_operations(&held, "web", SYNC_POINTER, true).is_none());
        assert_eq!(
            hold_operations(&held, "web", SYNC_POINTER, false).unwrap()[1]["op"],
            "remove"
        );
    }

    #[test]
    fn branch_plans() {
        let a = app(sample(), &[]);
        assert!(plan_branch(&a, 0, "main", None).is_err());
        let c = plan_branch(&a, 0, "feat/x", Some("main")).unwrap();
        assert_eq!(c.hold, Some(true));
        assert_eq!(c.patch[1]["path"], "/spec/source/targetRevision");
        assert!(c.notes.iter().any(|n| n.contains("prune aktiv")));
        let c = plan_branch(&a, 0, "feat/x", Some("feat/x")).unwrap();
        assert_eq!(c.hold, Some(false));
        assert!(!off_target(&a));
        assert!(is_commit_id("abcdef1"));
        assert!(!is_commit_id("main"));
        assert_eq!(
            repo_from_url("git@github.com:pkr-lab/x.git").as_deref(),
            Some("pkr-lab/x")
        );
        assert_eq!(quote_ref("feat/ä b"), "feat/%C3%A4%20b");
    }

    #[test]
    fn scale_plans() {
        let a = app(sample(), &[]);
        let live = BTreeMap::from([("Deployment/web/web".to_string(), 2)]);
        let c = plan_set(&a, &live, 0, None).unwrap();
        assert_eq!(c.action, ScaleAction::Down);
        assert!(c.pause);
        assert_eq!((c.before.as_str(), c.after.as_str()), ("2 Pods", "0 Pods"));
        assert_eq!(
            c.state_value.as_deref(),
            Some(
                "{\"autoSync\": {\"prune\": true, \"selfHeal\": true}, \"replicas\": {\"Deployment/web/web\": 2}}"
            )
        );
        assert_eq!(plan_set(&a, &live, 2, None).unwrap_err(), "bereits 2 Pods");
        let b = app(
            sample(),
            &[("web.scale", c.state_value.as_deref().unwrap())],
        );
        let up = plan_up(&b).unwrap();
        assert_eq!(up.after, "2 Pods");
        assert!(up.resume.is_none());
        assert_eq!(up.ops[0].0.target(), "deployments.apps/web");
    }

    #[test]
    fn jwt_and_token_state() {
        let payload = "eyJleHAiOjIwMDAwMDAwMDB9";
        let token = format!("h.{payload}.s");
        assert_eq!(jwt_exp(&token), Some(Some(2_000_000_000)));
        let cfg: serde_yaml_ng::Value = serde_yaml_ng::from_str(&format!(
            "contexts:\n- name: hs\n  user: u\nusers:\n- name: u\n  auth-token: {token}\n"
        ))
        .unwrap();
        assert_eq!(
            token_status(&cfg, "hs", 0),
            TokenState::Valid(Some(2_000_000_000))
        );
        assert_eq!(
            token_status(&cfg, "hs", 2_100_000_000),
            TokenState::Expired(2_000_000_000)
        );
        assert_eq!(token_status(&cfg, "x", 0), TokenState::None);
    }
}
