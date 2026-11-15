use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Frame, Margin, RichText, Stroke, TextStyle, Ui,
};
use egui_extras::{Column, TableBuilder};
use tokio::sync::oneshot;

use crate::cubi_config::CubiConfig;

pub mod theme {
    use super::Color32;
    pub const BG: Color32 = Color32::from_rgb(0x0f, 0x12, 0x17);
    pub const PANEL: Color32 = Color32::from_rgb(0x14, 0x18, 0x1f);
    pub const CARD: Color32 = Color32::from_rgb(0x1a, 0x20, 0x29);
    pub const RAISED: Color32 = Color32::from_rgb(0x22, 0x2a, 0x35);
    pub const BORDER: Color32 = Color32::from_rgb(0x2a, 0x33, 0x40);
    pub const TEXT: Color32 = Color32::from_rgb(0xd8, 0xde, 0xe6);
    pub const STRONG: Color32 = Color32::from_rgb(0xf3, 0xf6, 0xfa);
    pub const MUTED: Color32 = Color32::from_rgb(0x86, 0x92, 0xa2);
    pub const ACCENT: Color32 = Color32::from_rgb(0x5b, 0x9d, 0xff);
    pub const GREEN: Color32 = Color32::from_rgb(0x3f, 0xb9, 0x80);
    pub const YELLOW: Color32 = Color32::from_rgb(0xe6, 0xad, 0x3c);
    pub const RED: Color32 = Color32::from_rgb(0xef, 0x5f, 0x58);
    pub const CYAN: Color32 = Color32::from_rgb(0x4c, 0xc6, 0xcf);
    pub const MAGENTA: Color32 = Color32::from_rgb(0xc7, 0x7d, 0xe0);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    Ok,
    Warn,
    Bad,
    Info,
    Accent,
    Special,
    Muted,
    Plain,
}

impl Tone {
    pub fn color(self) -> Color32 {
        match self {
            Tone::Ok => theme::GREEN,
            Tone::Warn => theme::YELLOW,
            Tone::Bad => theme::RED,
            Tone::Info => theme::CYAN,
            Tone::Accent => theme::ACCENT,
            Tone::Special => theme::MAGENTA,
            Tone::Muted => theme::MUTED,
            Tone::Plain => theme::TEXT,
        }
    }
}

pub fn apply_theme(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        let v = &mut style.visuals;
        v.panel_fill = theme::BG;
        v.window_fill = theme::CARD;
        v.extreme_bg_color = Color32::from_rgb(0x0b, 0x0e, 0x12);
        v.faint_bg_color = Color32::from_rgb(0x17, 0x1c, 0x24);
        v.code_bg_color = Color32::from_rgb(0x0b, 0x0e, 0x12);
        v.hyperlink_color = theme::ACCENT;
        v.override_text_color = Some(theme::TEXT);
        v.window_corner_radius = CornerRadius::same(10);
        v.window_stroke = Stroke::new(1.0, theme::BORDER);
        v.selection.bg_fill = theme::ACCENT.gamma_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, theme::ACCENT);
        v.striped = true;
        for (w, fill) in [
            (&mut v.widgets.noninteractive, theme::CARD),
            (&mut v.widgets.inactive, theme::RAISED),
            (&mut v.widgets.hovered, Color32::from_rgb(0x2c, 0x36, 0x44)),
            (&mut v.widgets.active, Color32::from_rgb(0x33, 0x3f, 0x50)),
            (&mut v.widgets.open, theme::RAISED),
        ] {
            w.corner_radius = CornerRadius::same(6);
            w.bg_fill = fill;
            w.weak_bg_fill = fill;
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, theme::BORDER);
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, theme::BORDER);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.7));
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.interact_size.y = 26.0;
        style
            .text_styles
            .insert(TextStyle::Body, FontId::proportional(14.0));
        style
            .text_styles
            .insert(TextStyle::Button, FontId::proportional(14.0));
        style
            .text_styles
            .insert(TextStyle::Heading, FontId::proportional(21.0));
        style
            .text_styles
            .insert(TextStyle::Small, FontId::proportional(12.0));
        style
            .text_styles
            .insert(TextStyle::Monospace, FontId::monospace(13.0));
    });
}

#[derive(Clone)]
pub struct Cx {
    pub rt: tokio::runtime::Handle,
    pub egui: egui::Context,
    pub cubi: Arc<Result<CubiConfig, String>>,
    pub terminal: Arc<Vec<String>>,
}

