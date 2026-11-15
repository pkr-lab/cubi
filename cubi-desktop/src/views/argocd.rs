use std::collections::BTreeMap;
use std::time::Duration;

use eframe::egui::{self, RichText, Ui};
use serde_json::Value;

use crate::argo::{self, App, BranchChange, ScaleAction, ScaleChange, SyncAction, SyncChange};
use crate::shell;
use crate::timeutil::{age, parse_iso};
use crate::ui::{self, Cell, Cx, OutputWindow, TableSpec, Task, Tone};
use crate::views::kube::sv;

#[derive(PartialEq, Eq, Clone, Copy, Default, Debug)]
pub enum Tab {
    #[default]
    Apps,
    AutoSync,
    Feature,
    Scale,
}

#[derive(PartialEq, Eq, Clone, Default)]
enum Filter {
    #[default]
    All,
    Problems,
    Project(String),
}

type Report = Vec<(Tone, String)>;
type CommitView = (
    String,
    String,
    String,
    Task<Vec<(String, String, String, String)>>,
);

enum Plan {
    Sync(Vec<SyncChange>),
    Branch(Vec<BranchChange>),
    Scale(Vec<ScaleChange>),
}

#[derive(Default)]
struct OpState {
    checks: Vec<bool>,
    plan: Task<(Plan, Vec<(String, String)>)>,
    apply: Task<Report>,
    confirm: bool,
}

#[derive(Default)]
pub struct ArgoCd {
    pub tab: Tab,
    apps: Task<Vec<App>>,
    filter: Filter,
    selected: Option<usize>,
    out: Option<OutputWindow>,
    confirm_sync: Option<Vec<String>>,
    multi: bool,
    multi_checks: Vec<bool>,
    context_op: Task<()>,
    commits: Option<CommitView>,
    error: Option<String>,
    sync_action: Option<SyncAction>,
    branch_reset: bool,
    branch_target: String,
    source_pick: BTreeMap<String, usize>,
    scale_action: Option<ScaleAction>,
    scale_count: String,
    scale_live: Task<BTreeMap<String, i64>>,
    scale_only: BTreeMap<String, bool>,
    op: OpState,
}

fn ns(cx: &Cx) -> String {
    argo::namespace(cx.cubi().ok())
}

fn matches(app: &App, f: &Filter) -> bool {
    match f {
        Filter::All => true,
        Filter::Problems => argo::is_problem(app),
        Filter::Project(p) => app.project == *p,
    }
}

impl super::View for ArgoCd {
    fn id(&self) -> &'static str {
        "argocd"
    }
    fn title(&self) -> &'static str {
        "ArgoCD"
    }
    fn icon(&self) -> &'static str {
        "⟳"
    }
    fn hint(&self) -> &'static str {
        "Apps, Sync, Auto-Sync, Feature-Testing, Skalieren"
    }

    fn open(&mut self, arg: &str) {
        if let Some(t) = super::argocd_tab(arg) {
            self.set_tab(t);
        }
    }

    fn ui(&mut self, ui: &mut Ui, cx: &Cx) {
        if self.context_op.poll() {
            self.error = self.context_op.error().map(str::to_string);
            self.apps.at = None;
        }
        let namespace = ns(cx);
        let with_state = self.tab != Tab::Apps;
        let c = cx.clone();
        let n2 = namespace.clone();
        let interval = if self.tab == Tab::Apps { 10 } else { 3600 };
        ui::auto_refresh(ui, &mut self.apps, Duration::from_secs(interval), |t| {
            t.spawn(&c, async move {
                let mut a = argo::fetch_apps(&n2, with_state).await?;
                a.sort_by(|x, y| x.name.cmp(&y.name));
                Ok(a)
            })
        });
        let current = argo::current_context();
        ui::page_header(
            ui,
            "ArgoCD",
            &format!("Kontext {current} · Namespace {namespace}"),
            |ui| {
                if ui.button("⟳ Neu laden").clicked() {
                    self.apps.at = None;
                }
                let contexts = cx
                    .cubi()
                    .ok()
                    .and_then(|c| argo::configured_contexts(c).ok())
                    .unwrap_or_default();
                let mut pick = None;
                egui::ComboBox::from_id_salt("argoctx")
                    .selected_text(format!("Kontext: {current}"))
                    .show_ui(ui, |ui| {
                        for c in &contexts {
                            if ui.selectable_label(c.name == current, &c.name).clicked() {
                                pick = Some(c.name.clone());
                            }
                        }
                    });
                if let Some(p) = pick {
                    self.context_op
                        .spawn(cx, async move { argo::activate(&p).await });
                }
            },
        );
        ui.horizontal(|ui| {
            for (t, l) in [
                (Tab::Apps, "Apps"),
                (Tab::AutoSync, "Auto-Sync"),
                (Tab::Feature, "Feature-Testing"),
                (Tab::Scale, "Pods skalieren"),
            ] {
                if ui
                    .selectable_label(self.tab == t, RichText::new(l).strong())
                    .clicked()
                    && self.tab != t
                {
                    self.set_tab(t);
                }
            }
        });
        ui.separator();
        if let Some(e) = &self.error {
            ui::error_box(ui, e);
        }
        match self.tab {
            Tab::Apps => self.apps_tab(ui, cx),
            Tab::AutoSync => self.autosync_tab(ui, cx),
            Tab::Feature => self.feature_tab(ui, cx),
            Tab::Scale => self.scale_tab(ui, cx),
        }
        self.commits_window(ui.ctx());
        ui::show_output(ui.ctx(), &mut self.out);
    }
}

