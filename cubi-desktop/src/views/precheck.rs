use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, CornerRadius, Frame, Margin, RichText, Stroke, Ui};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::precheck::config::{Check, Config, Profile};
use crate::precheck::git::{self, RepoState};
use crate::precheck::runner::{self, Event, Gate, Status};
use crate::precheck::suggest;
use crate::ui::{self, Cx, Tone, theme};

enum Msg {
    Loaded(Result<Loaded, String>),
    RunStarted { base: String, changed: usize },
    RunFailed(String),
    Action(Result<String, String>),
}

struct Loaded {
    state: RepoState,
    subjects: Vec<String>,
    gh_ok: Result<(), String>,
    pr_url: Option<String>,
}

struct CheckView {
    check: Check,
    status: Status,
    log: Vec<String>,
    started: Option<Instant>,
    elapsed: Option<Duration>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pending {
    Push,
    Pr,
}

struct RepoView {
    state: RepoState,
    profile: Profile,
    checks: Vec<CheckView>,
    selected: usize,
    ran_for: Option<String>,
    subjects: Vec<String>,
    gh_ok: Result<(), String>,
    pr_url: Option<String>,
    pr_title: String,
    pr_body: String,
    new_branch: String,
    confirm: Option<Pending>,
    confirm_ack: bool,
}

pub struct Precheck {
    cfg: Result<Config, String>,
    repo_input: String,
    view: Option<RepoView>,
    error: Option<String>,
    actions_log: Vec<(Tone, String)>,
    tx: mpsc::UnboundedSender<Msg>,
    rx: mpsc::UnboundedReceiver<Msg>,
    run_rx: Option<mpsc::UnboundedReceiver<Event>>,
    cancel: Option<CancellationToken>,
    busy: bool,
    cx: Option<Cx>,
    started: bool,
}

impl Precheck {
    pub fn new(cfg: Result<Config, String>, start_repo: Option<PathBuf>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let repo_input = start_repo
            .or_else(|| cfg.as_ref().ok()?.repo_paths().into_iter().next())
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        Self {
            cfg,
            repo_input,
            view: None,
            error: None,
            actions_log: Vec::new(),
            tx,
            rx,
            run_rx: None,
            cancel: None,
            busy: false,
            cx: None,
            started: false,
        }
    }

    fn spawn<F>(&self, fut: F)
    where
        F: std::future::Future<Output = Msg> + Send + 'static,
    {
        let Some(cx) = &self.cx else { return };
        let tx = self.tx.clone();
        let ctx = cx.egui.clone();
        cx.rt.spawn(async move {
            let _ = tx.send(fut.await);
            ctx.request_repaint();
        });
    }

    fn load_repo(&mut self) {
        let path = crate::precheck::config::expand_home(self.repo_input.trim());
        let cfg = self.cfg.as_ref().ok().cloned();
        self.spawn(async move { Msg::Loaded(load(path, cfg).await.map_err(|e| format!("{e:#}"))) });
    }

    fn running(&self) -> bool {
        self.cancel.is_some()
    }

    fn start_run(&mut self, only: Option<usize>) {
        let Some(cx) = self.cx.clone() else { return };
        let Some(v) = &mut self.view else { return };
        let Ok(cfg) = &self.cfg else { return };
        let indices: Vec<usize> = match only {
            Some(i) => vec![i],
            None => (0..v.checks.len()).collect(),
        };
        for &i in &indices {
            let c = &mut v.checks[i];
            c.status = Status::Pending;
            c.log.clear();
            c.started = None;
            c.elapsed = None;
        }
        v.ran_for = Some(v.state.fingerprint());
        let list: Vec<(usize, Check)> = indices
            .iter()
            .map(|&i| (i, v.checks[i].check.clone()))
            .collect();
        let repo = v.state.root.clone();
        let base_branch = v.profile.base_branch.clone();
        let config_dir = cfg.dir.clone();
        let parallel = cfg.app.parallel;
        let cancel = CancellationToken::new();
        self.cancel = Some(cancel.clone());
        let (etx, erx) = mpsc::unbounded_channel();
        self.run_rx = Some(erx);
        let tx = self.tx.clone();
        let ctx = cx.egui.clone();
        cx.rt.spawn(async move {
            match runner::prepare(&repo, &base_branch, &config_dir).await {
                Ok((warning, rc)) => {
                    if let Some(w) = warning {
                        let _ = tx.send(Msg::RunFailed(w));
                    }
                    let _ = tx.send(Msg::RunStarted {
                        base: rc.base.clone(),
                        changed: rc.changed.len(),
                    });
                    let pump_ctx = ctx.clone();
                    let (itx, mut irx) = mpsc::unbounded_channel();
                    let fwd = tokio::spawn(async move {
                        while let Some(ev) = irx.recv().await {
                            let _ = etx.send(ev);
                            pump_ctx.request_repaint();
                        }
                    });
                    runner::run_all(list, rc, parallel, itx, cancel).await;
                    let _ = fwd.await;
                }
                Err(e) => {
                    let _ = tx.send(Msg::RunFailed(format!(
                        "Vorbereitung fehlgeschlagen: {e:#}"
                    )));
                    for (idx, _) in list {
                        let _ = etx.send(Event::Status {
                            idx,
                            status: Status::Failed("Lauf nicht gestartet".into()),
                            elapsed: None,
                        });
                    }
                    let _ = etx.send(Event::AllDone);
                }
            }
            ctx.request_repaint();
        });
    }

