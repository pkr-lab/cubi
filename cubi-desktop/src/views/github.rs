use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::{Context, bail};
use eframe::egui::{self, RichText, Ui};
use serde_json::Value;

use crate::cubi_config::as_str;
use crate::shell;
use crate::timeutil::age;
use crate::ui::{self, Cell, Cx, OutputWindow, TableSpec, Task, Tone};
use crate::views::kube::sv;

const RUN_FIELDS: &str =
    "databaseId,displayTitle,status,conclusion,workflowName,headBranch,event,createdAt";
const PR_FIELDS: &str = "number,title,author,headRefName,isDraft,updatedAt,reviewDecision";
const ISSUE_FIELDS: &str = "number,title,author,labels,updatedAt";

pub fn run_tone(s: &str) -> Tone {
    match s {
        "success" => Tone::Ok,
        "failure" | "timed_out" | "startup_failure" => Tone::Bad,
        "cancelled" | "skipped" | "neutral" | "stale" => Tone::Muted,
        "in_progress" | "queued" | "waiting" | "pending" | "action_required" => Tone::Warn,
        _ => Tone::Plain,
    }
}

pub fn run_status(e: &Value) -> String {
    let st = gs(e, "status");
    let concl = gs(e, "conclusion");
    if st == "completed" && !concl.is_empty() {
        concl
    } else if st.is_empty() {
        "-".into()
    } else {
        st
    }
}

pub fn review(e: &Value) -> (&'static str, Tone) {
    if e.get("isDraft").and_then(Value::as_bool).unwrap_or(false) {
        return ("draft", Tone::Muted);
    }
    match e
        .get("reviewDecision")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "APPROVED" => ("approved", Tone::Ok),
        "CHANGES_REQUESTED" => ("changes", Tone::Bad),
        "REVIEW_REQUIRED" => ("review", Tone::Warn),
        _ => ("-", Tone::Muted),
    }
}

pub fn gs(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn author(e: &Value) -> String {
    let a = e.get("author").cloned().unwrap_or(Value::Null);
    Some(gs(&a, "login"))
        .filter(|s| !s.is_empty())
        .or_else(|| Some(gs(&a, "name")).filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "-".into())
}

fn names(v: &Value, key: &str, field: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|x| gs(x, field))
        .collect()
}

pub fn field_value(entry: &Value, name: &str) -> Option<String> {
    let lower_first = name
        .chars()
        .next()
        .map(|c| c.to_lowercase().collect::<String>() + &name[c.len_utf8()..])
        .unwrap_or_default();
    for k in [name.to_lowercase(), name.to_string(), lower_first] {
        if let Some(v) = entry.get(&k) {
            return v.as_str().map(str::to_string);
        }
    }
    None
}

const BOARD_TONES: &[Tone] = &[
    Tone::Info,
    Tone::Warn,
    Tone::Ok,
    Tone::Special,
    Tone::Accent,
    Tone::Bad,
    Tone::Plain,
];

#[derive(Clone)]
struct Board {
    owner: String,
    number: i64,
    field_name: String,
    field_id: String,
    options: Vec<(String, String)>,
    project_id: String,
}

async fn resolve_board(owner: String, number: i64) -> anyhow::Result<Board> {
    let n = number.to_string();
    let fields = shell::capture_json(
        &[
            "gh",
            "project",
            "field-list",
            &n,
            "--owner",
            &owner,
            "--format",
            "json",
            "--limit",
            "100",
        ],
        20,
    )
    .await?;
    let list: Vec<Value> = fields
        .get("fields")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let has_opts = |f: &&Value| {
        f.get("options")
            .and_then(Value::as_array)
            .is_some_and(|o| !o.is_empty())
    };
    let field = list
        .iter()
        .filter(has_opts)
        .find(|f| gs(f, "name").trim().eq_ignore_ascii_case("status"))
        .or_else(|| list.iter().find(has_opts))
        .context("Kein Status-Feld mit Spalten im Board gefunden")?
        .clone();
    let view = shell::capture_json(
        &[
            "gh", "project", "view", &n, "--owner", &owner, "--format", "json",
        ],
        20,
    )
    .await?;
    let pid = gs(&view, "id");
    if pid.is_empty() {
        bail!("Projekt-ID nicht gefunden");
    }
    Ok(Board {
        owner,
        number,
        field_name: Some(gs(&field, "name"))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Status".into()),
        field_id: gs(&field, "id"),
        options: field
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|o| (gs(o, "name"), gs(o, "id")))
            .collect(),
        project_id: pid,
    })
}

#[derive(Clone, Default)]
struct IssueDetail {
    title: String,
    body: String,
    state: String,
    labels: Vec<String>,
    assignees: Vec<String>,
    all_labels: Vec<String>,
    all_users: Vec<String>,
}