impl Cx {
    pub fn cubi(&self) -> Result<&CubiConfig, String> {
        self.cubi.as_ref().as_ref().map_err(Clone::clone)
    }

    pub fn terminal(&self, argv: Vec<String>) -> Result<(), String> {
        crate::shell::open_terminal(&self.terminal, &argv, &[]).map_err(|e| format!("{e:#}"))
    }
}

pub struct Task<T> {
    rx: Option<oneshot::Receiver<Result<T, String>>>,
    pub result: Option<Result<T, String>>,
    pub at: Option<Instant>,
}

impl<T> Default for Task<T> {
    fn default() -> Self {
        Self {
            rx: None,
            result: None,
            at: None,
        }
    }
}

impl<T: Send + 'static> Task<T> {
    pub fn spawn<F>(&mut self, cx: &Cx, fut: F)
    where
        F: Future<Output = anyhow::Result<T>> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let ctx = cx.egui.clone();
        cx.rt.spawn(async move {
            let r = fut.await.map_err(|e| format!("{e:#}"));
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        self.rx = Some(rx);
    }

    pub fn poll(&mut self) -> bool {
        let Some(rx) = &mut self.rx else { return false };
        match rx.try_recv() {
            Ok(r) => {
                self.result = Some(r);
                self.rx = None;
                self.at = Some(Instant::now());
                true
            }
            Err(oneshot::error::TryRecvError::Empty) => false,
            Err(oneshot::error::TryRecvError::Closed) => {
                self.result = Some(Err("Aufgabe abgebrochen".into()));
                self.rx = None;
                true
            }
        }
    }

    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }

    pub fn value(&self) -> Option<&T> {
        self.result.as_ref().and_then(|r| r.as_ref().ok())
    }

    pub fn error(&self) -> Option<&str> {
        self.result
            .as_ref()
            .and_then(|r| r.as_ref().err())
            .map(String::as_str)
    }

    pub fn due(&self, every: Duration) -> bool {
        !self.busy() && self.at.is_none_or(|t| t.elapsed() >= every)
    }

    pub fn clear(&mut self) {
        self.rx = None;
        self.result = None;
        self.at = None;
    }
}

pub fn auto_refresh<T: Send + 'static>(
    ui: &Ui,
    task: &mut Task<T>,
    every: Duration,
    start: impl FnOnce(&mut Task<T>),
) {
    task.poll();
    if task.due(every) {
        start(task);
    }
    ui.ctx().request_repaint_after(Duration::from_secs(1));
}

pub fn pill(ui: &mut Ui, text: impl Into<String>, tone: Tone) -> egui::Response {
    let c = tone.color();
    let galley = ui
        .painter()
        .layout_no_wrap(text.into(), FontId::proportional(12.5), c);
    let pad = egui::vec2(8.0, 3.0);
    let size = galley.size() + pad * 2.0;
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        p.rect(
            rect,
            CornerRadius::same(9),
            c.gamma_multiply(0.16),
            Stroke::new(1.0, c.gamma_multiply(0.45)),
            egui::StrokeKind::Inside,
        );
        p.galley(rect.min + pad, galley, c);
    }
    resp
}

pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new()
        .fill(theme::CARD)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(14))
        .show(ui, add)
        .inner
}

pub fn section_title(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text.to_uppercase())
            .size(11.5)
            .color(theme::MUTED)
            .strong(),
    );
}

pub fn page_header(ui: &mut Ui, title: &str, subtitle: &str, right: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(title).heading().color(theme::STRONG).strong());
            if !subtitle.is_empty() {
                ui.label(RichText::new(subtitle).color(theme::MUTED));
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
    });
    ui.add_space(6.0);
}

pub fn stat(ui: &mut Ui, label: &str, value: impl Into<String>, tone: Tone) {
    Frame::new()
        .fill(theme::CARD)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_min_width(110.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(label).size(12.0).color(theme::MUTED));
                ui.label(
                    RichText::new(value.into())
                        .size(22.0)
                        .strong()
                        .color(tone.color()),
                );
            });
        });
}

pub fn primary_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.into()).color(Color32::WHITE).strong())
        .fill(theme::ACCENT.gamma_multiply(0.85))
}

pub fn danger_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.into()).color(Color32::WHITE).strong())
        .fill(theme::RED.gamma_multiply(0.75))
}