    fn poll(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Loaded(Ok(l)) => self.apply_loaded(l),
                Msg::Loaded(Err(e)) => {
                    self.error = Some(e);
                    self.view = None;
                }
                Msg::RunStarted { base, changed } => {
                    self.log_action(
                        Tone::Muted,
                        format!(
                            "Lauf gestartet: Basis {}, {changed} geänderte Dateien",
                            &base[..base.len().min(10)]
                        ),
                    );
                }
                Msg::RunFailed(e) => self.log_action(Tone::Warn, e),
                Msg::Action(r) => {
                    self.busy = false;
                    match r {
                        Ok(s) => self.log_action(Tone::Ok, s),
                        Err(e) => self.log_action(Tone::Bad, format!("FEHLER: {e}")),
                    }
                    self.load_repo();
                }
            }
        }
        let mut done = false;
        if let (Some(rx), Some(v)) = (&mut self.run_rx, &mut self.view) {
            while let Ok(ev) = rx.try_recv() {
                match ev {
                    Event::Status {
                        idx,
                        status,
                        elapsed,
                    } => {
                        let c = &mut v.checks[idx];
                        if status == Status::Running {
                            c.started = Some(Instant::now());
                        }
                        if elapsed.is_some() {
                            c.elapsed = elapsed;
                        }
                        c.status = status;
                    }
                    Event::Log { idx, line } => v.checks[idx].log.push(line),
                    Event::AllDone => done = true,
                }
            }
        }
        if done {
            self.cancel = None;
            self.run_rx = None;
            self.refresh_suggestions();
            self.load_repo();
        }
    }

    fn apply_loaded(&mut self, l: Loaded) {
        self.error = None;
        let Ok(cfg) = &self.cfg else { return };
        let Some(slug) = l.state.remote_slug.clone() else {
            self.error = Some("Kein GitHub-Remote 'origin' gefunden".into());
            self.view = None;
            return;
        };
        let Some(profile) = cfg.profile_for_remote(&slug).cloned() else {
            self.error = Some(format!(
                "Kein Profil für {slug} in {}/profiles",
                cfg.dir.display()
            ));
            self.view = None;
            return;
        };
        let same = self
            .view
            .as_ref()
            .is_some_and(|v| v.state.root == l.state.root && v.profile.name == profile.name);
        if same {
            let v = self.view.as_mut().expect("view");
            let branch_changed = v.state.branch != l.state.branch;
            v.state = l.state;
            v.subjects = l.subjects;
            v.gh_ok = l.gh_ok;
            v.pr_url = l.pr_url;
            if branch_changed {
                self.refresh_suggestions();
            }
            return;
        }
        let checks = profile
            .checks
            .iter()
            .map(|c| CheckView {
                check: c.clone(),
                status: Status::Pending,
                log: Vec::new(),
                started: None,
                elapsed: None,
            })
            .collect();
        self.view = Some(RepoView {
            new_branch: suggest::branch_suggestion(&l.subjects),
            state: l.state,
            profile,
            checks,
            selected: 0,
            ran_for: None,
            subjects: l.subjects,
            gh_ok: l.gh_ok,
            pr_url: l.pr_url,
            pr_title: String::new(),
            pr_body: String::new(),
            confirm: None,
            confirm_ack: false,
        });
        self.refresh_suggestions();
    }

    fn refresh_suggestions(&mut self) {
        let Some(v) = &mut self.view else { return };
        let branch = v.state.branch.clone().unwrap_or_default();
        v.pr_title = suggest::pr_title(&v.profile.name, &branch, &v.subjects);
        let rows: Vec<(String, bool, Status)> = v
            .checks
            .iter()
            .map(|c| (c.check.name.clone(), c.check.required, c.status.clone()))
            .collect();
        v.pr_body = suggest::pr_body(&v.subjects, &rows);
    }

    fn log_action(&mut self, tone: Tone, s: String) {
        self.actions_log.push((tone, s));
    }

    fn blockers(&self) -> Vec<String> {
        let Some(v) = &self.view else { return vec![] };
        let checks: Vec<Check> = v.checks.iter().map(|c| c.check.clone()).collect();
        let statuses: Vec<Status> = v.checks.iter().map(|c| c.status.clone()).collect();
        let mut b = match runner::gate(&checks, &statuses) {
            Gate::Green => vec![],
            Gate::Blocked(b) => b,
        };
        if self.running() {
            b.push("Checks laufen noch".into());
        }
        if v.ran_for.is_some() && v.ran_for.as_deref() != Some(&v.state.fingerprint()) {
            b.push("Stand hat sich seit dem Lauf geändert (neu prüfen)".into());
        }
        if !v.state.changes.is_empty() {
            b.push(format!(
                "{} nicht committete Änderungen (gepusht wird nur HEAD)",
                v.state.changes.len()
            ));
        }
        b
    }

    fn do_push(&mut self) {
        let Some(v) = &self.view else { return };
        let Some(branch) = v.state.branch.clone() else {
            return;
        };
        let repo = v.state.root.clone();
        let protected = v.profile.protected_branches.clone();
        self.busy = true;
        self.log_action(Tone::Muted, format!("git push origin {branch} …"));
        self.spawn(async move {
            Msg::Action(
                git::push(&repo, &branch, &protected)
                    .await
                    .map_err(|e| format!("{e:#}")),
            )
        });
    }

    fn do_pr(&mut self) {
        let Some(v) = &self.view else { return };
        let Some(branch) = v.state.branch.clone() else {
            return;
        };
        let repo = v.state.root.clone();
        let base = v.profile.base_branch.clone();
        let protected = v.profile.protected_branches.clone();
        let (title, body) = (v.pr_title.clone(), v.pr_body.clone());
        self.busy = true;
        self.log_action(
            Tone::Muted,
            format!("gh pr create --base {base} --head {branch} …"),
        );
        self.spawn(async move {
            Msg::Action(
                git::pr_create(&repo, &base, &branch, &title, &body, &protected)
                    .await
                    .map(|url| format!("PR erstellt: {}", url.trim()))
                    .map_err(|e| format!("{e:#}")),
            )
        });
    }

    fn do_branch(&mut self) {
        let Some(v) = &self.view else { return };
        let repo = v.state.root.clone();
        let name = v.new_branch.trim().to_string();
        let protected = v.profile.protected_branches.clone();
        self.busy = true;
        self.spawn(async move {
            Msg::Action(
                git::create_branch(&repo, &name, &protected)
                    .await
                    .map(|_| format!("Branch {name} angelegt und ausgecheckt"))
                    .map_err(|e| format!("{e:#}")),
            )
        });
    }
}