async fn load_issue(repo: String, n: i64) -> anyhow::Result<IssueDetail> {
    let ns = n.to_string();
    let d = shell::capture_json(
        &[
            "gh",
            "issue",
            "view",
            &ns,
            "-R",
            &repo,
            "--json",
            "number,title,body,state,labels,assignees",
        ],
        20,
    )
    .await?;
    let mut all_labels: Vec<String> = shell::capture_json(
        &[
            "gh", "label", "list", "-R", &repo, "--json", "name", "--limit", "100",
        ],
        20,
    )
    .await
    .map(|v| {
        v.as_array()
            .into_iter()
            .flatten()
            .map(|e| gs(e, "name"))
            .collect()
    })
    .unwrap_or_default();
    all_labels.sort();
    let path = format!("repos/{repo}/assignees");
    let mut all_users: Vec<String> = shell::capture_json(&["gh", "api", &path, "--paginate"], 20)
        .await
        .map(|v| {
            v.as_array()
                .into_iter()
                .flatten()
                .map(|e| gs(e, "login"))
                .collect()
        })
        .unwrap_or_default();
    all_users.sort();
    Ok(IssueDetail {
        title: gs(&d, "title"),
        body: gs(&d, "body"),
        state: gs(&d, "state"),
        labels: names(&d, "labels", "name"),
        assignees: names(&d, "assignees", "login"),
        all_labels,
        all_users,
    })
}

struct IssueEdit {
    number: i64,
    detail: Task<IssueDetail>,
    title: String,
    body: String,
    labels: BTreeSet<String>,
    users: BTreeSet<String>,
    comment: String,
    init: bool,
}

#[derive(PartialEq, Eq, Clone, Copy, Default)]
enum Tab {
    #[default]
    Runs,
    Pulls,
    Issues,
    Board,
}

#[derive(Clone)]
enum Confirm {
    Rerun(String),
    Draft(i64, bool),
    State(i64, bool),
}

#[derive(Default)]
pub struct GitHub {
    repo: Option<String>,
    repos: Task<Vec<String>>,
    tab: Tab,
    list: Task<Vec<Value>>,
    list_key: String,
    selected: Option<usize>,
    out: Option<OutputWindow>,
    op: Task<String>,
    message: Option<(Tone, String)>,
    confirm: Option<Confirm>,
    edit: Option<IssueEdit>,
    boards: Task<Vec<(String, i64)>>,
    board_pick: Option<(String, i64)>,
    board: Task<Board>,
    move_to: String,
}

impl GitHub {
    fn known_repos(cx: &Cx) -> (String, Vec<String>) {
        let Ok(c) = cx.cubi() else {
            return (String::new(), vec![]);
        };
        let default = c.str_in("github", "default_repo").unwrap_or_default();
        let mut repos: Vec<String> = c
            .section("github")
            .get("repos")
            .and_then(|v| v.as_sequence())
            .map(|s| s.iter().filter_map(as_str).collect())
            .unwrap_or_default();
        if !default.is_empty() && !repos.contains(&default) {
            repos.insert(0, default.clone());
        }
        (default, repos)
    }

    fn run_op(&mut self, cx: &Cx, label: String, argv: Vec<String>) {
        self.op.spawn(cx, async move {
            shell::capture_ok(&argv, 30, &format!("{label} fehlgeschlagen")).await?;
            Ok(format!("✔ {label}"))
        });
    }

    fn header(&mut self, ui: &mut Ui, cx: &Cx) {
        let (_, mut known) = Self::known_repos(cx);
        if let Some(extra) = self.repos.value() {
            for r in extra {
                if !known.contains(r) {
                    known.push(r.clone());
                }
            }
        }
        let repo = self.repo.clone().unwrap_or_default();
        ui::page_header(
            ui,
            "GitHub",
            "Workflow-Runs, Pull Requests, Issues und Kanban-Board",
            |ui| {
                if !repo.is_empty() {
                    ui.hyperlink_to("Repo im Browser ↗", format!("https://github.com/{repo}"));
                }
                if ui.button("⟳ Neu laden").clicked() {
                    self.list.at = None;
                }
                let mut pick = None;
                let combo = egui::ComboBox::from_id_salt("ghrepo")
                    .width(260.0)
                    .selected_text(if repo.is_empty() {
                        "Repo wählen"
                    } else {
                        &repo
                    })
                    .show_ui(ui, |ui| {
                        for r in &known {
                            if ui.selectable_label(*r == repo, r).clicked() {
                                pick = Some(r.clone());
                            }
                        }
                        if self.repos.busy() {
                            ui.spinner();
                        }
                    });
                if combo.response.clicked() && self.repos.result.is_none() && !self.repos.busy() {
                    self.repos.spawn(cx, async {
                        let v = shell::capture_json(
                            &[
                                "gh",
                                "repo",
                                "list",
                                "--limit",
                                "100",
                                "--json",
                                "nameWithOwner",
                            ],
                            20,
                        )
                        .await?;
                        Ok(v.as_array()
                            .into_iter()
                            .flatten()
                            .map(|e| gs(e, "nameWithOwner"))
                            .filter(|s| !s.is_empty())
                            .collect())
                    });
                }
                if let Some(p) = pick {
                    self.repo = Some(p);
                    self.list.clear();
                    self.selected = None;
                    self.edit = None;
                }
            },
        );
    }
}

