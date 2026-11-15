use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::config::{Check, Config, Profile};
use crate::git::{self, RepoState};
use crate::runner::{self, Event, Gate, Status};
use crate::suggest;

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
    base: Option<String>,
    subjects: Vec<String>,
    gh_ok: Result<(), String>,
    pr_url: Option<String>,
    pr_title: String,
    pr_body: String,
    new_branch: String,
    confirm: Option<Pending>,
    confirm_ack: bool,
}

pub struct App {
    rt: tokio::runtime::Runtime,
    cfg: Result<Config, String>,
    repo_input: String,
    view: Option<RepoView>,
    error: Option<String>,
    actions_log: Vec<String>,
    tx: mpsc::UnboundedSender<Msg>,
    rx: mpsc::UnboundedReceiver<Msg>,
    run_rx: Option<mpsc::UnboundedReceiver<Event>>,
    cancel: Option<CancellationToken>,
    busy: bool,
    ctx: Option<egui::Context>,
}

impl App {
    pub fn new(cfg: Result<Config, String>, start_repo: Option<PathBuf>) -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio-Runtime");
        let (tx, rx) = mpsc::unbounded_channel();
        let repo_input = start_repo
            .or_else(|| cfg.as_ref().ok()?.repo_paths().into_iter().next())
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let mut app = Self {
            rt,
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
            ctx: None,
        };
        if !app.repo_input.is_empty() {
            app.load_repo();
        }
        app
    }

    fn spawn<F>(&self, fut: F)
    where
        F: std::future::Future<Output = Msg> + Send + 'static,
    {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        self.rt.spawn(async move {
            let _ = tx.send(fut.await);
            if let Some(c) = ctx {
                c.request_repaint();
            }
        });
    }

    fn load_repo(&mut self) {
        let path = crate::config::expand_home(self.repo_input.trim());
        let cfg = self.cfg.as_ref().ok().cloned();
        self.spawn(async move { Msg::Loaded(load(path, cfg).await.map_err(|e| format!("{e:#}"))) });
    }

    fn running(&self) -> bool {
        self.cancel.is_some()
    }

    fn start_run(&mut self, only: Option<usize>) {
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
        let ctx = self.ctx.clone();
        self.rt.spawn(async move {
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
                            if let Some(c) = &pump_ctx {
                                c.request_repaint();
                            }
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
            if let Some(c) = ctx {
                c.request_repaint();
            }
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
                    self.log_action(format!(
                        "Lauf gestartet: Basis {}, {changed} geänderte Dateien",
                        &base[..base.len().min(10)]
                    ));
                    if let Some(v) = &mut self.view {
                        v.base = Some(base);
                    }
                }
                Msg::RunFailed(e) => self.log_action(e),
                Msg::Action(r) => {
                    self.busy = false;
                    match r {
                        Ok(s) => self.log_action(s),
                        Err(e) => self.log_action(format!("FEHLER: {e}")),
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
            base: None,
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

    fn log_action(&mut self, s: String) {
        self.actions_log.push(s);
    }

    fn blockers(&self) -> Vec<String> {
        let Some(v) = &self.view else {
            return vec![];
        };
        let checks: Vec<Check> = v.checks.iter().map(|c| c.check.clone()).collect();
        let statuses: Vec<Status> = v.checks.iter().map(|c| c.status.clone()).collect();
        let mut b = match runner::gate(&checks, &statuses) {
            Gate::Green => vec![],
            Gate::Blocked(b) => b,
        };
        if self.running() {
            b.push("Checks laufen noch".into());
        }
        if v.ran_for.as_deref() != Some(&v.state.fingerprint()) && v.ran_for.is_some() {
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
        self.log_action(format!("git push origin {branch} ..."));
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
        self.log_action(format!("gh pr create --base {base} --head {branch} ..."));
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

fn status_color(s: &Status) -> Color32 {
    match s {
        Status::Ok => Color32::from_rgb(60, 170, 90),
        Status::Failed(_) | Status::Cancelled => Color32::from_rgb(220, 70, 70),
        Status::Missing(_) => Color32::from_rgb(230, 150, 40),
        Status::Running => Color32::from_rgb(80, 140, 230),
        Status::Skipped(_) | Status::Pending => Color32::GRAY,
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

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.ctx.is_none() {
            self.ctx = Some(ui.ctx().clone());
        }
        self.poll();
        if self.running() {
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }

        egui::Panel::top("repo").show(ui, |ui| self.ui_repo(ui));
        egui::Panel::bottom("actions")
            .resizable(true)
            .show(ui, |ui| self.ui_actions(ui));
        egui::Panel::left("checks")
            .resizable(true)
            .default_size(420.0)
            .show(ui, |ui| self.ui_checks(ui));
        egui::CentralPanel::default_margins().show(ui, |ui| self.ui_log(ui));
        self.ui_confirm(ui.ctx());
    }
}

impl App {
    fn ui_repo(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        if let Err(e) = &self.cfg {
            ui.colored_label(Color32::RED, format!("Konfiguration: {e}"));
        }
        ui.horizontal(|ui| {
            ui.label("Repo:");
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
                if let Some(p) = pick {
                    self.repo_input = p;
                    self.view = None;
                    self.load_repo();
                }
            }
            let resp =
                ui.add(egui::TextEdit::singleline(&mut self.repo_input).desired_width(380.0));
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui
                .add_enabled(!self.running(), egui::Button::new("Öffnen"))
                .clicked()
                || enter)
                && !self.running()
            {
                self.view = None;
                self.load_repo();
            }
            if ui.button("Aktualisieren").clicked() {
                self.load_repo();
            }
        });
        if let Some(e) = &self.error {
            ui.colored_label(Color32::RED, e);
        }
        if let Some(v) = &self.view {
            let s = &v.state;
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(s.remote_slug.clone().unwrap_or_default()).strong());
                ui.label(format!("Profil: {}", v.profile.name));
                ui.separator();
                let branch = s.branch.clone().unwrap_or_else(|| "(detached)".into());
                let protected =
                    git::push_allowed(s.branch.as_deref(), &v.profile.protected_branches).is_err();
                ui.label(
                    RichText::new(format!("Branch: {branch}")).color(if protected {
                        Color32::from_rgb(230, 150, 40)
                    } else {
                        ui.visuals().text_color()
                    }),
                );
                ui.label(format!("HEAD {}", &s.head[..s.head.len().min(8)]));
                ui.separator();
                match &s.upstream {
                    Some(u) => ui.label(format!("{u}: {} voraus, {} zurück", s.ahead, s.behind)),
                    None => ui.label("noch nicht gepusht"),
                };
                ui.separator();
                if s.changes.is_empty() {
                    ui.label("Arbeitsverzeichnis sauber");
                } else {
                    ui.label(format!("{} geänderte Dateien", s.changes.len()))
                        .on_hover_text(s.changes.join("\n"));
                }
                ui.separator();
                ui.label(format!("{} Commits seit Basis", v.subjects.len()))
                    .on_hover_text(v.subjects.join("\n"));
                ui.separator();
                match &v.gh_ok {
                    Ok(()) => ui.label("gh: angemeldet"),
                    Err(e) => ui.colored_label(Color32::from_rgb(230, 150, 40), e),
                };
            });
        }
        ui.add_space(4.0);
    }

    fn ui_checks(&mut self, ui: &mut egui::Ui) {
        let running = self.running();
        let mut start: Option<Option<usize>> = None;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !running && self.view.is_some(),
                    egui::Button::new("▶ Alle Checks starten"),
                )
                .clicked()
            {
                start = Some(None);
            }
            if ui
                .add_enabled(running, egui::Button::new("⏹ Abbrechen"))
                .clicked()
                && let Some(c) = &self.cancel
            {
                c.cancel();
            }
        });
        ui.separator();
        let Some(v) = &mut self.view else {
            ui.label("Kein Repo geladen.");
            return;
        };
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (i, c) in v.checks.iter().enumerate() {
                let elapsed = match (&c.status, c.started, c.elapsed) {
                    (Status::Running, Some(s), _) => Some(s.elapsed()),
                    (_, _, e) => e,
                };
                ui.horizontal(|ui| {
                    ui.label(RichText::new(status_icon(&c.status)).color(status_color(&c.status)));
                    let mut text = RichText::new(&c.check.name);
                    if !c.check.required {
                        text = text.italics();
                    }
                    let r = ui.selectable_label(v.selected == i, text);
                    if r.clicked() {
                        v.selected = i;
                    }
                    r.on_hover_text(format!(
                        "{}\nQuelle: {}\n{}",
                        c.check.id,
                        c.check.source,
                        if c.check.required {
                            "Pflicht"
                        } else {
                            "optional"
                        }
                    ));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(!running, egui::Button::new("▶").small())
                            .on_hover_text("Nur diesen Check starten")
                            .clicked()
                        {
                            start = Some(Some(i));
                        }
                        if let Some(e) = elapsed {
                            ui.label(fmt_dur(e));
                        }
                        ui.label(RichText::new(c.status.label()).color(status_color(&c.status)));
                    });
                });
                if let Some(d) = c.status.detail() {
                    ui.label(
                        RichText::new(format!("    {d}"))
                            .small()
                            .color(status_color(&c.status)),
                    );
                }
            }
        });
        if let Some(s) = start {
            self.start_run(s);
        }
    }

    fn ui_log(&mut self, ui: &mut egui::Ui) {
        let Some(v) = &self.view else { return };
        let Some(c) = v.checks.get(v.selected) else {
            return;
        };
        ui.horizontal(|ui| {
            ui.heading(&c.check.name);
            ui.label(RichText::new(c.status.label()).color(status_color(&c.status)));
        });
        ui.label(RichText::new(format!("Quelle: {}", c.check.source)).small());
        ui.separator();
        let row_h = ui.text_style_height(&egui::TextStyle::Monospace);
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show_rows(ui, row_h, c.log.len(), |ui, range| {
                for line in &c.log[range] {
                    ui.label(RichText::new(line).monospace());
                }
            });
    }

    fn ui_actions(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        let blockers = self.blockers();
        let busy = self.busy || self.running();
        let mut act: Option<Pending> = None;
        let mut confirm: Option<Pending> = None;
        let mut make_branch = false;
        let mut regen = false;
        if let Some(v) = &mut self.view {
            let branch = v.state.branch.clone();
            let push_rule = git::push_allowed(branch.as_deref(), &v.profile.protected_branches);
            if let Err(why) = &push_rule {
                ui.colored_label(Color32::from_rgb(230, 150, 40), why);
                ui.horizontal(|ui| {
                    ui.label("Neuer Branch:");
                    ui.text_edit_singleline(&mut v.new_branch);
                    if ui
                        .add_enabled(!busy, egui::Button::new("Branch anlegen"))
                        .clicked()
                    {
                        make_branch = true;
                    }
                });
            } else {
                if blockers.is_empty() {
                    ui.colored_label(status_color(&Status::Ok), "Alle Pflicht-Checks grün.");
                } else {
                    ui.colored_label(
                        status_color(&Status::Failed(String::new())),
                        format!("Gesperrt: {}", blockers.join("; ")),
                    );
                }
                let pushed = v.state.upstream.is_some() && v.state.ahead == 0;
                ui.horizontal(|ui| {
                    let can_push = blockers.is_empty() && !busy && !pushed;
                    if ui
                        .add_enabled(can_push, egui::Button::new("⬆ Push"))
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
                            egui::Button::new("PR erstellen"),
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
                        ui.hyperlink_to(format!("Offener PR: {url}"), url);
                    } else if !pushed {
                        ui.label("PR erst nach dem Push möglich.");
                    }
                });
                if v.pr_url.is_none() {
                    egui::CollapsingHeader::new("PR-Titel und Beschreibung")
                        .default_open(false)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Titel:");
                                ui.add(
                                    egui::TextEdit::singleline(&mut v.pr_title)
                                        .desired_width(600.0),
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
        }
        egui::CollapsingHeader::new(format!("Aktionen ({})", self.actions_log.len()))
            .default_open(true)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(120.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for l in &self.actions_log {
                            ui.label(RichText::new(l).monospace().small());
                        }
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
        egui::Modal::new(egui::Id::unique("cubi-precheck-override")).show(ctx, |ui| {
            ui.heading("Override bestätigen");
            ui.label("Folgende Pflicht-Bedingungen sind nicht erfüllt:");
            for b in &blockers {
                ui.label(format!("• {b}"));
            }
            ui.add_space(8.0);
            ui.checkbox(
                &mut v.confirm_ack,
                format!("Ich will trotzdem {what} und übernehme die Verantwortung."),
            );
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(v.confirm_ack, egui::Button::new(format!("Trotzdem {what}")))
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
            self.log_action(format!(
                "Override bestätigt ({what}) trotz: {}",
                blockers.join("; ")
            ));
            match kind {
                Pending::Push => self.do_push(),
                Pending::Pr => self.do_pr(),
            }
        }
    }
}