pub fn spinner_label(ui: &mut Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.spinner();
        ui.label(RichText::new(text).color(theme::MUTED));
    });
}

pub fn error_box(ui: &mut Ui, text: &str) {
    Frame::new()
        .fill(theme::RED.gamma_multiply(0.12))
        .stroke(Stroke::new(1.0, theme::RED.gamma_multiply(0.5)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(format!("✖ {text}")).color(theme::RED));
        });
}

pub fn note_box(ui: &mut Ui, text: &str, tone: Tone) {
    let c = tone.color();
    Frame::new()
        .fill(c.gamma_multiply(0.10))
        .stroke(Stroke::new(1.0, c.gamma_multiply(0.4)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).color(c));
        });
}

pub fn task_status<T: Send + 'static>(ui: &mut Ui, task: &Task<T>, loading: &str) -> bool {
    if let Some(e) = task.error() {
        error_box(ui, e);
        return false;
    }
    if task.busy() && task.result.is_none() {
        spinner_label(ui, loading);
        return false;
    }
    task.value().is_some()
}

pub struct Cell {
    pub text: String,
    pub tone: Tone,
    pub pill: bool,
    pub mono: bool,
}

impl Cell {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            tone: Tone::Plain,
            pill: false,
            mono: false,
        }
    }
    pub fn muted(text: impl Into<String>) -> Self {
        Self {
            tone: Tone::Muted,
            ..Self::plain(text)
        }
    }
    pub fn tone(text: impl Into<String>, tone: Tone) -> Self {
        Self {
            tone,
            ..Self::plain(text)
        }
    }
    pub fn pill(text: impl Into<String>, tone: Tone) -> Self {
        Self {
            tone,
            pill: true,
            ..Self::plain(text)
        }
    }
    pub fn mono(text: impl Into<String>, tone: Tone) -> Self {
        Self {
            tone,
            mono: true,
            ..Self::plain(text)
        }
    }
    pub fn show(&self, ui: &mut Ui) {
        self.show_with(ui, true);
    }

    pub fn show_with(&self, ui: &mut Ui, truncate: bool) {
        if self.pill {
            pill(ui, &self.text, self.tone);
        } else {
            let mut t = RichText::new(&self.text).color(self.tone.color());
            if self.mono {
                t = t.monospace();
            }
            let l = egui::Label::new(t);
            ui.add(if truncate { l.truncate() } else { l.extend() });
        }
    }
}

pub struct TableSpec<'a> {
    pub id: &'a str,
    pub headers: &'a [&'a str],
    pub checks: Option<&'a mut Vec<bool>>,
}