impl super::View for GitHub {
    fn id(&self) -> &'static str {
        "github"
    }
    fn title(&self) -> &'static str {
        "GitHub"
    }
    fn icon(&self) -> &'static str {
        "🐙"
    }
    fn hint(&self) -> &'static str {
        "Runs, Pull Requests, Issues, Kanban-Board"
    }

    fn ui(&mut self, ui: &mut Ui, cx: &Cx) {
        if self.repo.is_none() {
            let (d, r) = Self::known_repos(cx);
            self.repo = Some(d)
                .filter(|s| !s.is_empty())
                .or_else(|| r.first().cloned());
        }
        self.repos.poll();
        if self.op.poll() {
            self.message = Some(match &self.op.result {
                Some(Ok(m)) => (Tone::Ok, m.clone()),
                Some(Err(e)) => (Tone::Bad, e.clone()),
                None => (Tone::Muted, String::new()),
            });
            self.list.at = None;
            if let Some(e) = &mut self.edit {
                e.detail.clear();
                e.init = false;
            }
        }
        self.header(ui, cx);
        let Some(repo) = self.repo.clone() else {
            ui::note_box(
                ui,
                "Kein Repo konfiguriert (github.default_repo oder github.repos in targets.yaml). Oben eins wählen.",
                Tone::Warn,
            );
            return;
        };
        ui.horizontal(|ui| {
            for (t, l) in [
                (Tab::Runs, "Workflow-Runs"),
                (Tab::Pulls, "Pull Requests"),
                (Tab::Issues, "Issues"),
                (Tab::Board, "Kanban-Board"),
            ] {
                if ui
                    .selectable_label(self.tab == t, RichText::new(l).strong())
                    .clicked()
                    && self.tab != t
                {
                    self.tab = t;
                    self.list.clear();
                    self.selected = None;
                    self.edit = None;
                }
            }
        });
        ui.separator();
        if let Some((tone, m)) = &self.message {
            ui::note_box(ui, m, *tone);
        }
        if self.op.busy() {
            ui::spinner_label(ui, "Änderung läuft …");
        }
        let key = format!("{repo}|{}", self.tab as u8);
        if key != self.list_key {
            self.list_key = key;
            self.list.clear();
        }
        match self.tab {
            Tab::Runs => self.runs(ui, cx, &repo),
            Tab::Pulls => self.pulls(ui, cx, &repo),
            Tab::Issues => self.issues(ui, cx, &repo),
            Tab::Board => self.board(ui, cx, &repo),
        }
        self.confirm_dialog(ui, cx, &repo);
        ui::show_output(ui.ctx(), &mut self.out);
    }
}

impl GitHub {
    fn load_list(&mut self, ui: &Ui, cx: &Cx, argv: Vec<String>) {
        let c = cx.clone();
        ui::auto_refresh(ui, &mut self.list, Duration::from_secs(30), |t| {
            t.spawn(&c, async move {
                Ok(shell::capture_json(&argv, 30)
                    .await?
                    .as_array()
                    .cloned()
                    .unwrap_or_default())
            })
        });
    }