async fn load(path: PathBuf, cfg: Option<Config>) -> anyhow::Result<Loaded> {
    let state = git::state(&path).await?;
    let base_branch = state
        .remote_slug
        .as_deref()
        .and_then(|s| cfg.as_ref()?.profile_for_remote(s))
        .map_or_else(|| "main".to_string(), |p| p.base_branch.clone());
    let subjects = match git::merge_base(&state.root, &base_branch).await {
        Ok(b) => git::commit_subjects(&state.root, &b)
            .await
            .unwrap_or_default(),
        Err(_) => vec![],
    };
    let gh_ok = git::gh_auth_status().await.map_err(|e| format!("{e:#}"));
    let pr_url = match (&state.branch, &gh_ok) {
        (Some(b), Ok(())) if state.upstream.is_some() => git::existing_pr(&state.root, b).await,
        _ => None,
    };
    Ok(Loaded {
        state,
        subjects,
        gh_ok,
        pr_url,
    })
}

pub fn status_tone(s: &Status) -> Tone {
    match s {
        Status::Ok => Tone::Ok,
        Status::Failed(_) | Status::Cancelled => Tone::Bad,
        Status::Missing(_) => Tone::Warn,
        Status::Running => Tone::Accent,
        Status::Skipped(_) | Status::Pending => Tone::Muted,
    }
}

