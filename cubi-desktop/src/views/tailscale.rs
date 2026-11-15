use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::bail;
use eframe::egui::{self, RichText, Ui};
use serde_json::Value;

use crate::shell;
use crate::timeutil::age;
use crate::ui::{self, Cell, Cx, OutputWindow, TableSpec, Task, Tone};

#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub name: String,
    pub ip: String,
    pub os: String,
    pub online: bool,
    pub last_seen: String,
    pub is_self: bool,
}

fn device(e: &Value, is_self: bool) -> Device {
    let gs = |k: &str| {
        e.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let name = Some(gs("HostName"))
        .filter(|s| !s.is_empty())
        .or_else(|| Some(gs("DNSName")).filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "?".into());
    Device {
        name,
        ip: e
            .get("TailscaleIPs")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_str)
            .unwrap_or("-")
            .into(),
        os: Some(gs("OS"))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "-".into()),
        online: e.get("Online").and_then(Value::as_bool).unwrap_or(false) || is_self,
        last_seen: gs("LastSeen"),
        is_self,
    }
}

pub fn parse(data: &Value) -> anyhow::Result<Vec<Device>> {
    let state = data
        .get("BackendState")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if state != "Running" {
        bail!(
            "Tailscale ist nicht verbunden (Status: {}). Mit „Up“ starten oder unter Login anmelden.",
            if state.is_empty() { "unbekannt" } else { state }
        );
    }
    let mut devices = vec![device(data.get("Self").unwrap_or(&Value::Null), true)];
    let mut peers: Vec<Device> = data
        .get("Peer")
        .and_then(Value::as_object)
        .map(|m| m.values().map(|p| device(p, false)).collect())
        .unwrap_or_default();
    peers.sort_by_key(|d| (!d.online, d.name.to_lowercase()));
    devices.extend(peers);
    Ok(devices)
}

pub fn ssh_aliases() -> BTreeSet<String> {
    let path = dirs::home_dir().unwrap_or_default().join(".ssh/config");
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeSet::new();
    };
    let mut out = BTreeSet::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() > 1 && parts[0].eq_ignore_ascii_case("host") {
            out.extend(
                parts[1..]
                    .iter()
                    .filter(|p| !p.contains('*') && !p.contains('?'))
                    .map(|p| p.to_string()),
            );
        }
    }
    out
}

async fn fetch() -> anyhow::Result<Vec<Device>> {
    let out = shell::capture(&["tailscale", "status", "--json"], 15).await?;
    match serde_json::from_str::<Value>(&out.stdout) {
        Ok(v) => parse(&v),
        Err(_) => {
            let l = shell::first_line(&out.stderr);
            bail!(
                "{}",
                if l.is_empty() {
                    "Tailscale-Status nicht lesbar".into()
                } else {
                    l
                }
            )
        }
    }
}

#[derive(Default)]
pub struct Tailscale {
    list: Task<Vec<Device>>,
    selected: Option<usize>,
    out: Option<OutputWindow>,
    confirm_down: bool,
    error: Option<String>,
}