impl ArgoCd {
    pub fn set_tab(&mut self, t: Tab) {
        self.tab = t;
        self.apps.clear();
        self.op = OpState::default();
        self.selected = None;
    }

    fn cli(&mut self, cx: &Cx, title: String, args: Vec<String>, empty: &'static str) {
        self.out = Some(OutputWindow::from_future(cx, title, async move {
            let mut argv = vec!["argocd".to_string()];
            argv.extend(args);
            let out = shell::capture(&argv, 90).await?;
            let mut text = shell::merge_output(&out);
            if !out.ok()
                && argo::AUTH_HINTS
                    .iter()
                    .any(|h| out.stderr.to_lowercase().contains(h))
            {
                text += "\n\nHinweis: Login abgelaufen oder fehlt. Unter Login erneuern.";
            }
            Ok(if text.is_empty() { empty.into() } else { text })
        }));
    }

    fn apps_tab(&mut self, ui: &mut Ui, cx: &Cx) {
        let apps = self.apps.value().cloned().unwrap_or_default();
        let mut visible: Vec<App> = apps
            .iter()
            .filter(|a| matches(a, &self.filter))
            .cloned()
            .collect();
        visible.sort_by_key(argo::rank);
        if !apps.is_empty() {
            let count = |f: &dyn Fn(&App) -> bool| apps.iter().filter(|a| f(a)).count();
            ui.horizontal(|ui| {
                ui::stat(ui, "Apps", apps.len().to_string(), Tone::Plain);
                ui::stat(
                    ui,
                    "Synced",
                    count(&|a| a.sync == "Synced").to_string(),
                    Tone::Ok,
                );
                let oos = count(&|a| a.sync == "OutOfSync");
                ui::stat(
                    ui,
                    "OutOfSync",
                    oos.to_string(),
                    if oos > 0 { Tone::Warn } else { Tone::Muted },
                );
                ui::stat(
                    ui,
                    "Healthy",
                    count(&|a| a.health == "Healthy").to_string(),
                    Tone::Ok,
                );
                let deg = count(&|a| matches!(a.health.as_str(), "Degraded" | "Missing"));
                ui::stat(
                    ui,
                    "Degraded/Missing",
                    deg.to_string(),
                    if deg > 0 { Tone::Bad } else { Tone::Muted },
                );
                let prog = count(&|a| a.health == "Progressing");
                ui::stat(
                    ui,
                    "Progressing",
                    prog.to_string(),
                    if prog > 0 { Tone::Warn } else { Tone::Muted },
                );
            });
            ui.add_space(6.0);
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("Filter").color(ui::theme::MUTED));
            let label = match &self.filter {
                Filter::All => "Alle Apps".to_string(),
                Filter::Problems => "Nur Probleme".into(),
                Filter::Project(p) => format!("Projekt: {p}"),
            };
            let mut projects: Vec<String> = apps.iter().map(|a| a.project.clone()).collect();
            projects.sort();
            projects.dedup();
            egui::ComboBox::from_id_salt("argofilter")
                .selected_text(label)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.filter, Filter::All, "Alle Apps");
                    ui.selectable_value(&mut self.filter, Filter::Problems, "Nur Probleme");
                    for p in projects {
                        ui.selectable_value(
                            &mut self.filter,
                            Filter::Project(p.clone()),
                            format!("Projekt: {p}"),
                        );
                    }
                });
            if self.filter != Filter::All {
                ui::pill(ui, format!("{} sichtbar", visible.len()), Tone::Accent);
            }
            ui.separator();
            if ui.toggle_value(&mut self.multi, "Mehrere syncen").changed() && self.multi {
                self.multi_checks = visible.iter().map(|a| a.sync == "OutOfSync").collect();
            }
            if self.multi {
                let names: Vec<String> = visible
                    .iter()
                    .zip(&self.multi_checks)
                    .filter(|(_, c)| **c)
                    .map(|(a, _)| a.name.clone())
                    .collect();
                if ui
                    .add_enabled(
                        !names.is_empty(),
                        ui::primary_button(format!("{} App(s) synchronisieren", names.len())),
                    )
                    .clicked()
                {
                    self.confirm_sync = Some(names);
                }
            }
        });
        ui.add_space(4.0);
        if self.multi_checks.len() != visible.len() {
            self.multi_checks = visible.iter().map(|a| a.sync == "OutOfSync").collect();
        }
        let sel = self.selected.and_then(|i| visible.get(i)).cloned();
        egui::Panel::right("argodetail")
            .resizable(true)
            .min_size(340.0)
            .default_size(430.0)
            .show_collapsible(ui, &mut (sel.is_some() && !self.multi), |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if let Some(a) = &sel {
                        self.detail(ui, cx, a);
                    }
                });
            });
        egui::CentralPanel::no_frame().show(ui, |ui| {
            if ui::task_status(ui, &self.apps, "Lade ArgoCD-Apps …") {
                let rows: Vec<Vec<Cell>> = visible
                    .iter()
                    .map(|a| {
                        vec![
                            Cell::plain(&a.name),
                            Cell::muted(&a.project),
                            Cell::pill(&a.sync, argo::sync_tone(&a.sync)),
                            Cell::pill(&a.health, argo::health_tone(&a.health)),
                            Cell::mono(argo::revision_label(a), Tone::Muted),
                        ]
                    })
                    .collect();
                ui::card(ui, |ui| {
                    ui::table(
                        ui,
                        TableSpec {
                            id: "argoapps",
                            headers: &["App", "Projekt", "Sync", "Health", "Ziel"],
                            checks: if self.multi {
                                Some(&mut self.multi_checks)
                            } else {
                                None
                            },
                        },
                        &rows,
                        &mut self.selected,
                    );
                });
            }
        });
        if let Some(names) = self.confirm_sync.clone() {
            match ui::confirm(
                ui.ctx(),
                "argosync",
                &format!("{} App(s) synchronisieren?", names.len()),
                &[names.join(", ")],
                "Synchronisieren",
                false,
            ) {
                Some(true) => {
                    self.confirm_sync = None;
                    let mut args = sv(&["app", "sync"]);
                    args.extend(names.iter().cloned());
                    self.cli(
                        cx,
                        format!("Sync · {}", names.join(", ")),
                        args,
                        "(keine Ausgabe)",
                    );
                    self.apps.at = None;
                }
                Some(false) => self.confirm_sync = None,
                None => {}
            }
        }
    }

    fn detail(&mut self, ui: &mut Ui, cx: &Cx, a: &App) {
        let st = a.raw.get("status").cloned().unwrap_or(Value::Null);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(&a.name)
                    .strong()
                    .size(17.0)
                    .color(ui::theme::STRONG),
            );
        });
        ui.horizontal(|ui| {
            ui::pill(ui, &a.sync, argo::sync_tone(&a.sync));
            ui::pill(ui, &a.health, argo::health_tone(&a.health));
            ui::pill(
                ui,
                argo::policy_label(a.automated.as_ref()),
                argo::policy_tone(a.automated.as_ref()),
            );
        });
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            let name = a.name.clone();
            if ui.add(ui::primary_button("Sync")).clicked() {
                self.confirm_sync = Some(vec![name.clone()]);
            }
            for (label, extra, empty) in [
                (
                    "Refresh",
                    vec!["app", "get", "--refresh"],
                    "(keine Ausgabe)",
                ),
                (
                    "Hard Refresh",
                    vec!["app", "get", "--hard-refresh"],
                    "(keine Ausgabe)",
                ),
                ("Diff", vec!["app", "diff"], "Keine Unterschiede"),
                ("Historie", vec!["app", "history"], "Keine Historie"),
            ] {
                if ui.button(label).clicked() {
                    let mut args: Vec<String> = extra[..2].iter().map(|s| s.to_string()).collect();
                    args.push(name.clone());
                    args.extend(extra[2..].iter().map(|s| s.to_string()));
                    self.cli(cx, format!("{label} · {name}"), args, empty);
                }
            }
        });
        ui.add_space(6.0);
        ui::card(ui, |ui| {
            egui::Grid::new("argogrid")
                .num_columns(2)
                .spacing([14.0, 4.0])
                .show(ui, |ui| {
                    let mut row = |k: &str, v: String| {
                        ui.label(RichText::new(k).color(ui::theme::MUTED));
                        ui.add(egui::Label::new(v).wrap());
                        ui.end_row();
                    };
                    row("Projekt", a.project.clone());
                    row(
                        "Namespace",
                        a.raw
                            .pointer("/spec/destination/namespace")
                            .and_then(Value::as_str)
                            .unwrap_or("-")
                            .into(),
                    );
                    row(
                        "Revision",
                        st.pointer("/sync/revision")
                            .and_then(Value::as_str)
                            .map_or("-".into(), |r| r.chars().take(10).collect()),
                    );
                    if let Some(m) = st.pointer("/health/message").and_then(Value::as_str) {
                        row("Meldung", m.into());
                    }
                    row(
                        "Repo",
                        if a.repo.is_empty() {
                            "-".into()
                        } else {
                            a.repo.clone()
                        },
                    );
                    row(
                        "Pfad",
                        if a.path.is_empty() {
                            "-".into()
                        } else {
                            a.path.clone()
                        },
                    );
                    row("Ziel", argo::revision_label(a));
                    row("Sync-Policy", argo::policy_label(a.automated.as_ref()));
                    if let Some(op) = st.get("operationState") {
                        let fin = op
                            .get("finishedAt")
                            .and_then(Value::as_str)
                            .and_then(parse_iso)
                            .map_or("-".into(), |t| age(&t.to_rfc3339()));
                        row(
                            "Letzter Sync",
                            format!(
                                "{} vor {fin}",
                                op.get("phase").and_then(Value::as_str).unwrap_or("-")
                            ),
                        );
                        if let Some(m) = op.get("message").and_then(Value::as_str) {
                            row("", m.into());
                        }
                    }
                    if let Some(o) = &a.appset {
                        row("ApplicationSet", o.clone());
                    } else if let Some(m) = &a.managed_by {
                        row("Verwaltet von", m.clone());
                    }
                });
        });
        if let Some(repo) = argo::repo_from_url(&a.repo) {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Quelle").color(ui::theme::MUTED));
                if ui.button("Letzte Commits für den Pfad").clicked() {
                    let mut t = Task::default();
                    t.spawn(
                        cx,
                        crate::views::github::commits(
                            repo.clone(),
                            a.path.clone(),
                            a.revision.clone(),
                        ),
                    );
                    self.commits = Some((repo.clone(), a.path.clone(), a.revision.clone(), t));
                }
                let r = if a.revision.is_empty() || a.revision == "HEAD" {
                    "HEAD".to_string()
                } else {
                    a.revision.clone()
                };
                let suffix = if a.path.is_empty() {
                    String::new()
                } else {
                    format!("/{}", a.path)
                };
                ui.hyperlink_to(
                    "Pfad auf GitHub ↗",
                    format!("https://github.com/{repo}/tree/{r}{suffix}"),
                );
            });
        }
        let conditions = st
            .get("conditions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if !conditions.is_empty() {
            ui.add_space(8.0);
            ui::section_title(ui, "Bedingungen");
            for c in conditions {
                ui.label(
                    RichText::new(format!(
                        "{}: {}",
                        c.get("type").and_then(Value::as_str).unwrap_or("-"),
                        c.get("message").and_then(Value::as_str).unwrap_or("")
                    ))
                    .color(ui::theme::YELLOW),
                );
            }
        }
        let affected: Vec<Value> = st
            .get("resources")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|r| {
                r.get("status").and_then(Value::as_str) == Some("OutOfSync")
                    || matches!(
                        r.pointer("/health/status").and_then(Value::as_str),
                        Some("Degraded" | "Missing" | "Progressing" | "Unknown" | "Suspended")
                    )
            })
            .cloned()
            .collect();
        if !affected.is_empty() {
            ui.add_space(8.0);
            ui::section_title(ui, "Betroffene Ressourcen");
            let rows: Vec<Vec<Cell>> = affected
                .iter()
                .map(|r| {
                    let g = |p: &str| {
                        r.pointer(p)
                            .and_then(Value::as_str)
                            .unwrap_or("-")
                            .to_string()
                    };
                    vec![
                        Cell::muted(g("/kind")),
                        Cell::plain(g("/name")),
                        Cell::pill(g("/status"), argo::sync_tone(&g("/status"))),
                        Cell::pill(g("/health/status"), argo::health_tone(&g("/health/status"))),
                    ]
                })
                .collect();
            let mut none = None;
            ui::table(
                ui,
                TableSpec {
                    id: "affected",
                    headers: &["Kind", "Name", "Sync", "Health"],
                    checks: None,
                },
                &rows,
                &mut none,
            );
        }
    }

    fn commits_window(&mut self, ctx: &egui::Context) {
        let Some((repo, path, rev, task)) = &mut self.commits else {
            return;
        };
        task.poll();
        let mut open = true;
        egui::Window::new(RichText::new(format!("Commits · {repo}")).strong())
            .open(&mut open)
            .default_size([760.0, 460.0])
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(format!(
                        "{} @ {}",
                        if path.is_empty() { "/" } else { path },
                        if rev.is_empty() { "HEAD" } else { rev }
                    ))
                    .color(ui::theme::MUTED),
                );
                if ui::task_status(ui, task, "Lade Commits …") {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        egui::Grid::new("commits")
                            .striped(true)
                            .num_columns(4)
                            .spacing([12.0, 6.0])
                            .show(ui, |ui| {
                                for (sha, a, who, msg) in task.value().unwrap() {
                                    ui.hyperlink_to(
                                        RichText::new(&sha[..sha.len().min(7)])
                                            .monospace()
                                            .color(ui::theme::YELLOW),
                                        format!("https://github.com/{repo}/commit/{sha}"),
                                    );
                                    ui.label(RichText::new(a).color(ui::theme::MUTED));
                                    ui.label(RichText::new(who).color(ui::theme::MUTED));
                                    ui.label(msg);
                                    ui.end_row();
                                }
                            });
                    });
                }
            });
        if !open {
            self.commits = None;
        }
    }

    fn op_table(
        &mut self,
        ui: &mut Ui,
        headers: &[&str],
        rows: &[Vec<Cell>],
        preselect: impl Fn(usize) -> bool,
    ) {
        if self.op.checks.len() != rows.len() {
            self.op.checks = (0..rows.len()).map(preselect).collect();
        }
        let mut none = None;
        ui::card(ui, |ui| {
            ui::table(
                ui,
                TableSpec {
                    id: "opapps",
                    headers,
                    checks: Some(&mut self.op.checks),
                },
                rows,
                &mut none,
            );
        });
    }

    fn chosen(&self) -> Vec<App> {
        let apps = self.apps.value().cloned().unwrap_or_default();
        apps.into_iter()
            .zip(&self.op.checks)
            .filter(|(_, c)| **c)
            .map(|(a, _)| a)
            .collect()
    }

    fn plan_view(&mut self, ui: &mut Ui, cx: &Cx, question: String) {
        self.op.plan.poll();
        self.op.apply.poll();
        if let Some(rep) = self.op.apply.value().cloned() {
            ui::card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui::section_title(ui, "Ergebnis");
                ui::report_lines(ui, &rep);
                ui.add_space(6.0);
                if ui.button("Fertig").clicked() {
                    self.op = OpState::default();
                    self.apps.clear();
                }
            });
            return;
        }
        if self.op.apply.busy() {
            ui::spinner_label(ui, "Änderungen werden angewendet und nachgeprüft …");
            return;
        }
        if let Some(e) = self.op.apply.error() {
            ui::error_box(ui, e);
        }
        if self.op.plan.busy() {
            ui::spinner_label(ui, "Plane Änderungen …");
            return;
        }
        if let Some(e) = self.op.plan.error() {
            ui::error_box(ui, e);
            return;
        }
        let Some((plan, skipped)) = &self.op.plan.result.as_ref().and_then(|r| r.as_ref().ok())
        else {
            return;
        };
        let mut count = 0;
        ui::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui::section_title(ui, "Geplante Änderungen");
            ui.add_space(4.0);
            let line = |ui: &mut Ui, name: &str, change: String, extra: String| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(name).strong());
                    ui.label(RichText::new(change).color(ui::theme::CYAN));
                    if !extra.is_empty() {
                        ui.label(RichText::new(extra).color(ui::theme::MUTED));
                    }
                });
            };
            let note = |ui: &mut Ui, n: &str| {
                let tone =
                    if n.contains("verwaltet") || n.contains("prune") || n.contains("Auto-Sync") {
                        Tone::Warn
                    } else {
                        Tone::Muted
                    };
                ui.label(RichText::new(format!("    ⚠ {n}")).color(tone.color()));
            };
            match plan {
                Plan::Sync(c) => {
                    count = c.len();
                    for ch in c {
                        line(
                            ui,
                            &ch.app.name,
                            format!("{} → {}", ch.before, ch.after),
                            String::new(),
                        );
                        ch.notes.iter().for_each(|n| note(ui, n));
                    }
                }
                Plan::Branch(c) => {
                    count = c.len();
                    for ch in c {
                        line(
                            ui,
                            &ch.app.name,
                            format!("{} → {}", ch.previous, ch.target),
                            if ch.app.multi {
                                format!("({})", argo::source_title(&ch.source))
                            } else {
                                String::new()
                            },
                        );
                        match ch.remote {
                            Some(false) => {
                                ui.label(
                                    RichText::new(format!(
                                        "    ⚠ Branch {} existiert nicht in {}",
                                        ch.target,
                                        argo::short_repo(
                                            ch.source
                                                .get("repoURL")
                                                .and_then(Value::as_str)
                                                .unwrap_or_default()
                                        )
                                    ))
                                    .color(ui::theme::RED)
                                    .strong(),
                                );
                            }
                            None => {
                                ui.label(
                                    RichText::new("    Existenz im Remote-Repo nicht prüfbar")
                                        .color(ui::theme::MUTED),
                                );
                            }
                            Some(true) => {}
                        }
                        ch.notes.iter().for_each(|n| note(ui, n));
                    }
                }
                Plan::Scale(c) => {
                    count = c.len();
                    for ch in c {
                        line(
                            ui,
                            &ch.app.name,
                            format!("{} → {}", ch.before, ch.after),
                            format!("({} Workloads)", ch.ops.len()),
                        );
                        for (w, n) in &ch.ops {
                            ui.label(
                                RichText::new(format!(
                                    "    {}/{} ({}) → {n}",
                                    w.kind, w.name, w.namespace
                                ))
                                .color(ui::theme::MUTED),
                            );
                        }
                        ch.notes.iter().for_each(|n| note(ui, n));
                    }
                }
            }
            for (name, reason) in skipped {
                ui.label(
                    RichText::new(format!("{name}  übersprungen ({reason})"))
                        .color(ui::theme::MUTED),
                );
            }
            ui.add_space(6.0);
            if count == 0 {
                ui::note_box(ui, "Nichts zu tun.", Tone::Muted);
            } else if ui
                .add(ui::primary_button(format!("{count} App(s) ändern …")))
                .clicked()
            {
                self.op.confirm = true;
            }
        });
        if self.op.confirm {
            match ui::confirm(
                ui.ctx(),
                "argo-op",
                &question.replace("{n}", &count.to_string()),
                &["Die Änderung wird danach automatisch nachgeprüft (8 s).".into()],
                "Anwenden",
                true,
            ) {
                Some(true) => {
                    self.op.confirm = false;
                    let Some(Ok((plan, _))) = self.op.plan.result.take() else {
                        return;
                    };
                    let namespace = ns(cx);
                    self.op.apply.spawn(cx, async move {
                        Ok(match plan {
                            Plan::Sync(c) => argo::apply_sync(&namespace, c).await,
                            Plan::Branch(c) => argo::apply_branches(&namespace, c).await,
                            Plan::Scale(c) => argo::apply_scale(&namespace, c).await,
                        })
                    });
                }
                Some(false) => self.op.confirm = false,
                None => {}
            }
        }
    }

    fn owner_cell(a: &App) -> Cell {
        Cell::muted(
            a.appset
                .as_ref()
                .map(|s| format!("AppSet {s}"))
                .or_else(|| a.managed_by.clone())
                .unwrap_or_else(|| "-".into()),
        )
    }

    fn autosync_tab(&mut self, ui: &mut Ui, cx: &Cx) {
        ui.horizontal(|ui| {
            for (a, l, h) in [
                (
                    SyncAction::Disable,
                    "Auto-Sync deaktivieren",
                    "entfernt spec.syncPolicy.automated, merkt prune/selfHeal",
                ),
                (
                    SyncAction::Enable,
                    "Auto-Sync aktivieren",
                    "stellt die gemerkten Einstellungen wieder her",
                ),
            ] {
                if ui
                    .radio(self.sync_action == Some(a), l)
                    .on_hover_text(h)
                    .clicked()
                {
                    self.sync_action = Some(a);
                    self.op = OpState::default();
                }
            }
        });
        let Some(action) = self.sync_action else {
            ui.label(RichText::new("Aktion wählen, dann Apps auswählen.").color(ui::theme::MUTED));
            return;
        };
        if !ui::task_status(ui, &self.apps, "Lade Apps mit gemerktem Zustand …") {
            return;
        }
        let apps = self.apps.value().cloned().unwrap_or_default();
        if self.op.plan.result.is_none()
            && !self.op.plan.busy()
            && !self.op.apply.busy()
            && self.op.apply.result.is_none()
        {
            let rows: Vec<Vec<Cell>> = apps
                .iter()
                .map(|a| {
                    let applicable = (a.automated.is_some()) == (action == SyncAction::Disable);
                    let dim = |c: Cell| if applicable { c } else { Cell::muted(c.text) };
                    vec![
                        dim(Cell::plain(&a.name)),
                        dim(Cell::muted(&a.project)),
                        Cell::pill(
                            argo::policy_label(a.automated.as_ref()),
                            if applicable {
                                argo::policy_tone(a.automated.as_ref())
                            } else {
                                Tone::Muted
                            },
                        ),
                        Cell::muted(
                            a.stored
                                .as_ref()
                                .map_or("-".into(), |s| argo::policy_label(Some(s))),
                        ),
                        Self::owner_cell(a),
                    ]
                })
                .collect();
            self.op_table(
                ui,
                &["App", "Projekt", "Auto-Sync", "Gemerkt", "Verwaltet von"],
                &rows,
                |_| false,
            );
            let chosen = self.chosen();
            if ui
                .add_enabled(
                    !chosen.is_empty(),
                    ui::primary_button(format!("Plan für {} App(s) anzeigen", chosen.len())),
                )
                .clicked()
            {
                self.op.plan.spawn(cx, async move {
                    let mut changes = Vec::new();
                    let mut skipped = Vec::new();
                    for a in chosen {
                        match argo::plan_sync(&a, action) {
                            Ok(c) => changes.push(c),
                            Err(r) => skipped.push((a.name.clone(), r)),
                        }
                    }
                    Ok((Plan::Sync(changes), skipped))
                });
            }
        } else {
            if ui.button("← Auswahl ändern").clicked() && !self.op.apply.busy() {
                self.op.plan.clear();
            }
            self.plan_view(ui, cx, "{n} App(s) ändern?".into());
        }
    }

    fn feature_tab(&mut self, ui: &mut Ui, cx: &Cx) {
        ui.horizontal(|ui| {
            if ui
                .radio(!self.branch_reset, "Branch umstellen")
                .on_hover_text("spec.source.targetRevision ändern")
                .clicked()
            {
                self.branch_reset = false;
                self.op = OpState::default();
            }
            if ui
                .radio(
                    self.branch_reset,
                    format!("Auf {} zurücksetzen", argo::DEFAULT_BRANCH),
                )
                .on_hover_text("Apps abseits von main sind vorgewählt")
                .clicked()
            {
                self.branch_reset = true;
                self.op = OpState::default();
            }
            if !self.branch_reset {
                ui.separator();
                ui.label("Ziel-Branch");
                ui.add(
                    egui::TextEdit::singleline(&mut self.branch_target)
                        .hint_text(argo::DEFAULT_BRANCH)
                        .desired_width(220.0),
                );
            }
        });
        if !ui::task_status(ui, &self.apps, "Lade Apps …") {
            return;
        }
        let apps = self.apps.value().cloned().unwrap_or_default();
        if self.op.plan.result.is_none()
            && !self.op.plan.busy()
            && !self.op.apply.busy()
            && self.op.apply.result.is_none()
        {
            let rows: Vec<Vec<Cell>> = apps
                .iter()
                .map(|a| {
                    let extra = if a.sources.len() > 1 {
                        format!(" (+{})", a.sources.len() - 1)
                    } else {
                        String::new()
                    };
                    vec![
                        Cell::plain(&a.name),
                        Cell::mono(
                            argo::revision_label(a),
                            if argo::off_target(a) {
                                Tone::Warn
                            } else {
                                Tone::Muted
                            },
                        ),
                        Cell::pill(
                            argo::policy_label(a.automated.as_ref()),
                            argo::policy_tone(a.automated.as_ref()),
                        ),
                        Cell::muted(argo::short_repo(&a.repo) + &extra),
                    ]
                })
                .collect();
            let reset = self.branch_reset;
            let pre: Vec<bool> = apps.iter().map(|a| reset && argo::off_target(a)).collect();
            self.op_table(ui, &["App", "Branch", "Auto-Sync", "Quelle"], &rows, |i| {
                pre[i]
            });
            let chosen = self.chosen();
            let multi: Vec<&App> = chosen
                .iter()
                .filter(|a| a.multi && a.sources.len() > 1)
                .collect();
            if !multi.is_empty() {
                ui::card(ui, |ui| {
                    ui::section_title(ui, "Source je App mit mehreren Quellen");
                    for a in multi {
                        let cur = *self.source_pick.get(&a.name).unwrap_or(&0);
                        egui::ComboBox::from_label(&a.name)
                            .selected_text(argo::source_title(&a.sources[cur]))
                            .show_ui(ui, |ui| {
                                for (i, s) in a.sources.iter().enumerate() {
                                    if ui
                                        .selectable_label(i == cur, argo::source_title(s))
                                        .clicked()
                                    {
                                        self.source_pick.insert(a.name.clone(), i);
                                    }
                                }
                            });
                    }
                });
            }
            let target = if reset {
                argo::DEFAULT_BRANCH.to_string()
            } else {
                self.branch_target.trim().to_string()
            };
            let bad = target.chars().any(char::is_whitespace);
            if bad {
                ui::error_box(ui, "Ein Branch-Name darf keine Leerzeichen enthalten");
            }
            let ok = !chosen.is_empty() && !target.is_empty() && !bad;
            if ui
                .add_enabled(
                    ok,
                    ui::primary_button(format!("Plan für {} App(s) anzeigen", chosen.len())),
                )
                .clicked()
            {
                let picks: Vec<(App, usize)> = chosen
                    .into_iter()
                    .map(|a| {
                        let i = if a.multi {
                            *self.source_pick.get(&a.name).unwrap_or(&0)
                        } else {
                            0
                        };
                        (a, i)
                    })
                    .collect();
                let namespace = ns(cx);
                self.op.plan.spawn(cx, async move {
                    let (c, s) = argo::plan_branches(&namespace, picks, target).await?;
                    Ok((Plan::Branch(c), s))
                });
            }
        } else {
            if ui.button("← Auswahl ändern").clicked() && !self.op.apply.busy() {
                self.op.plan.clear();
            }
            let t = if self.branch_reset {
                argo::DEFAULT_BRANCH.to_string()
            } else {
                self.branch_target.trim().to_string()
            };
            self.plan_view(ui, cx, format!("{{n}} App(s) auf {t} umstellen?"));
        }
    }

    fn scale_tab(&mut self, ui: &mut Ui, cx: &Cx) {
        ui.horizontal(|ui| {
            for (a, l, h) in [
                (
                    ScaleAction::Down,
                    "Auf 0 Pods stellen",
                    "merkt die Replikas, deaktiviert Auto-Sync",
                ),
                (
                    ScaleAction::Set,
                    "Auf eigene Anzahl stellen",
                    "Zahl frei wählbar, auch nach oben",
                ),
                (
                    ScaleAction::Up,
                    "Pods wiederherstellen",
                    "stellt die gemerkten Replikas und Auto-Sync wieder her",
                ),
            ] {
                if ui
                    .radio(self.scale_action == Some(a), l)
                    .on_hover_text(h)
                    .clicked()
                {
                    self.scale_action = Some(a);
                    self.op = OpState::default();
                    self.scale_live.clear();
                    self.scale_only.clear();
                }
            }
            if self.scale_action == Some(ScaleAction::Set) {
                ui.separator();
                ui.label("Pods pro Workload");
                if self.scale_count.is_empty() {
                    self.scale_count = "1".into();
                }
                ui.add(egui::TextEdit::singleline(&mut self.scale_count).desired_width(60.0));
            }
        });
        let Some(action) = self.scale_action else {
            ui.label(RichText::new("Aktion wählen, dann Apps auswählen.").color(ui::theme::MUTED));
            return;
        };
        let count: Result<i64, &str> = match action {
            ScaleAction::Set => self
                .scale_count
                .trim()
                .parse::<i64>()
                .map_err(|_| "Bitte eine ganze Zahl eingeben")
                .and_then(|n| {
                    if n < 0 {
                        Err("Die Anzahl darf nicht negativ sein")
                    } else {
                        Ok(n)
                    }
                }),
            _ => Ok(0),
        };
        if !ui::task_status(ui, &self.apps, "Lade Apps mit gemerktem Zustand …") {
            return;
        }
        let apps = self.apps.value().cloned().unwrap_or_default();
        if self.op.plan.result.is_none()
            && !self.op.plan.busy()
            && !self.op.apply.busy()
            && self.op.apply.result.is_none()
        {
            let rows: Vec<Vec<Cell>> = apps
                .iter()
                .map(|a| {
                    let w = argo::workloads(a).len();
                    let applicable = if action == ScaleAction::Up {
                        a.scale.is_some()
                    } else {
                        w > 0
                    };
                    let status = match &a.scale {
                        Some(s) => Cell::pill(
                            format!("geändert (vorher {})", s.replicas.values().sum::<i64>()),
                            if applicable { Tone::Warn } else { Tone::Muted },
                        ),
                        None => Cell::muted("normal"),
                    };
                    vec![
                        if applicable {
                            Cell::plain(&a.name)
                        } else {
                            Cell::muted(&a.name)
                        },
                        Cell::muted(w.to_string()),
                        status,
                        Cell::pill(
                            argo::policy_label(a.automated.as_ref()),
                            if applicable {
                                argo::policy_tone(a.automated.as_ref())
                            } else {
                                Tone::Muted
                            },
                        ),
                    ]
                })
                .collect();
            self.op_table(
                ui,
                &["App", "Workloads", "Pods", "Auto-Sync"],
                &rows,
                |_| false,
            );
            let chosen = self.chosen();
            if let Err(e) = count {
                ui::error_box(ui, e);
            }
            self.scale_live.poll();
            if action == ScaleAction::Set {
                let entries: Vec<(String, argo::Workload)> = chosen
                    .iter()
                    .flat_map(|a| {
                        argo::workloads(a)
                            .into_iter()
                            .map(move |w| (a.name.clone(), w))
                    })
                    .collect();
                if entries.len() > chosen.len() {
                    if ui.button("Workloads einzeln wählen …").clicked() {
                        let items: Vec<argo::Workload> =
                            entries.iter().map(|e| e.1.clone()).collect();
                        self.scale_live
                            .spawn(cx, async move { argo::fetch_live(&items).await });
                        self.scale_only = entries.iter().map(|e| (e.1.key(), true)).collect();
                    }
                    if !self.scale_only.is_empty() {
                        ui::card(ui, |ui| {
                            ui::section_title(ui, "Workloads");
                            let live = self.scale_live.value().cloned().unwrap_or_default();
                            for (app, w) in &entries {
                                if let Some(v) = self.scale_only.get_mut(&w.key()) {
                                    let cur =
                                        live.get(&w.key()).map_or("?".into(), |n| n.to_string());
                                    ui.checkbox(
                                        v,
                                        format!("{app} · {}/{}  (aktuell {cur})", w.kind, w.name),
                                    );
                                }
                            }
                        });
                    }
                }
            }
            let ok = !chosen.is_empty() && count.is_ok();
            if ui
                .add_enabled(
                    ok,
                    ui::primary_button(format!("Plan für {} App(s) anzeigen", chosen.len())),
                )
                .clicked()
            {
                let n = count.unwrap_or(0);
                let only: Option<Vec<String>> = (!self.scale_only.is_empty()).then(|| {
                    self.scale_only
                        .iter()
                        .filter(|(_, v)| **v)
                        .map(|(k, _)| k.clone())
                        .collect()
                });
                self.op.plan.spawn(cx, async move {
                    let mut live = BTreeMap::new();
                    if action != ScaleAction::Up {
                        for a in &chosen {
                            live.extend(argo::fetch_live(&argo::workloads(a)).await?);
                        }
                    }
                    let mut changes = Vec::new();
                    let mut skipped = Vec::new();
                    for a in &chosen {
                        let r = if action == ScaleAction::Up {
                            argo::plan_up(a)
                        } else {
                            argo::plan_set(a, &live, n, only.as_deref())
                        };
                        match r {
                            Ok(c) => changes.push(c),
                            Err(e) => skipped.push((a.name.clone(), e)),
                        }
                    }
                    Ok((Plan::Scale(changes), skipped))
                });
            }
        } else {
            if ui.button("← Auswahl ändern").clicked() && !self.op.apply.busy() {
                self.op.plan.clear();
            }
            let q = match action {
                ScaleAction::Down => "{n} App(s) auf 0 Pods stellen?".to_string(),
                ScaleAction::Set => format!(
                    "{{n}} App(s) auf {} Pods je Workload stellen?",
                    count.unwrap_or(0)
                ),
                ScaleAction::Up => "{n} App(s) wiederherstellen?".into(),
            };
            self.plan_view(ui, cx, q);
        }
    }
}