    fn runs(&mut self, ui: &mut Ui, cx: &Cx, repo: &str) {
        self.load_list(
            ui,
            cx,
            sv(&[
                "gh", "run", "list", "-R", repo, "--limit", "40", "--json", RUN_FIELDS,
            ]),
        );
        let items = self.list.value().cloned().unwrap_or_default();
        if let Some(e) = self.selected.and_then(|i| items.get(i)) {
            let id = gs(e, "databaseId");
            ui::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(gs(e, "displayTitle")).strong());
                    ui::pill(ui, run_status(e), run_tone(&run_status(e)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.hyperlink_to(
                            "Im Browser ↗",
                            format!("https://github.com/{repo}/actions/runs/{id}"),
                        );
                        if ui.button("Neu starten").clicked() {
                            self.confirm = Some(Confirm::Rerun(id.clone()));
                        }
                        if ui.button("Log fehlgeschlagener Schritte").clicked() {
                            self.out = Some(OutputWindow::new(
                                cx,
                                format!("Run {id} · fehlgeschlagene Schritte"),
                                "Keine fehlgeschlagenen Schritte",
                                sv(&["gh", "run", "view", &id, "-R", repo, "--log-failed"]),
                                60,
                            ));
                        }
                        if ui.button("Details").clicked() {
                            self.out = Some(OutputWindow::new(
                                cx,
                                format!("Run {id}"),
                                "",
                                sv(&["gh", "run", "view", &id, "-R", repo]),
                                30,
                            ));
                        }
                    });
                });
            });
            ui.add_space(6.0);
        }
        if ui::task_status(ui, &self.list, "Lade Runs …") {
            let rows: Vec<Vec<Cell>> = items
                .iter()
                .map(|e| {
                    let st = run_status(e);
                    vec![
                        Cell::plain(gs(e, "workflowName")),
                        Cell::plain(gs(e, "displayTitle")),
                        Cell::mono(gs(e, "headBranch"), Tone::Muted),
                        Cell::muted(gs(e, "event")),
                        Cell::pill(&st, run_tone(&st)),
                        Cell::muted(age(&gs(e, "createdAt"))),
                    ]
                })
                .collect();
            ui::card(ui, |ui| {
                ui::table(
                    ui,
                    TableSpec {
                        id: "runs",
                        headers: &["Workflow", "Titel", "Branch", "Event", "Status", "Alter"],
                        checks: None,
                    },
                    &rows,
                    &mut self.selected,
                );
            });
        }
    }

    fn pulls(&mut self, ui: &mut Ui, cx: &Cx, repo: &str) {
        self.load_list(
            ui,
            cx,
            sv(&[
                "gh", "pr", "list", "-R", repo, "--limit", "40", "--json", PR_FIELDS,
            ]),
        );
        let items = self.list.value().cloned().unwrap_or_default();
        if let Some(e) = self.selected.and_then(|i| items.get(i)) {
            let n = e.get("number").and_then(Value::as_i64).unwrap_or(0);
            let ns = n.to_string();
            let draft = e.get("isDraft").and_then(Value::as_bool).unwrap_or(false);
            ui::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("#{n} {}", gs(e, "title"))).strong());
                    let (r, t) = review(e);
                    ui::pill(ui, r, t);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.hyperlink_to(
                            "Im Browser ↗",
                            format!("https://github.com/{repo}/pull/{n}"),
                        );
                        if ui
                            .button(if draft {
                                "Bereit für Review"
                            } else {
                                "Als Entwurf"
                            })
                            .clicked()
                        {
                            self.confirm = Some(Confirm::Draft(n, draft));
                        }
                        for (label, sub, empty) in [
                            ("Diff", "diff", "Kein Diff"),
                            ("Checks", "checks", "Keine Checks"),
                            ("Details", "view", ""),
                        ] {
                            if ui.button(label).clicked() {
                                self.out = Some(OutputWindow::new(
                                    cx,
                                    format!("PR #{n} · {label}"),
                                    empty,
                                    sv(&["gh", "pr", sub, &ns, "-R", repo]),
                                    60,
                                ));
                            }
                        }
                    });
                });
            });
            ui.add_space(6.0);
        }
        if ui::task_status(ui, &self.list, "Lade Pull Requests …") {
            let rows: Vec<Vec<Cell>> = items
                .iter()
                .map(|e| {
                    let (r, t) = review(e);
                    vec![
                        Cell::muted(format!("#{}", gs(e, "number"))),
                        Cell::plain(gs(e, "title")),
                        Cell::muted(author(e)),
                        Cell::mono(gs(e, "headRefName"), Tone::Muted),
                        Cell::pill(r, t),
                        Cell::muted(age(&gs(e, "updatedAt"))),
                    ]
                })
                .collect();
            ui::card(ui, |ui| {
                ui::table(
                    ui,
                    TableSpec {
                        id: "prs",
                        headers: &["#", "Titel", "Autor", "Branch", "Review", "Alter"],
                        checks: None,
                    },
                    &rows,
                    &mut self.selected,
                );
            });
        }
    }

    fn issues(&mut self, ui: &mut Ui, cx: &Cx, repo: &str) {
        self.load_list(
            ui,
            cx,
            sv(&[
                "gh",
                "issue",
                "list",
                "-R",
                repo,
                "--limit",
                "40",
                "--json",
                ISSUE_FIELDS,
            ]),
        );
        let items = self.list.value().cloned().unwrap_or_default();
        let sel_n = self
            .selected
            .and_then(|i| items.get(i))
            .and_then(|e| e.get("number").and_then(Value::as_i64));
        if let Some(n) = sel_n
            && self.edit.as_ref().is_none_or(|e| e.number != n)
        {
            self.edit = Some(IssueEdit {
                number: n,
                detail: Task::default(),
                title: String::new(),
                body: String::new(),
                labels: BTreeSet::new(),
                users: BTreeSet::new(),
                comment: String::new(),
                init: false,
            });
        }
        let mut show = self.edit.is_some();
        egui::Panel::right("issue-edit")
            .resizable(true)
            .min_size(340.0)
            .default_size(440.0)
            .show_collapsible(ui, &mut show, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.issue_panel(ui, cx, repo));
            });
        egui::CentralPanel::no_frame().show(ui, |ui| {
            if ui::task_status(ui, &self.list, "Lade Issues …") {
                let rows: Vec<Vec<Cell>> = items
                    .iter()
                    .map(|e| {
                        vec![
                            Cell::muted(format!("#{}", gs(e, "number"))),
                            Cell::plain(gs(e, "title")),
                            Cell::muted(author(e)),
                            Cell::tone(names(e, "labels", "name").join(", "), Tone::Info),
                            Cell::muted(age(&gs(e, "updatedAt"))),
                        ]
                    })
                    .collect();
                ui::card(ui, |ui| {
                    ui::table(
                        ui,
                        TableSpec {
                            id: "issues",
                            headers: &["#", "Titel", "Autor", "Labels", "Alter"],
                            checks: None,
                        },
                        &rows,
                        &mut self.selected,
                    );
                });
            }
        });
    }

    fn issue_panel(&mut self, ui: &mut Ui, cx: &Cx, repo: &str) {
        let Some(ed) = &mut self.edit else { return };
        let n = ed.number;
        ed.detail.poll();
        if ed.detail.result.is_none() && !ed.detail.busy() {
            ed.detail.spawn(cx, load_issue(repo.to_string(), n));
        }
        if !ui::task_status(ui, &ed.detail, "Lade Issue …") {
            return;
        }
        let d = ed.detail.value().cloned().unwrap_or_default();
        if !ed.init {
            ed.title = d.title.clone();
            ed.body = d.body.clone();
            ed.labels = d.labels.iter().cloned().collect();
            ed.users = d.assignees.iter().cloned().collect();
            ed.init = true;
        }
        let ns = n.to_string();
        let mut ops: Vec<(String, Vec<String>)> = Vec::new();
        let closed = d.state == "CLOSED";
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Issue #{n}")).strong().size(16.0));
            ui::pill(
                ui,
                if closed { "geschlossen" } else { "offen" },
                if closed { Tone::Muted } else { Tone::Ok },
            );
            ui.hyperlink_to("↗", format!("https://github.com/{repo}/issues/{n}"));
        });
        ui.horizontal(|ui| {
            if ui.button("Details").clicked() {
                self.out = Some(OutputWindow::new(
                    cx,
                    format!("Issue #{n}"),
                    "",
                    sv(&["gh", "issue", "view", &ns, "-R", repo]),
                    30,
                ));
            }
            if ui
                .button(if closed {
                    "Wieder öffnen"
                } else {
                    "Schließen"
                })
                .clicked()
            {
                self.confirm = Some(Confirm::State(n, closed));
            }
        });
        ui.add_space(6.0);
        ui::section_title(ui, "Titel");
        ui.add(egui::TextEdit::singleline(&mut ed.title).desired_width(f32::INFINITY));
        if ed.title.trim() != d.title
            && !ed.title.trim().is_empty()
            && ui.button("Titel speichern").clicked()
        {
            ops.push((
                "Titel geändert".into(),
                sv(&["gh", "issue", "edit", &ns, "-R", repo, "--title", &ed.title]),
            ));
        }
        ui.add_space(6.0);
        ui::section_title(ui, "Beschreibung");
        ui.add(
            egui::TextEdit::multiline(&mut ed.body)
                .desired_rows(6)
                .desired_width(f32::INFINITY),
        );
        if ed.body != d.body && ui.button("Beschreibung speichern").clicked() {
            ops.push((
                "Beschreibung geändert".into(),
                sv(&["gh", "issue", "edit", &ns, "-R", repo, "--body", &ed.body]),
            ));
        }
        ui.add_space(6.0);
        ui::section_title(ui, "Labels");
        ui.horizontal_wrapped(|ui| {
            for l in &d.all_labels {
                let mut on = ed.labels.contains(l);
                if ui.checkbox(&mut on, l).changed() {
                    if on {
                        ed.labels.insert(l.clone());
                    } else {
                        ed.labels.remove(l);
                    }
                }
            }
            if d.all_labels.is_empty() {
                ui.label(RichText::new("Keine Labels verfügbar").color(ui::theme::MUTED));
            }
        });
        let have: BTreeSet<String> = d.labels.iter().cloned().collect();
        if ed.labels != have && ui.button("Labels übernehmen").clicked() {
            let mut argv = sv(&["gh", "issue", "edit", &ns, "-R", repo]);
            for a in ed.labels.difference(&have) {
                argv.extend(["--add-label".into(), a.clone()]);
            }
            for r in have.difference(&ed.labels) {
                argv.extend(["--remove-label".into(), r.clone()]);
            }
            ops.push(("Labels geändert".into(), argv));
        }
        ui.add_space(6.0);
        ui::section_title(ui, "Zugewiesen");
        let mut users: Vec<String> = d.all_users.clone();
        for a in &d.assignees {
            if !users.contains(a) {
                users.push(a.clone());
            }
        }
        ui.horizontal_wrapped(|ui| {
            for u in &users {
                let mut on = ed.users.contains(u);
                if ui.checkbox(&mut on, u).changed() {
                    if on {
                        ed.users.insert(u.clone());
                    } else {
                        ed.users.remove(u);
                    }
                }
            }
        });
        let have: BTreeSet<String> = d.assignees.iter().cloned().collect();
        if ed.users != have && ui.button("Zuweisung übernehmen").clicked() {
            let mut argv = sv(&["gh", "issue", "edit", &ns, "-R", repo]);
            for a in ed.users.difference(&have) {
                argv.extend(["--add-assignee".into(), a.clone()]);
            }
            for r in have.difference(&ed.users) {
                argv.extend(["--remove-assignee".into(), r.clone()]);
            }
            ops.push(("Zuweisung geändert".into(), argv));
        }
        ui.add_space(6.0);
        ui::section_title(ui, "Kommentar");
        ui.add(
            egui::TextEdit::multiline(&mut ed.comment)
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .hint_text("Neuer Kommentar …"),
        );
        if !ed.comment.trim().is_empty() && ui.add(ui::primary_button("Kommentieren")).clicked() {
            ops.push((
                "Kommentar hinzugefügt".into(),
                sv(&[
                    "gh",
                    "issue",
                    "comment",
                    &ns,
                    "-R",
                    repo,
                    "--body",
                    &ed.comment,
                ]),
            ));
            ed.comment.clear();
        }
        for (label, argv) in ops {
            self.run_op(cx, format!("#{n}: {label}"), argv);
        }
    }

    fn board(&mut self, ui: &mut Ui, cx: &Cx, repo: &str) {
        self.boards.poll();
        self.board.poll();
        let configured = cx.cubi().ok().and_then(|c| {
            let b = c.section("github").get("board").cloned()?;
            let owner = b.get("owner").and_then(as_str)?;
            let number = b
                .get("number")
                .and_then(|v| v.as_i64().or_else(|| as_str(v)?.parse().ok()))?;
            Some((owner, number))
        });
        let owner = repo.split('/').next().unwrap_or_default().to_string();
        let pick = configured.clone().or_else(|| self.board_pick.clone());
        if configured.is_none() {
            if self.boards.result.is_none() && !self.boards.busy() {
                let o = owner.clone();
                self.boards.spawn(cx, async move {
                    let v = shell::capture_json(
                        &[
                            "gh", "project", "list", "--owner", &o, "--format", "json", "--limit",
                            "100",
                        ],
                        20,
                    )
                    .await?;
                    let list: Vec<(String, i64)> = v
                        .get("projects")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|p| Some((gs(p, "title"), p.get("number")?.as_i64()?)))
                        .collect();
                    if list.is_empty() {
                        bail!("Keine Projekte für {o} gefunden");
                    }
                    Ok(list)
                });
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("Board").color(ui::theme::MUTED));
                let label = self
                    .board_pick
                    .as_ref()
                    .map_or("wählen …".to_string(), |(_, n)| {
                        self.boards
                            .value()
                            .and_then(|b| b.iter().find(|x| x.1 == *n))
                            .map_or(format!("#{n}"), |x| format!("{} #{}", x.0, x.1))
                    });
                egui::ComboBox::from_id_salt("board")
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        for (t, n) in self.boards.value().cloned().unwrap_or_default() {
                            if ui
                                .selectable_label(
                                    self.board_pick.as_ref().is_some_and(|b| b.1 == n),
                                    format!("{t} #{n}"),
                                )
                                .clicked()
                            {
                                self.board_pick = Some((owner.clone(), n));
                                self.board.clear();
                                self.list.clear();
                            }
                        }
                    });
                if let Some(e) = self.boards.error() {
                    ui.colored_label(ui::theme::RED, e);
                }
            });
        }
        let Some((bo, bn)) = pick else {
            ui.label(
                RichText::new("Board wählen oder github.board in targets.yaml setzen.")
                    .color(ui::theme::MUTED),
            );
            return;
        };
        if self.board.result.is_none() && !self.board.busy() {
            self.board.spawn(cx, resolve_board(bo.clone(), bn));
        }
        if !ui::task_status(ui, &self.board, "Lade Board …") {
            return;
        }
        let b = self.board.value().cloned().unwrap();
        let bns = b.number.to_string();
        let o = b.owner.clone();
        let c = cx.clone();
        ui::auto_refresh(ui, &mut self.list, Duration::from_secs(30), |t| {
            t.spawn(&c, async move {
                let v = shell::capture_json(
                    &[
                        "gh",
                        "project",
                        "item-list",
                        &bns,
                        "--owner",
                        &o,
                        "--format",
                        "json",
                        "--limit",
                        "200",
                    ],
                    30,
                )
                .await?;
                Ok(v.get("items")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|e| {
                        !gs(e, "id").is_empty()
                            && e.pointer("/content/type").and_then(Value::as_str) == Some("Issue")
                    })
                    .cloned()
                    .collect())
            })
        });
        let order: Vec<String> = b.options.iter().map(|o| o.0.clone()).collect();
        let mut items = self.list.value().cloned().unwrap_or_default();
        items.sort_by_key(|e| {
            field_value(e, &b.field_name)
                .and_then(|v| order.iter().position(|o| *o == v))
                .unwrap_or(order.len())
        });
        ui.horizontal(|ui| {
            ui::pill(ui, format!("{} Einträge", items.len()), Tone::Muted);
            for (i, o) in order.iter().enumerate() {
                let n = items
                    .iter()
                    .filter(|e| field_value(e, &b.field_name).as_deref() == Some(o))
                    .count();
                ui::pill(ui, format!("{o} {n}"), BOARD_TONES[i % BOARD_TONES.len()]);
            }
        });
        ui.add_space(4.0);
        if let Some(e) = self.selected.and_then(|i| items.get(i)).cloned() {
            let item_id = gs(&e, "id");
            let content = e.get("content").cloned().unwrap_or(Value::Null);
            let irepo = Some(gs(&content, "repository"))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| repo.to_string());
            let num = gs(&content, "number");
            let current = field_value(&e, &b.field_name).unwrap_or_default();
            ui::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("#{num} {}", gs(&content, "title"))).strong());
                    ui.label("Spalte");
                    egui::ComboBox::from_id_salt("move")
                        .selected_text(if self.move_to.is_empty() {
                            current.clone()
                        } else {
                            self.move_to.clone()
                        })
                        .show_ui(ui, |ui| {
                            for o in &order {
                                ui.selectable_value(&mut self.move_to, o.clone(), o);
                            }
                        });
                    if !self.move_to.is_empty()
                        && self.move_to != current
                        && ui.add(ui::primary_button("Verschieben")).clicked()
                    {
                        if let Some((_, oid)) = b.options.iter().find(|o| o.0 == self.move_to) {
                            self.run_op(
                                cx,
                                format!("#{num} nach {} verschoben", self.move_to),
                                sv(&[
                                    "gh",
                                    "project",
                                    "item-edit",
                                    "--id",
                                    &item_id,
                                    "--field-id",
                                    &b.field_id,
                                    "--project-id",
                                    &b.project_id,
                                    "--single-select-option-id",
                                    oid,
                                ]),
                            );
                        }
                        self.move_to.clear();
                    }
                    if !num.is_empty() {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.hyperlink_to(
                                "Im Browser ↗",
                                format!("https://github.com/{irepo}/issues/{num}"),
                            );
                            if ui.button("Details").clicked() {
                                self.out = Some(OutputWindow::new(
                                    cx,
                                    format!("Issue #{num}"),
                                    "",
                                    sv(&["gh", "issue", "view", &num, "-R", &irepo]),
                                    30,
                                ));
                            }
                        });
                    }
                });
            });
            ui.add_space(6.0);
        }
        if ui::task_status(ui, &self.list, "Lade Einträge …") {
            let rows: Vec<Vec<Cell>> = items
                .iter()
                .map(|e| {
                    let st = field_value(e, &b.field_name).unwrap_or_else(|| "-".into());
                    let idx = order.iter().position(|o| *o == st).unwrap_or(order.len());
                    let c = e.get("content").cloned().unwrap_or(Value::Null);
                    let labels: Vec<String> = e
                        .get("labels")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|l| l.as_str().map(str::to_string))
                        .collect();
                    vec![
                        Cell::pill(&st, BOARD_TONES[idx % BOARD_TONES.len()]),
                        Cell::muted(format!("#{}", gs(&c, "number"))),
                        Cell::plain(
                            Some(gs(&c, "title"))
                                .filter(|s| !s.is_empty())
                                .unwrap_or_else(|| gs(e, "title")),
                        ),
                        Cell::muted(gs(&c, "repository")),
                        Cell::tone(labels.join(", "), Tone::Info),
                    ]
                })
                .collect();
            let before = self.selected;
            ui::card(ui, |ui| {
                ui::table(
                    ui,
                    TableSpec {
                        id: "board",
                        headers: &["Status", "#", "Titel", "Repo", "Labels"],
                        checks: None,
                    },
                    &rows,
                    &mut self.selected,
                );
            });
            if before != self.selected {
                self.move_to.clear();
            }
        }
    }

    fn confirm_dialog(&mut self, ui: &mut Ui, cx: &Cx, repo: &str) {
        let Some(c) = self.confirm.clone() else {
            return;
        };
        let (title, yes, label, argv) = match &c {
            Confirm::Rerun(id) => (
                format!("Run {id} neu starten?"),
                "Neu starten",
                format!("Run {id} neu gestartet"),
                sv(&["gh", "run", "rerun", id, "-R", repo]),
            ),
            Confirm::Draft(n, true) => (
                format!("PR #{n} als bereit für Review markieren?"),
                "Bereit",
                format!("PR #{n} bereit für Review"),
                sv(&["gh", "pr", "ready", &n.to_string(), "-R", repo]),
            ),
            Confirm::Draft(n, false) => (
                format!("PR #{n} als Entwurf markieren?"),
                "Als Entwurf",
                format!("PR #{n} ist Entwurf"),
                sv(&["gh", "pr", "ready", &n.to_string(), "-R", repo, "--undo"]),
            ),
            Confirm::State(n, true) => (
                format!("Issue #{n} wieder öffnen?"),
                "Öffnen",
                format!("Issue #{n} geöffnet"),
                sv(&["gh", "issue", "reopen", &n.to_string(), "-R", repo]),
            ),
            Confirm::State(n, false) => (
                format!("Issue #{n} schließen?"),
                "Schließen",
                format!("Issue #{n} geschlossen"),
                sv(&["gh", "issue", "close", &n.to_string(), "-R", repo]),
            ),
        };
        match ui::confirm(
            ui.ctx(),
            "gh-confirm",
            &title,
            &[],
            yes,
            matches!(c, Confirm::State(_, false)),
        ) {
            Some(true) => {
                self.confirm = None;
                self.run_op(cx, label, argv);
            }
            Some(false) => self.confirm = None,
            None => {}
        }
    }
}

