use eframe::egui::{self, RichText, Ui};

use crate::cubi_config::CubiConfig;
use crate::tokens::{self, RULES, Token};
use crate::ui::{self, Cell, Cx, OutputWindow, TableSpec, Task, Tone};

pub fn due_cell(t: &Token, today: chrono::NaiveDate) -> (String, Tone) {
    match (t.due(), t.days_left(today)) {
        (Some(d), Some(l)) if l < 0 => (format!("{d} · seit {} d abgelaufen", -l), Tone::Bad),
        (Some(d), Some(l)) => (
            format!("{d} · in {l} d"),
            if l <= tokens::URGENT {
                Tone::Bad
            } else if l <= tokens::SOON {
                Tone::Warn
            } else {
                Tone::Ok
            },
        ),
        _ => ("unbekannt".into(), Tone::Warn),
    }
}

fn mode_tone(m: &str) -> Tone {
    match m {
        "auto" => Tone::Ok,
        "scripted" => Tone::Info,
        _ => Tone::Plain,
    }
}

#[derive(Default)]
struct MarkForm {
    issued: String,
    expires: String,
    error: Option<String>,
}

#[derive(Default)]
pub struct Tokens {
    list: Task<Vec<Token>>,
    selected: Option<usize>,
    out: Option<OutputWindow>,
    mark: Option<MarkForm>,
    confirm_script: bool,
    message: Option<(Tone, String)>,
}

fn cfg(cx: &Cx) -> Option<CubiConfig> {
    cx.cubi().ok().cloned()
}