impl super::View for Tailscale {
    fn id(&self) -> &'static str {
        "tailscale"
    }
    fn title(&self) -> &'static str {
        "Tailscale"
    }
    fn icon(&self) -> &'static str {
        "🔗"
    }
    fn hint(&self) -> &'static str {
        "Geräte, Ping, SSH, Up/Down"
    }

    fn ui(&mut self, ui: &mut Ui, cx: &Cx) {
        let c = cx.clone();
        ui::auto_refresh(ui, &mut self.list, Duration::from_secs(15), |t| {
            t.spawn(&c, fetch())
        });
        let devices = self.list.value().cloned().unwrap_or_default();
        let online = devices.iter().filter(|d| d.online).count();
        ui::page_header(
            ui,
            "Tailscale",
            "Geräte im Tailnet · aktualisiert alle 15 s",
            |ui| {
                if ui.button("⟳ Neu laden").clicked() {
                    self.list.at = None;
                }
                if ui.button("Status roh").clicked() {
                    self.out = Some(OutputWindow::new(
                        cx,
                        "tailscale status",
                        "",
                        vec!["tailscale".into(), "status".into()],
                        15,
                    ));
                }
                if ui.button("Down").clicked() {
                    self.confirm_down = true;
                }
                if ui.button("Up").clicked() {
                    self.error = cx.terminal(vec!["tailscale".into(), "up".into()]).err();
                }
            },
        );
        if !devices.is_empty() {
            ui.horizontal(|ui| {
                ui::stat(ui, "Geräte", devices.len().to_string(), Tone::Plain);
                ui::stat(ui, "Online", online.to_string(), Tone::Ok);
                ui::stat(
                    ui,
                    "Offline",
                    (devices.len() - online).to_string(),
                    if devices.len() > online {
                        Tone::Bad
                    } else {
                        Tone::Muted
                    },
                );
            });
            ui.add_space(8.0);
        }
        if let Some(e) = &self.error {
            ui::error_box(ui, e);
        }
        let sel = self.selected.and_then(|i| devices.get(i)).cloned();
        if let Some(d) = sel.filter(|d| !d.is_self) {
            ui::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&d.name).strong());
                    ui.label(RichText::new(&d.ip).monospace().color(ui::theme::MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Ping").clicked() {
                            self.out = Some(OutputWindow::new(
                                cx,
                                format!("Ping · {}", d.name),
                                "",
                                vec![
                                    "tailscale".into(),
                                    "ping".into(),
                                    "-c".into(),
                                    "3".into(),
                                    d.ip.clone(),
                                ],
                                30,
                            ));
                        }
                        if ui.add(ui::primary_button("SSH")).clicked() {
                            let target = if ssh_aliases().contains(&d.name) {
                                d.name.clone()
                            } else {
                                d.ip.clone()
                            };
                            self.error = cx.terminal(vec!["ssh".into(), target]).err();
                        }
                    });
                });
            });
            ui.add_space(6.0);
        }
        if ui::task_status(ui, &self.list, "Lese tailscale status …") {
            let rows: Vec<Vec<Cell>> = devices
                .iter()
                .map(|d| {
                    let status = if d.is_self {
                        Cell::pill("dieses Gerät", Tone::Info)
                    } else if d.online {
                        Cell::pill("online", Tone::Ok)
                    } else {
                        Cell::pill(format!("offline · {}", age(&d.last_seen)), Tone::Bad)
                    };
                    vec![
                        Cell::plain(&d.name),
                        Cell::mono(&d.ip, Tone::Muted),
                        Cell::muted(&d.os),
                        status,
                    ]
                })
                .collect();
            ui::card(ui, |ui| {
                ui::table(
                    ui,
                    TableSpec {
                        id: "ts",
                        headers: &["Gerät", "IP", "OS", "Status"],
                        checks: None,
                    },
                    &rows,
                    &mut self.selected,
                );
            });
        }
        if self.confirm_down {
            match ui::confirm(
                ui.ctx(),
                "ts-down",
                "Tailscale trennen?",
                &["Die Verbindung zum Tailnet wird beendet.".into()],
                "Trennen",
                true,
            ) {
                Some(true) => {
                    self.confirm_down = false;
                    self.out = Some(OutputWindow::new(
                        cx,
                        "tailscale down",
                        "Getrennt. Bei Berechtigungsfehlern sudo tailscale down ausführen.",
                        vec!["tailscale".into(), "down".into()],
                        30,
                    ));
                    self.list.at = None;
                }
                Some(false) => self.confirm_down = false,
                None => {}
            }
        }
        ui::show_output(ui.ctx(), &mut self.out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status() {
        let v: Value = serde_json::from_str(r#"{"BackendState":"Running","Self":{"HostName":"me","TailscaleIPs":["100.1.1.1"]},"Peer":{"a":{"HostName":"zz","Online":true},"b":{"HostName":"aa","Online":false}}}"#).unwrap();
        let d = parse(&v).unwrap();
        assert!(d[0].is_self && d[0].online);
        assert_eq!(d[1].name, "zz");
        assert_eq!(d[2].name, "aa");
        let v: Value = serde_json::from_str(r#"{"BackendState":"Stopped"}"#).unwrap();
        assert!(parse(&v).unwrap_err().to_string().contains("Stopped"));
    }
}