fn status_icon(s: &Status) -> &'static str {
    match s {
        Status::Ok => "✔",
        Status::Failed(_) => "✖",
        Status::Cancelled => "⏹",
        Status::Missing(_) => "⚠",
        Status::Running => "⟳",
        Status::Skipped(_) => "⏭",
        Status::Pending => "○",
    }
}

fn fmt_dur(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 60 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{:.1}s", d.as_secs_f32())
    }
}

impl super::View for Precheck {
    fn id(&self) -> &'static str {
        "precheck"
    }
    fn title(&self) -> &'static str {
        "Pre-Push-Checks"
    }
    fn icon(&self) -> &'static str {
        "✔"
    }
    fn hint(&self) -> &'static str {
        "CI-Checks lokal, dann Push oder PR"
    }

    fn ui(&mut self, ui: &mut Ui, cx: &Cx) {
        if self.cx.is_none() {
            self.cx = Some(cx.clone());
        }
        if !self.started {
            self.started = true;
            if !self.repo_input.is_empty() {
                self.load_repo();
            }
        }
        self.poll();
        if self.running() {
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }
        self.ui_header(ui);
        egui::Panel::bottom("pc-actions")
            .resizable(false)
            .frame(Frame::NONE.inner_margin(Margin {
                left: 0,
                right: 0,
                top: 8,
                bottom: 0,
            }))
            .show(ui, |ui| self.ui_actions(ui));
        egui::Panel::left("pc-checks")
            .resizable(true)
            .min_size(380.0)
            .default_size(440.0)
            .max_size(700.0)
            .frame(Frame::NONE.inner_margin(Margin {
                left: 0,
                right: 8,
                top: 0,
                bottom: 0,
            }))
            .show(ui, |ui| self.ui_checks(ui));
        egui::CentralPanel::no_frame().show(ui, |ui| self.ui_log(ui));
        self.ui_confirm(ui.ctx());
    }
}