impl Tokens {
    fn detail(&mut self, ui: &mut Ui, cx: &Cx, t: &Token) {
        let today = tokens::today();
        ui::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(&t.id)
                        .strong()
                        .size(16.0)
                        .color(ui::theme::STRONG),
                );
                ui::pill(ui, &t.mode, mode_tone(&t.mode));
                let (d, tone) = due_cell(t, today);
                ui::pill(ui, d, tone);
            });
            if !t.blast_radius.is_empty() {
                ui.label(RichText::new(&t.blast_radius).color(ui::theme::MUTED));
            }
            ui.add_space(6.0);
            egui::Grid::new("tokgrid")
                .num_columns(2)
                .spacing([16.0, 4.0])
                .show(ui, |ui| {
                    let mut row = |k: &str, v: String| {
                        ui.label(RichText::new(k).color(ui::theme::MUTED));
                        ui.label(v);
                        ui.end_row();
                    };
                    row("Dienst", format!("{} · {}", t.service, t.kind));
                    row(
                        "Ausgestellt",
                        t.issued
                            .map_or_else(|| "unbekannt".into(), |d| d.to_string()),
                    );
                    if let Some(l) = &t.live_check {
                        row("Live-Prüfung", format!("{l} (token-watch)"));
                    }
                    row(
                        "Runbook",
                        if t.runbook.is_empty() {
                            "-".into()
                        } else {
                            t.runbook.clone()
                        },
                    );
                    row(
                        "Abgelegt in",
                        if t.stored_in.is_empty() {
                            "-".into()
                        } else {
                            t.stored_in.join("\n")
                        },
                    );
                });
            ui.add_space(8.0);
            ui.collapsing(RichText::new("Rotation").strong(), |ui| {
                if t.mode == "auto" {
                    ui::note_box(
                        ui,
                        &format!(
                            "{} rotiert automatisch, siehe Runbook ({}). Nichts zu tun.",
                            t.id, t.runbook
                        ),
                        Tone::Ok,
                    );
                    return;
                }
                if !t.hint.is_empty() {
                    ui.label(&t.hint);
                }
                if t.overlap {
                    for (i, r) in RULES.iter().enumerate() {
                        ui.label(format!("{}. {r}", i + 1));
                    }
                } else {
                    ui::note_box(
                        ui,
                        "Keine Überlappung möglich: Das Token wird in einem Schritt getauscht.",
                        Tone::Warn,
                    );
                }
                if t.mode == "scripted" {
                    ui.add_space(4.0);
                    if t.command.is_empty() {
                        ui::error_box(ui, &format!("{}: Im Register fehlt rotation.command", t.id));
                    } else {
                        ui.label(
                            RichText::new(t.command.join(" "))
                                .monospace()
                                .color(ui::theme::MUTED),
                        );
                        if ui
                            .add(ui::primary_button("Skript im Terminal ausführen"))
                            .clicked()
                        {
                            self.confirm_script = true;
                        }
                    }
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Runbook lesen").clicked() {
                    let c = cfg(cx);
                    let rb = t.runbook.clone();
                    let id = t.id.clone();
                    self.out = Some(OutputWindow::from_future(
                        cx,
                        format!("Runbook · {id}"),
                        async move {
                            let (p, anchor) = rb.split_once('#').unwrap_or((&rb, ""));
                            let path = tokens::resolve_runbook(c.as_ref(), p)?;
                            if p.is_empty() || !path.is_file() {
                                anyhow::bail!(
                                    "Runbook ist keine Datei im Repo: {}",
                                    if rb.is_empty() { "-" } else { &rb }
                                );
                            }
                            Ok(tokens::runbook_text(
                                &std::fs::read_to_string(path)?,
                                anchor,
                            ))
                        },
                    ));
                }
                if ui.button("Als rotiert eintragen").clicked() {
                    self.mark = Some(MarkForm {
                        issued: today.to_string(),
                        ..Default::default()
                    });
                }
            });
            if let Some(mut form) = self.mark.take() {
                ui.add_space(6.0);
                let mut close = false;
                ui::card(ui, |ui| {
                    egui::Grid::new("markgrid").num_columns(2).show(ui, |ui| {
                        ui.label("Ausgestellt am (JJJJ-MM-TT)");
                        ui.text_edit_singleline(&mut form.issued);
                        ui.end_row();
                        ui.label(if t.max_age_days.is_some() {
                            "Läuft ab am (leer = ausgestellt + max_age_days)"
                        } else {
                            "Läuft ab am (leer = unbekannt)"
                        });
                        ui.text_edit_singleline(&mut form.expires);
                        ui.end_row();
                    });
                    if let Some(e) = &form.error {
                        ui::error_box(ui, e);
                    }
                    ui.horizontal(|ui| {
                        if ui.add(ui::primary_button("Im Register speichern")).clicked() {
                            let r = (|| {
                                let issued = tokens::validate_date(&form.issued)?;
                                let expires = if form.expires.trim().is_empty() { None } else { Some(tokens::validate_date(&form.expires)?) };
                                tokens::set_dates(cfg(cx).as_ref(), &t.id, issued, expires)
                            })();
                            match r {
                                Ok(p) => {
                                    self.message = Some((Tone::Ok, format!("Register aktualisiert: {}. Änderung committen, ein PR pro Rotation. Danach prüft token-watch das neue Datum.", p.display())));
                                    close = true;
                                    self.list.clear();
                                }
                                Err(e) => form.error = Some(format!("{e:#}")),
                            }
                        }
                        if ui.button("Abbrechen").clicked() {
                            close = true;
                        }
                    });
                });
                if !close {
                    self.mark = Some(form);
                }
            }
        });
    }
}

