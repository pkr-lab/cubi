use eframe::egui::{self, RichText, Ui};

use crate::cubi_config::as_str;
use crate::shell;
use crate::ui::{self, Cx, Task, Tone};

#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub name: String,
    pub ssh: String,
    pub detail: String,
}

pub fn group_title(key: &str) -> String {
    if key.chars().count() <= 3 {
        key.to_uppercase()
    } else {
        let mut c = key.chars();
        c.next()
            .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
            .unwrap_or_default()
    }
}

pub fn groups(section: &serde_yaml_ng::Mapping) -> Result<Vec<(String, Vec<Target>)>, String> {
    if section.is_empty() {
        return Err("Keine Connect-Ziele in targets.yaml".into());
    }
    let mut out = Vec::new();
    for (k, v) in section {
        let key = as_str(k).unwrap_or_default();
        let Some(entries) = v.as_sequence().filter(|s| !s.is_empty()) else {
            return Err(format!("Keine Ziele in connect.{key}"));
        };
        let mut targets = Vec::new();
        for e in entries {
            match (
                e.get("name").and_then(as_str),
                e.get("ssh").and_then(as_str),
            ) {
                (Some(name), Some(ssh)) => targets.push(Target {
                    name,
                    ssh,
                    detail: String::new(),
                }),
                _ => {
                    return Err(format!(
                        "Ungültiger Eintrag in connect.{key}: name und ssh sind Pflicht"
                    ));
                }
            }
        }
        out.push((key, targets));
    }
    Ok(out)
}

pub fn describe_from(ssh_g: &str) -> String {
    let mut host = None;
    let mut user = None;
    for line in ssh_g.lines() {
        let (k, v) = line.split_once(' ').unwrap_or((line, ""));
        if k == "hostname" && host.is_none() {
            host = Some(v.to_string());
        }
        if k == "user" && user.is_none() {
            user = Some(v.to_string());
        }
    }
    match (user, host) {
        (Some(u), Some(h)) if !u.is_empty() && !h.is_empty() => format!("{u}@{h}"),
        (_, Some(h)) => h,
        _ => String::new(),
    }
}

async fn describe(alias: String) -> String {
    match shell::capture(&["ssh", "-G", &alias], 5).await {
        Ok(o) if o.ok() => describe_from(&o.stdout),
        _ => String::new(),
    }
}

#[derive(Default)]
pub struct Connect {
    data: Task<Vec<(String, Vec<Target>)>>,
    error: Option<String>,
}

impl super::View for Connect {
    fn id(&self) -> &'static str {
        "connect"
    }
    fn title(&self) -> &'static str {
        "Connect"
    }
    fn icon(&self) -> &'static str {
        "🖧"
    }
    fn hint(&self) -> &'static str {
        "SSH zu VMs und Hosts"
    }

    fn ui(&mut self, ui: &mut Ui, cx: &Cx) {
        self.data.poll();
        if self.data.result.is_none() && !self.data.busy() {
            let section = cx.cubi().map(|c| c.section("connect"));
            self.data.spawn(cx, async move {
                let section = section.map_err(anyhow::Error::msg)?;
                let mut gs = groups(&section).map_err(anyhow::Error::msg)?;
                for (_, ts) in &mut gs {
                    for t in ts.iter_mut() {
                        t.detail = describe(t.ssh.clone()).await;
                    }
                }
                Ok(gs)
            });
        }
        ui::page_header(ui, "Connect", "SSH-Verbindung im Terminal öffnen", |ui| {
            if ui.button("⟳ Neu laden").clicked() {
                self.data.clear();
            }
        });
        if let Some(e) = &self.error {
            ui::error_box(ui, e);
        }
        if !ui::task_status(ui, &self.data, "Lese SSH-Ziele …") {
            return;
        }
        let groups = self.data.value().cloned().unwrap_or_default();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (key, targets) in &groups {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui::section_title(ui, &group_title(key));
                        ui::pill(ui, format!("{} Ziele", targets.len()), Tone::Muted);
                    });
                    ui.add_space(4.0);
                    let per_row = ((ui.available_width() + 8.0) / 258.0).floor().max(1.0) as usize;
                    for chunk in targets.chunks(per_row) {
                        ui.horizontal(|ui| {
                            for t in chunk {
                                ui::card(ui, |ui| {
                                    ui.set_width(222.0);
                                    ui.vertical(|ui| {
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(&t.name)
                                                    .strong()
                                                    .size(15.0)
                                                    .color(ui::theme::STRONG),
                                            )
                                            .truncate(),
                                        );
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(if t.detail.is_empty() {
                                                    &t.ssh
                                                } else {
                                                    &t.detail
                                                })
                                                .monospace()
                                                .color(ui::theme::MUTED),
                                            )
                                            .truncate(),
                                        );
                                        ui.add_space(4.0);
                                        if ui.add(ui::primary_button("⏵ SSH verbinden")).clicked()
                                        {
                                            self.error = cx
                                                .terminal(vec!["ssh".into(), t.ssh.clone()])
                                                .err();
                                        }
                                    });
                                });
                            }
                        });
                        ui.add_space(4.0);
                    }
                    ui.add_space(10.0);
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_and_describe() {
        let m: serde_yaml_ng::Mapping = serde_yaml_ng::from_str(
            "vm:\n  - name: a\n    ssh: b\nhosts:\n  - name: c\n    ssh: d\n",
        )
        .unwrap();
        let g = groups(&m).unwrap();
        assert_eq!(group_title(&g[0].0), "VM");
        assert_eq!(group_title(&g[1].0), "Hosts");
        let bad: serde_yaml_ng::Mapping = serde_yaml_ng::from_str("vm:\n  - name: a\n").unwrap();
        assert!(groups(&bad).is_err());
        assert_eq!(
            describe_from("user pi\nhostname 10.0.0.1\nhostname x\n"),
            "pi@10.0.0.1"
        );
    }
}
