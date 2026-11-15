use eframe::egui::{self, RichText, Ui};
use serde_json::Value;

use crate::argo::{self, Context, TokenState};
use crate::shell;
use crate::ui::{self, Cx, OutputWindow, Task, Tone};

#[derive(Clone)]
pub struct Row {
    pub service: &'static str,
    pub target: String,
    pub ok: bool,
    pub status: String,
    pub context: Option<Context>,
}

fn fmt_ts(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

pub fn argocd_row(ctx: &Context, cfg: &serde_yaml_ng::Value, now: i64) -> Row {
    let (ok, status) = if ctx.core {
        (true, "Core-Modus, kein Login nötig".to_string())
    } else {
        match argo::token_status(cfg, &ctx.name, now) {
            TokenState::Valid(Some(t)) => (true, format!("gültig bis {}", fmt_ts(t))),
            TokenState::Valid(None) => (true, "eingeloggt".into()),
            TokenState::Expired(t) => (false, format!("abgelaufen seit {}", fmt_ts(t))),
            TokenState::Invalid => (false, "Token ungültig".into()),
            TokenState::None => (false, "nicht eingeloggt".into()),
        }
    };
    Row {
        service: "ArgoCD",
        target: ctx.name.clone(),
        ok,
        status,
        context: Some(ctx.clone()),
    }
}

pub fn gh_account(text: &str) -> String {
    let i = text.find("account ").map(|i| i + 8);
    i.and_then(|i| text[i..].split_whitespace().next())
        .unwrap_or("unbekannt")
        .to_string()
}

async fn github_row() -> Row {
    let mut r = Row {
        service: "GitHub",
        target: "github.com".into(),
        ok: false,
        status: String::new(),
        context: None,
    };
    match shell::capture(&["gh", "auth", "status", "-h", "github.com"], 15).await {
        Err(e) => r.status = format!("{e:#}"),
        Ok(o) if !o.ok() => r.status = "nicht eingeloggt".into(),
        Ok(o) => {
            r.ok = true;
            r.status = format!("eingeloggt als {}", gh_account(&(o.stdout + &o.stderr)));
        }
    }
    r
}

pub fn tailscale_status(data: &Value) -> (String, bool, String) {
    let state = data
        .get("BackendState")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let tailnet = data
        .pointer("/CurrentTailnet/Name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("Tailnet")
        .to_string();
    let (ok, s) = match state {
        "Running" => (true, "verbunden".to_string()),
        "NeedsLogin" => (false, "Login nötig".into()),
        "Stopped" => (false, "gestoppt".into()),
        "" => (false, "unbekannt".into()),
        o => (false, o.into()),
    };
    (tailnet, ok, s)
}

async fn tailscale_row() -> Row {
    let mut r = Row {
        service: "Tailscale",
        target: "Tailnet".into(),
        ok: false,
        status: String::new(),
        context: None,
    };
    match shell::capture(&["tailscale", "status", "--json"], 15).await {
        Err(e) => r.status = format!("{e:#}"),
        Ok(o) => match serde_json::from_str::<Value>(&o.stdout) {
            Ok(v) => {
                let (t, ok, s) = tailscale_status(&v);
                (r.target, r.ok, r.status) = (t, ok, s);
            }
            Err(_) => {
                let l = shell::first_line(&o.stderr);
                r.status = if l.is_empty() {
                    "Status nicht lesbar".into()
                } else {
                    l
                };
            }
        },
    }
    r
}

#[derive(Default)]
pub struct Login {
    rows: Task<Vec<Row>>,
    out: Option<OutputWindow>,
    error: Option<String>,
}

impl super::View for Login {
    fn id(&self) -> &'static str {
        "login"
    }
    fn title(&self) -> &'static str {
        "Login"
    }
    fn icon(&self) -> &'static str {
        "🔑"
    }
    fn hint(&self) -> &'static str {
        "ArgoCD, GitHub, Tailscale"
    }

    fn ui(&mut self, ui: &mut Ui, cx: &Cx) {
        self.rows.poll();
        if self.rows.result.is_none() && !self.rows.busy() {
            let contexts = cx
                .cubi()
                .map_err(anyhow::Error::msg)
                .and_then(argo::configured_contexts);
            self.rows.spawn(cx, async move {
                let cfg = argo::read_argocd_config();
                let now = chrono::Utc::now().timestamp();
                let mut rows: Vec<Row> =
                    contexts?.iter().map(|c| argocd_row(c, &cfg, now)).collect();
                let (g, t) = tokio::join!(github_row(), tailscale_row());
                rows.push(g);
                rows.push(t);
                Ok(rows)
            });
        }
        ui::page_header(
            ui,
            "Login",
            "Anmeldestatus der Werkzeuge. Logins laufen im Terminal.",
            |ui| {
                if ui.button("⟳ Neu prüfen").clicked() {
                    self.rows.clear();
                }
            },
        );
        if let Some(e) = &self.error {
            ui::error_box(ui, e);
        }
        if !ui::task_status(ui, &self.rows, "Prüfe Anmeldungen …") {
            ui::show_output(ui.ctx(), &mut self.out);
            return;
        }
        let rows = self.rows.value().cloned().unwrap_or_default();
        let active = argo::current_context();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for r in &rows {
                    ui::card(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(r.service)
                                    .strong()
                                    .size(15.0)
                                    .color(ui::theme::STRONG),
                            );
                            ui.label(RichText::new(&r.target).color(ui::theme::MUTED));
                            if r.context.as_ref().is_some_and(|c| c.name == active) {
                                ui::pill(ui, "aktiv", Tone::Accent);
                            }
                            ui::pill(
                                ui,
                                format!("{} {}", if r.ok { "✔" } else { "✖" }, r.status),
                                if r.ok { Tone::Ok } else { Tone::Bad },
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| match r.service {
                                    "ArgoCD" => {
                                        let c = r.context.clone().unwrap();
                                        if ui.button("Kontext aktivieren").clicked() {
                                            let name = c.name.clone();
                                            self.out = Some(OutputWindow::from_future(
                                                cx,
                                                format!("ArgoCD-Kontext {name}"),
                                                async move {
                                                    argo::activate(&name).await?;
                                                    Ok(format!("ArgoCD-Kontext {name} ist aktiv."))
                                                },
                                            ));
                                            self.rows.clear();
                                        }
                                        if !c.core
                                            && ui
                                                .add(ui::primary_button("Login mit Passwort"))
                                                .clicked()
                                        {
                                            let mut cmd = vec![
                                                "argocd".to_string(),
                                                "login".into(),
                                                c.server.clone(),
                                            ];
                                            if c.plaintext {
                                                cmd.push("--plaintext".into());
                                            }
                                            if c.insecure {
                                                cmd.push("--insecure".into());
                                            }
                                            if c.server != c.name {
                                                cmd.extend(["--name".into(), c.name.clone()]);
                                            }
                                            if let Some(u) = &c.username {
                                                cmd.extend(["--username".into(), u.clone()]);
                                            }
                                            self.error = cx.terminal(cmd).err();
                                        }
                                    }
                                    "GitHub" => {
                                        for (label, extra) in [
                                            (
                                                "Projekt-Scope (Kanban)",
                                                vec![
                                                    "refresh",
                                                    "-h",
                                                    "github.com",
                                                    "-s",
                                                    "project,read:project",
                                                ],
                                            ),
                                            (
                                                "Scopes erneuern",
                                                vec!["refresh", "-h", "github.com"],
                                            ),
                                            ("Login", vec!["login", "-h", "github.com"]),
                                        ] {
                                            if ui.button(label).clicked() {
                                                let mut cmd = vec!["gh".to_string(), "auth".into()];
                                                cmd.extend(extra.iter().map(|s| s.to_string()));
                                                self.error = cx.terminal(cmd).err();
                                            }
                                        }
                                    }
                                    _ => {
                                        for sub in ["login", "up"] {
                                            if ui.button(format!("tailscale {sub}")).clicked() {
                                                self.error = cx
                                                    .terminal(vec!["tailscale".into(), sub.into()])
                                                    .err();
                                            }
                                        }
                                    }
                                },
                            );
                        });
                    });
                    ui.add_space(6.0);
                }
                ui.label(
                    RichText::new(
                        "Bei Berechtigungsfehlern von tailscale den Befehl mit sudo wiederholen.",
                    )
                    .small()
                    .color(ui::theme::MUTED),
                );
            });
        ui::show_output(ui.ctx(), &mut self.out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsing() {
        assert_eq!(
            gh_account("✓ Logged in to github.com account pkr-lab (keyring)"),
            "pkr-lab"
        );
        assert_eq!(gh_account("nichts"), "unbekannt");
        let v: Value = serde_json::from_str(
            r#"{"BackendState":"NeedsLogin","CurrentTailnet":{"Name":"t.ts.net"}}"#,
        )
        .unwrap();
        assert_eq!(
            tailscale_status(&v),
            ("t.ts.net".into(), false, "Login nötig".into())
        );
        let core = Context {
            name: "k".into(),
            server: "k".into(),
            core: true,
            plaintext: false,
            insecure: false,
            username: None,
        };
        assert!(argocd_row(&core, &serde_yaml_ng::Value::Null, 0).ok);
    }
}