impl super::View for Tokens {
    fn id(&self) -> &'static str {
        "tokens"
    }
    fn title(&self) -> &'static str {
        "Tokens"
    }
    fn icon(&self) -> &'static str {
        "🔐"
    }
    fn hint(&self) -> &'static str {
        "Ablauf und Rotation aus dem Token-Register"
    }

    fn ui(&mut self, ui: &mut Ui, cx: &Cx) {
        self.list.poll();
        if self.list.result.is_none() && !self.list.busy() {
            let c = cfg(cx);
            self.list.spawn(cx, async move {
                let today = tokens::today();
                let mut t = tokens::load(c.as_ref())?;
                t.sort_by_key(|x| {
                    (
                        x.urgency(today),
                        x.days_left(today).unwrap_or(0),
                        x.id.clone(),
                    )
                });
                Ok(t)
            });
        }
        ui::page_header(ui, "Tokens", "Token-Register von capulus-core", |ui| {
            if ui.button("⟳ Neu laden").clicked() {
                self.list.clear();
            }
            if ui.button("Register prüfen").clicked() {
                let c = cfg(cx);
                self.out = Some(OutputWindow::from_future(
                    cx,
                    "Register prüfen",
                    async move {
                        let root = tokens::repo_root(c.as_ref())?;
                        let o = crate::shell::capture_in(
                            &["python3", "scripts/check-token-register.py"],
                            30,
                            Some(&root),
                            &[],
                        )
                        .await?;
                        let t = crate::shell::merge_output(&o);
                        Ok(if t.is_empty() {
                            "(keine Ausgabe)".into()
                        } else {
                            t
                        })
                    },
                ));
            }
        });
        if let Some((tone, m)) = &self.message {
            ui::note_box(ui, m, *tone);
            ui.add_space(4.0);
        }
        if !ui::task_status(ui, &self.list, "Lese Token-Register …") {
            ui::show_output(ui.ctx(), &mut self.out);
            return;
        }
        let list = self.list.value().cloned().unwrap_or_default();
        let today = tokens::today();
        ui.horizontal(|ui| {
            let n = |u: u8| list.iter().filter(|t| t.urgency(today) == u).count();
            ui::stat(ui, "Tokens", list.len().to_string(), Tone::Plain);
            ui::stat(
                ui,
                "Dringend",
                n(0).to_string(),
                if n(0) > 0 { Tone::Bad } else { Tone::Muted },
            );
            ui::stat(
                ui,
                "Bald fällig",
                n(2).to_string(),
                if n(2) > 0 { Tone::Warn } else { Tone::Muted },
            );
            ui::stat(ui, "Ohne Ablaufdatum", n(1).to_string(), Tone::Muted);
        });
        ui.add_space(8.0);
        let rows: Vec<Vec<Cell>> = list
            .iter()
            .map(|t| {
                let (d, tone) = due_cell(t, today);
                vec![
                    Cell::plain(&t.id),
                    Cell::muted(&t.service),
                    Cell::pill(&t.mode, mode_tone(&t.mode)),
                    Cell::muted(t.issued.map_or("-".into(), |i| {
                        format!("{i} · {} d", (today - i).num_days())
                    })),
                    Cell::tone(d, tone),
                ]
            })
            .collect();
        let selected = self.selected.and_then(|i| list.get(i)).cloned();
        egui::Panel::right("tokdetail")
            .resizable(true)
            .min_size(340.0)
            .default_size(420.0)
            .show_collapsible(ui, &mut (selected.is_some()), |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if let Some(t) = &selected {
                        self.detail(ui, cx, t);
                    }
                });
            });
        egui::CentralPanel::no_frame().show(ui, |ui| {
            ui::card(ui, |ui| {
                let before = self.selected;
                ui::table(
                    ui,
                    TableSpec {
                        id: "tokens",
                        headers: &["Token", "Dienst", "Rotation", "Ausgestellt", "Läuft ab"],
                        checks: None,
                    },
                    &rows,
                    &mut self.selected,
                );
                if before != self.selected {
                    self.mark = None;
                }
            });
        });
        if self.confirm_script
            && let Some(t) = &selected
        {
            match ui::confirm(
                ui.ctx(),
                "tok-script",
                "Skript ausführen?",
                &[
                    t.command.join(" "),
                    "Das Skript läuft im Terminal im capulus-core-Checkout.".into(),
                ],
                "Ausführen",
                false,
            ) {
                Some(true) => {
                    self.confirm_script = false;
                    match tokens::repo_root(cfg(cx).as_ref()) {
                        Ok(root) => {
                            let mut argv = vec![root.join(&t.command[0]).display().to_string()];
                            argv.extend(t.command[1..].iter().cloned());
                            let full = [
                                vec!["env".to_string(), "-C".into(), root.display().to_string()],
                                argv,
                            ]
                            .concat();
                            if let Err(e) = cx.terminal(full) {
                                self.message = Some((Tone::Bad, e));
                            } else {
                                self.message = Some((
                                    Tone::Info,
                                    if t.overlap {
                                        "Skript läuft im Terminal. Das alte Token ist noch gültig: erst nach der Prüfung widerrufen, dann „Als rotiert eintragen“.".into()
                                    } else {
                                        "Skript läuft im Terminal. Danach „Als rotiert eintragen“."
                                            .into()
                                    },
                                ));
                            }
                        }
                        Err(e) => self.message = Some((Tone::Bad, format!("{e:#}"))),
                    }
                }
                Some(false) => self.confirm_script = false,
                None => {}
            }
        }
        ui::show_output(ui.ctx(), &mut self.out);
    }
}
