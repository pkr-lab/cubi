use std::time::Duration;

use eframe::egui::Ui;
use serde_json::Value;

use crate::shell;
use crate::timeutil::helm_updated_age;
use crate::ui::{self, Cell, Cx, OutputWindow, TableSpec, Task, Tone};

#[derive(Clone)]
pub struct Release {
    pub name: String,
    pub namespace: String,
    pub revision: String,
    pub status: String,
    pub chart: String,
    pub app_version: String,
    pub updated: String,
}

pub fn status_tone(s: &str) -> Tone {
    match s {
        "deployed" => Tone::Ok,
        "failed" | "unknown" => Tone::Bad,
        "superseded" | "uninstalled" => Tone::Muted,
        "uninstalling" | "pending-install" | "pending-upgrade" | "pending-rollback" => Tone::Warn,
        _ => Tone::Plain,
    }
}

pub fn parse(data: &Value) -> Vec<Release> {
    let g = |v: &Value, k: &str| match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => "-".into(),
    };
    let mut out: Vec<Release> = data
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| Release {
            name: g(r, "name"),
            namespace: g(r, "namespace"),
            revision: g(r, "revision"),
            status: g(r, "status"),
            chart: g(r, "chart"),
            app_version: g(r, "app_version"),
            updated: g(r, "updated"),
        })
        .collect();
    out.sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
    out
}

#[derive(Default)]
pub struct Helm {
    list: Task<Vec<Release>>,
    selected: Option<usize>,
    out: Option<OutputWindow>,
}

impl super::View for Helm {
    fn id(&self) -> &'static str {
        "helm"
    }
    fn title(&self) -> &'static str {
        "Helm"
    }
    fn icon(&self) -> &'static str {
        "⎈"
    }
    fn hint(&self) -> &'static str {
        "Releases mit Status, Values und Historie"
    }

    fn ui(&mut self, ui: &mut Ui, cx: &Cx) {
        let c = cx.clone();
        ui::auto_refresh(ui, &mut self.list, Duration::from_secs(30), |t| {
            t.spawn(&c, async {
                Ok(parse(
                    &shell::capture_json(&["helm", "list", "-A", "-o", "json", "--max", "500"], 30)
                        .await?,
                ))
            })
        });
        let n = self.list.value().map_or(0, Vec::len);
        ui::page_header(
            ui,
            "Helm",
            &format!("{n} Releases · aktualisiert alle 30 s"),
            |ui| {
                if ui.button("⟳ Neu laden").clicked() {
                    self.list.at = None;
                }
            },
        );
        let sel = self
            .selected
            .and_then(|i| self.list.value().and_then(|l| l.get(i)).cloned());
        if let Some(r) = &sel {
            ui::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(eframe::egui::RichText::new(&r.name).strong());
                    ui.label(eframe::egui::RichText::new(&r.namespace).color(ui::theme::MUTED));
                    ui::pill(ui, &r.status, status_tone(&r.status));
                    ui.with_layout(
                        eframe::egui::Layout::right_to_left(eframe::egui::Align::Center),
                        |ui| {
                            for (label, args, empty) in [
                                ("Historie", vec!["history"], ""),
                                ("Values", vec!["get", "values"], "Keine eigenen Values"),
                                ("Status", vec!["status"], ""),
                            ] {
                                if ui.button(label).clicked() {
                                    let mut argv: Vec<String> = vec!["helm".into()];
                                    argv.extend(args.iter().map(|s| s.to_string()));
                                    argv.extend([r.name.clone(), "-n".into(), r.namespace.clone()]);
                                    self.out = Some(OutputWindow::new(
                                        cx,
                                        format!("{label} · {}", r.name),
                                        empty,
                                        argv,
                                        60,
                                    ));
                                }
                            }
                        },
                    );
                });
            });
            ui.add_space(6.0);
        }
        if ui::task_status(ui, &self.list, "Lade Releases …") {
            let rows: Vec<Vec<Cell>> = self
                .list
                .value()
                .unwrap()
                .iter()
                .map(|r| {
                    vec![
                        Cell::tone(&r.name, Tone::Plain),
                        Cell::muted(&r.namespace),
                        Cell::muted(&r.revision),
                        Cell::pill(&r.status, status_tone(&r.status)),
                        Cell::plain(&r.chart),
                        Cell::muted(&r.app_version),
                        Cell::muted(helm_updated_age(&r.updated)),
                    ]
                })
                .collect();
            ui::card(ui, |ui| {
                ui::table(
                    ui,
                    TableSpec {
                        id: "helm",
                        headers: &[
                            "Release",
                            "Namespace",
                            "Rev",
                            "Status",
                            "Chart",
                            "App-Version",
                            "Alter",
                        ],
                        checks: None,
                    },
                    &rows,
                    &mut self.selected,
                );
            });
        }
        ui::show_output(ui.ctx(), &mut self.out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_sorts() {
        let v: Value = serde_json::from_str(r#"[{"name":"b","namespace":"x","revision":3,"status":"failed"},{"name":"a","namespace":"x"}]"#).unwrap();
        let r = parse(&v);
        assert_eq!(r[0].name, "a");
        assert_eq!(r[1].revision, "3");
        assert_eq!(status_tone(&r[1].status), Tone::Bad);
    }
}