pub fn table(
    ui: &mut Ui,
    spec: TableSpec<'_>,
    rows: &[Vec<Cell>],
    selected: &mut Option<usize>,
) -> Option<usize> {
    let TableSpec {
        id,
        headers,
        mut checks,
    } = spec;
    let mut clicked = None;
    let mut b = TableBuilder::new(ui)
        .id_salt(id)
        .striped(true)
        .sense(egui::Sense::click())
        .resizable(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .min_scrolled_height(0.0)
        .auto_shrink([false, true]);
    if checks.is_some() {
        b = b.column(Column::exact(26.0));
    }
    let n = headers.len();
    let grow = grow_column(rows, n);
    for i in 0..n {
        b = b.column(if i == grow {
            Column::remainder().at_least(140.0).clip(true)
        } else {
            Column::auto().at_least(44.0).clip(true)
        });
    }
    b.header(26.0, |mut h| {
        if let Some(c) = checks.as_deref_mut() {
            h.col(|ui| {
                let mut all = !c.is_empty() && c.iter().all(|x| *x);
                if ui.checkbox(&mut all, "").changed() {
                    c.iter_mut().for_each(|x| *x = all);
                }
            });
        }
        for t in headers {
            h.col(|ui| {
                ui.label(RichText::new(*t).size(12.0).strong().color(theme::MUTED));
            });
        }
    })
    .body(|body| {
        body.rows(28.0, rows.len(), |mut row| {
            let i = row.index();
            row.set_selected(*selected == Some(i));
            if let Some(c) = checks.as_deref_mut() {
                row.col(|ui| {
                    if let Some(v) = c.get_mut(i) {
                        ui.checkbox(v, "");
                    }
                });
            }
            for (ci, cell) in rows[i].iter().enumerate() {
                row.col(|ui| cell.show_with(ui, ci == grow));
            }
            if row.response().clicked() {
                *selected = Some(i);
                clicked = Some(i);
            }
        });
    });
    clicked
}

pub fn grow_column(rows: &[Vec<Cell>], n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let mut total = vec![0usize; n];
    for r in rows.iter().take(60) {
        for (i, c) in r.iter().enumerate().take(n) {
            if !c.pill {
                total[i] += c.text.chars().count();
            }
        }
    }
    total
        .iter()
        .enumerate()
        .max_by_key(|(i, t)| (**t, std::cmp::Reverse(*i)))
        .map_or(n - 1, |(i, _)| i)
}

pub struct OutputWindow {
    pub title: String,
    pub task: Task<String>,
    pub empty: String,
    open: bool,
}

impl OutputWindow {
    pub fn new(
        cx: &Cx,
        title: impl Into<String>,
        empty: &str,
        argv: Vec<String>,
        timeout: u64,
    ) -> Self {
        let mut task = Task::default();
        let empty_s = empty.to_string();
        task.spawn(cx, async move {
            let out = crate::shell::capture(&argv, timeout).await?;
            let text = crate::shell::merge_output(&out);
            Ok(if text.is_empty() { empty_s } else { text })
        });
        Self {
            title: title.into(),
            task,
            empty: empty.into(),
            open: true,
        }
    }

    pub fn from_future<F>(cx: &Cx, title: impl Into<String>, fut: F) -> Self
    where
        F: Future<Output = anyhow::Result<String>> + Send + 'static,
    {
        let mut task = Task::default();
        task.spawn(cx, fut);
        Self {
            title: title.into(),
            task,
            empty: String::new(),
            open: true,
        }
    }

    pub fn text(title: impl Into<String>, text: String) -> Self {
        Self {
            title: title.into(),
            task: Task {
                rx: None,
                result: Some(Ok(text)),
                at: Some(Instant::now()),
            },
            empty: String::new(),
            open: true,
        }
    }
}

pub fn show_output(ctx: &egui::Context, slot: &mut Option<OutputWindow>) {
    let Some(w) = slot else { return };
    w.task.poll();
    let mut open = w.open;
    egui::Window::new(RichText::new(&w.title).strong())
        .id(egui::Id::unique(("output", &w.title)))
        .open(&mut open)
        .default_size([820.0, 520.0])
        .resizable(true)
        .collapsible(false)
        .show(ctx, |ui| {
            if task_status(ui, &w.task, "Läuft …") {
                let text = w.task.value().cloned().unwrap_or_default();
                ui.horizontal(|ui| {
                    if ui.small_button("📋 Kopieren").clicked() {
                        ui.ctx().copy_text(text.clone());
                    }
                    ui.label(
                        RichText::new(format!("{} Zeilen", text.lines().count()))
                            .small()
                            .color(theme::MUTED),
                    );
                });
                ui.separator();
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add(egui::Label::new(RichText::new(text).monospace()).selectable(true));
                    });
            }
        });
    w.open = open;
    if !open {
        *slot = None;
    }
}

pub fn confirm(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    lines: &[String],
    yes: &str,
    danger: bool,
) -> Option<bool> {
    let mut result = None;
    let r = egui::Modal::new(egui::Id::unique(("confirm", id))).show(ctx, |ui| {
        ui.set_max_width(560.0);
        ui.label(RichText::new(title).heading().strong().color(theme::STRONG));
        ui.add_space(6.0);
        for l in lines {
            ui.label(l);
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let b = if danger {
                danger_button(yes)
            } else {
                primary_button(yes)
            };
            if ui.add(b).clicked() {
                result = Some(true);
            }
            if ui.button("Abbrechen").clicked() {
                result = Some(false);
            }
        });
    });
    if r.should_close() && result.is_none() {
        result = Some(false);
    }
    result
}

pub fn report_lines(ui: &mut Ui, lines: &[(Tone, String)]) {
    for (tone, text) in lines {
        ui.label(RichText::new(text).color(tone.color()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grow_picks_widest_text_column() {
        let rows = vec![vec![
            Cell::plain("CI"),
            Cell::plain("ein langer Titel"),
            Cell::pill("success success", Tone::Ok),
        ]];
        assert_eq!(grow_column(&rows, 3), 1);
        assert_eq!(grow_column(&[], 3), 0);
    }
}