impl Precheck {
    fn ui_header(&mut self, ui: &mut Ui) {
        let running = self.running();
        ui::page_header(
            ui,
            "Pre-Push-Checks",
            "Führt die CI-Checks des Repos lokal aus. Push und PR nur auf Klick.",
            |ui| {
                if ui
                    .add_enabled(running, ui::danger_button("⏹ Abbrechen"))
                    .clicked()
                    && let Some(c) = &self.cancel
                {
                    c.cancel();
                }
                if ui
                    .add_enabled(
                        !running && self.view.is_some(),
                        ui::primary_button("▶ Alle Checks starten"),
                    )
                    .clicked()
                {
                    self.start_run(None);
                }
            },
        );
        if let Err(e) = &self.cfg {
            ui::error_box(ui, &format!("Konfiguration: {e}"));
        }
        ui::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new("Repo").color(theme::MUTED));
                if let Ok(cfg) = &self.cfg {
                    let mut pick = None;
                    egui::ComboBox::from_id_salt("repo-pick")
                        .selected_text("Konfigurierte Repos")
                        .show_ui(ui, |ui| {
                            for p in cfg.repo_paths() {
                                let s = p.display().to_string();
                                if ui.selectable_label(false, &s).clicked() {
                                    pick = Some(s);
                                }
                            }
                        });
                    if let Some(p) = pick.filter(|_| !running) {
                        self.repo_input = p;
                        self.view = None;
                        self.load_repo();
                    }
                }
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.repo_input)
                        .desired_width(420.0)
                        .font(egui::TextStyle::Monospace),
                );
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui
                    .add_enabled(!running, egui::Button::new("Öffnen"))
                    .clicked()
                    || enter)
                    && !running
                {
                    self.view = None;
                    self.load_repo();
                }
                if ui
                    .button("⟳")
                    .on_hover_text("Git-Status neu lesen")
                    .clicked()
                {
                    self.load_repo();
                }
            });
            if let Some(e) = &self.error {
                ui::error_box(ui, e);
            }
            if let Some(v) = &self.view {
                let s = &v.state;
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(s.remote_slug.clone().unwrap_or_default())
                            .strong()
                            .size(15.0)
                            .color(theme::STRONG),
                    );
                    ui::pill(ui, format!("Profil {}", v.profile.name), Tone::Accent);
                    let branch = s.branch.clone().unwrap_or_else(|| "(detached)".into());
                    let protected =
                        git::push_allowed(s.branch.as_deref(), &v.profile.protected_branches)
                            .is_err();
                    ui::pill(
                        ui,
                        format!("⎇ {branch}"),
                        if protected { Tone::Warn } else { Tone::Info },
                    );
                    ui::pill(
                        ui,
                        format!("HEAD {}", &s.head[..s.head.len().min(8)]),
                        Tone::Muted,
                    );
                    match &s.upstream {
                        Some(u) => ui::pill(
                            ui,
                            format!("{u} · ↑{} ↓{}", s.ahead, s.behind),
                            if s.ahead > 0 { Tone::Warn } else { Tone::Ok },
                        ),
                        None => ui::pill(ui, "noch nicht gepusht", Tone::Warn),
                    };
                    if s.changes.is_empty() {
                        ui::pill(ui, "sauber", Tone::Ok);
                    } else {
                        ui::pill(
                            ui,
                            format!("{} geänderte Dateien", s.changes.len()),
                            Tone::Warn,
                        )
                        .on_hover_text(s.changes.join("\n"));
                    }
                    ui::pill(
                        ui,
                        format!("{} Commits seit Basis", v.subjects.len()),
                        Tone::Muted,
                    )
                    .on_hover_text(v.subjects.join("\n"));
                    match &v.gh_ok {
                        Ok(()) => ui::pill(ui, "gh angemeldet", Tone::Ok),
                        Err(e) => ui::pill(ui, e, Tone::Warn),
                    };
                });
            }
        });
        ui.add_space(8.0);
    }

    fn ui_checks(&mut self, ui: &mut Ui) {
        let running = self.running();
        let mut start: Option<usize> = None;
        let Some(v) = &mut self.view else {
            ui.label(RichText::new("Kein Repo geladen.").color(theme::MUTED));
            return;
        };
        let total = v.checks.len();
        let ok = v
            .checks
            .iter()
            .filter(|c| matches!(c.status, Status::Ok | Status::Skipped(_)))
            .count();
        let bad = v
            .checks
            .iter()
            .filter(|c| {
                c.check.required
                    && matches!(
                        c.status,
                        Status::Failed(_) | Status::Missing(_) | Status::Cancelled
                    )
            })
            .count();
        ui.horizontal(|ui| {
            ui::section_title(ui, "Checks");
            ui::pill(
                ui,
                format!("{ok}/{total} grün"),
                if ok == total { Tone::Ok } else { Tone::Muted },
            );
            if bad > 0 {
                ui::pill(ui, format!("{bad} Pflicht offen"), Tone::Bad);
            }
        });
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (i, c) in v.checks.iter().enumerate() {
                    let tone = status_tone(&c.status);
                    let selected = v.selected == i;
                    let elapsed = match (&c.status, c.started, c.elapsed) {
                        (Status::Running, Some(s), _) => Some(s.elapsed()),
                        (_, _, e) => e,
                    };
                    let resp = Frame::new()
                        .fill(if selected { theme::RAISED } else { theme::CARD })
                        .stroke(Stroke::new(
                            1.0,
                            if selected {
                                theme::ACCENT.gamma_multiply(0.7)
                            } else {
                                theme::BORDER
                            },
                        ))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::symmetric(10, 7))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui
                                            .add_enabled(!running, egui::Button::new("▶").small())
                                            .on_hover_text("Nur diesen Check starten")
                                            .clicked()
                                        {
                                            start = Some(i);
                                        }
                                        ui::pill(ui, c.status.label(), tone);
                                        if let Some(e) = elapsed {
                                            ui.label(
                                                RichText::new(fmt_dur(e))
                                                    .small()
                                                    .color(theme::MUTED),
                                            );
                                        }
                                        ui.with_layout(
                                            egui::Layout::left_to_right(egui::Align::Center),
                                            |ui| {
                                                if c.status == Status::Running {
                                                    ui.spinner();
                                                } else {
                                                    ui.label(
                                                        RichText::new(status_icon(&c.status))
                                                            .color(tone.color())
                                                            .size(16.0),
                                                    );
                                                }
                                                ui.vertical(|ui| {
                                                    ui.horizontal(|ui| {
                                                        ui.add(
                                                            egui::Label::new(
                                                                RichText::new(&c.check.name)
                                                                    .color(theme::STRONG),
                                                            )
                                                            .truncate(),
                                                        );
                                                    });
                                                    let sub = c.status.detail().map_or_else(
                                                        || c.check.source.clone(),
                                                        str::to_string,
                                                    );
                                                    let sub = if c.check.required {
                                                        sub
                                                    } else {
                                                        format!("optional · {sub}")
                                                    };
                                                    ui.add(
                                                        egui::Label::new(
                                                            RichText::new(sub).small().color(
                                                                if c.status.detail().is_some() {
                                                                    tone.color()
                                                                } else {
                                                                    theme::MUTED
                                                                },
                                                            ),
                                                        )
                                                        .truncate(),
                                                    );
                                                });
                                            },
                                        );
                                    },
                                );
                            });
                        })
                        .response
                        .interact(egui::Sense::click());
                    if resp.clicked() {
                        v.selected = i;
                    }
                    ui.add_space(4.0);
                }
            });
        if let Some(i) = start {
            self.start_run(Some(i));
        }
    }

    fn ui_log(&mut self, ui: &mut Ui) {
        let Some(v) = &self.view else { return };
        let Some(c) = v.checks.get(v.selected) else {
            return;
        };
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(&c.check.name)
                    .strong()
                    .size(16.0)
                    .color(theme::STRONG),
            );
            ui::pill(ui, c.status.label(), status_tone(&c.status));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("📋 Log kopieren").clicked() {
                    ui.ctx().copy_text(c.log.join("\n"));
                }
            });
        });
        ui.label(
            RichText::new(format!("Quelle: {}", c.check.source))
                .small()
                .color(theme::MUTED),
        );
        ui.add_space(4.0);
        Frame::new()
            .fill(egui::Color32::from_rgb(0x0b, 0x0e, 0x12))
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::same(10))
            .show(ui, |ui| {
                let row_h = ui.text_style_height(&egui::TextStyle::Monospace);
                if c.log.is_empty() {
                    ui.set_min_size(ui.available_size());
                    ui.label(
                        RichText::new("Noch keine Ausgabe.")
                            .monospace()
                            .color(theme::MUTED),
                    );
                    return;
                }
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show_rows(ui, row_h, c.log.len(), |ui, range| {
                        for line in &c.log[range] {
                            let color = if line.starts_with("$ ") {
                                theme::ACCENT
                            } else if line.starts_with("übersprungen")
                                || line.starts_with("Voraussetzung")
                            {
                                theme::YELLOW
                            } else {
                                theme::TEXT
                            };
                            ui.label(RichText::new(line).monospace().color(color));
                        }
                    });
            });
    }

    fn ui_actions(&mut self, ui: &mut Ui) {
        let blockers = self.blockers();
        let busy = self.busy || self.running();
        let mut act: Option<Pending> = None;
        let mut confirm: Option<Pending> = None;
        let mut make_branch = false;
        let mut regen = false;
        ui::card(ui, |ui| {
            ui.set_width(ui.available_width());
            let Some(v) = &mut self.view else {
                ui.label(RichText::new("Kein Repo geladen.").color(theme::MUTED));
                return;
            };
            let push_rule =
                git::push_allowed(v.state.branch.as_deref(), &v.profile.protected_branches);
            if let Err(why) = &push_rule {
                ui::note_box(ui, why, Tone::Warn);
                ui.horizontal(|ui| {
                    ui.label("Neuer Branch");
                    ui.add(
                        egui::TextEdit::singleline(&mut v.new_branch)
                            .font(egui::TextStyle::Monospace)
                            .desired_width(320.0),
                    );
                    if ui
                        .add_enabled(!busy, ui::primary_button("Branch anlegen"))
                        .clicked()
                    {
                        make_branch = true;
                    }
                });
            } else {
                ui.horizontal(|ui| {
                    if blockers.is_empty() {
                        ui::pill(ui, "✔ Alle Pflicht-Checks grün", Tone::Ok);
                    } else {
                        ui::pill(ui, "Push gesperrt", Tone::Bad).on_hover_text(blockers.join("\n"));
                        ui.add(
                            egui::Label::new(
                                RichText::new(blockers.join(" · "))
                                    .color(theme::RED)
                                    .small(),
                            )
                            .truncate(),
                        );
                    }
                });
                ui.add_space(4.0);
                let pushed = v.state.upstream.is_some() && v.state.ahead == 0;
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            blockers.is_empty() && !busy && !pushed,
                            ui::primary_button("⬆ Push"),
                        )
                        .clicked()
                    {
                        act = Some(Pending::Push);
                    }
                    if ui
                        .add_enabled(
                            !blockers.is_empty() && !busy && !pushed,
                            egui::Button::new("Push trotzdem …"),
                        )
                        .clicked()
                    {
                        confirm = Some(Pending::Push);
                    }
                    ui.separator();
                    let pr_possible = pushed && v.pr_url.is_none() && v.gh_ok.is_ok() && !busy;
                    if ui
                        .add_enabled(
                            pr_possible && blockers.is_empty(),
                            ui::primary_button("PR erstellen"),
                        )
                        .clicked()
                    {
                        act = Some(Pending::Pr);
                    }
                    if ui
                        .add_enabled(
                            pr_possible && !blockers.is_empty(),
                            egui::Button::new("PR trotzdem …"),
                        )
                        .clicked()
                    {
                        confirm = Some(Pending::Pr);
                    }
                    if let Some(url) = &v.pr_url {
                        ui.hyperlink_to(format!("Offener PR ↗ {url}"), url);
                    } else if !pushed {
                        ui.label(
                            RichText::new("PR erst nach dem Push möglich.").color(theme::MUTED),
                        );
                    }
                    if busy {
                        ui.spinner();
                    }
                });
                if v.pr_url.is_none() {
                    egui::CollapsingHeader::new("PR-Titel und Beschreibung")
                        .default_open(false)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Titel");
                                ui.add(
                                    egui::TextEdit::singleline(&mut v.pr_title)
                                        .desired_width(620.0),
                                );
                                if ui.button("Vorschlag neu erzeugen").clicked() {
                                    regen = true;
                                }
                            });
                            ui.add(
                                egui::TextEdit::multiline(&mut v.pr_body)
                                    .desired_rows(6)
                                    .desired_width(f32::INFINITY)
                                    .code_editor(),
                            );
                        });
                }
            }
            egui::CollapsingHeader::new(format!("Aktionen ({})", self.actions_log.len()))
                .default_open(false)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(110.0)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            for (tone, l) in &self.actions_log {
                                ui.label(RichText::new(l).monospace().small().color(tone.color()));
                            }
                        });
                });
        });
        if regen {
            self.refresh_suggestions();
        }
        if make_branch {
            self.do_branch();
        }
        match act {
            Some(Pending::Push) => self.do_push(),
            Some(Pending::Pr) => self.do_pr(),
            None => {}
        }
        if let (Some(p), Some(v)) = (confirm, &mut self.view) {
            v.confirm = Some(p);
            v.confirm_ack = false;
        }
    }

    fn ui_confirm(&mut self, ctx: &egui::Context) {
        let blockers = self.blockers();
        let Some(v) = &mut self.view else { return };
        let Some(kind) = v.confirm else { return };
        let mut go = false;
        let mut close = false;
        let what = match kind {
            Pending::Push => "pushen",
            Pending::Pr => "den PR erstellen",
        };
        egui::Modal::new(egui::Id::unique("cubi-desktop-override")).show(ctx, |ui| {
            ui.set_max_width(560.0);
            ui.label(
                RichText::new("Override bestätigen")
                    .heading()
                    .strong()
                    .color(theme::STRONG),
            );
            ui.add_space(4.0);
            ui.label("Folgende Pflicht-Bedingungen sind nicht erfüllt:");
            for b in &blockers {
                ui.label(RichText::new(format!("• {b}")).color(theme::RED));
            }
            ui.add_space(8.0);
            ui.checkbox(
                &mut v.confirm_ack,
                format!("Ich will trotzdem {what} und übernehme die Verantwortung."),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(v.confirm_ack, ui::danger_button(format!("Trotzdem {what}")))
                    .clicked()
                {
                    go = true;
                }
                if ui.button("Abbrechen").clicked() {
                    close = true;
                }
            });
        });
        if go || close {
            v.confirm = None;
            v.confirm_ack = false;
        }
        if go {
            self.log_action(
                Tone::Warn,
                format!("Override bestätigt ({what}) trotz: {}", blockers.join("; ")),
            );
            match kind {
                Pending::Push => self.do_push(),
                Pending::Pr => self.do_pr(),
            }
        }
    }
}