pub async fn commits(
    repo: String,
    path: String,
    revision: String,
) -> anyhow::Result<Vec<(String, String, String, String)>> {
    let mut q = format!("repos/{repo}/commits?per_page=30");
    let enc = |s: &str| -> String {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect()
    };
    if !path.is_empty() {
        q += &format!("&path={}", enc(&path));
    }
    if !revision.is_empty() && revision != "HEAD" {
        q += &format!("&sha={}", enc(&revision));
    }
    let v = shell::capture_json(&["gh", "api", &q], 30).await?;
    Ok(v.as_array()
        .into_iter()
        .flatten()
        .map(|e| {
            let sha = gs(e, "sha");
            let a = e.pointer("/commit/author").cloned().unwrap_or(Value::Null);
            let msg = e
                .pointer("/commit/message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .lines()
                .next()
                .unwrap_or_default()
                .to_string();
            (
                sha,
                age(&gs(&a, "date")),
                Some(gs(&a, "name"))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "-".into()),
                msg,
            )
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        let v: Value =
            serde_json::from_str(r#"{"status":"completed","conclusion":"failure"}"#).unwrap();
        assert_eq!(run_status(&v), "failure");
        assert_eq!(run_tone("failure"), Tone::Bad);
        let v: Value = serde_json::from_str(r#"{"status":"in_progress","conclusion":""}"#).unwrap();
        assert_eq!(run_status(&v), "in_progress");
        let pr: Value =
            serde_json::from_str(r#"{"isDraft":false,"reviewDecision":"APPROVED"}"#).unwrap();
        assert_eq!(review(&pr), ("approved", Tone::Ok));
        let e: Value = serde_json::from_str(r#"{"status":"Done"}"#).unwrap();
        assert_eq!(field_value(&e, "Status").as_deref(), Some("Done"));
        let e: Value = serde_json::from_str(r#"{"priorityLevel":"P1"}"#).unwrap();
        assert_eq!(field_value(&e, "PriorityLevel").as_deref(), Some("P1"));
    }
}
