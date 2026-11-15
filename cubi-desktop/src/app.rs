use std::sync::Arc;

use eframe::egui::{self, CornerRadius, Frame, Margin, RichText, Stroke};

use crate::ui::{self, Cx, theme};
use crate::views::{self, View};

pub struct DesktopApp {
    _rt: tokio::runtime::Runtime,
    cx: Cx,
    views: Vec<Box<dyn View>>,
    active: usize,
    #[cfg(feature = "screenshot")]
    shot: Option<Shot>,
}

#[cfg(feature = "screenshot")]
type Shot = (
    std::path::PathBuf,
    u32,
    bool,
    Arc<std::sync::Mutex<Option<Arc<egui::ColorImage>>>>,
);

impl DesktopApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        rt: tokio::runtime::Runtime,
        cubi: Result<crate::cubi_config::CubiConfig, String>,
        terminal: Vec<String>,
        precheck: views::precheck::Precheck,
        start: Option<String>,
    ) -> Self {
        ui::apply_theme(&cc.egui_ctx);
        let cx = Cx {
            rt: rt.handle().clone(),
            egui: cc.egui_ctx.clone(),
            cubi: Arc::new(cubi),
            terminal: Arc::new(terminal),
        };
        let mut views = views::all(&cx, precheck);
        let mut active = 0;
        if let Some(name) = start {
            let id = views::alias(&name).to_string();
            if let Some(i) = views.iter().position(|v| v.id() == id) {
                active = i;
            }
            views[active].open(&name);
        }
        Self {
            _rt: rt,
            cx,
            views,
            active,
            #[cfg(feature = "screenshot")]
            shot: std::env::var_os("CUBI_DESKTOP_SCREENSHOT")
                .map(|p| (p.into(), 0, false, Arc::new(std::sync::Mutex::new(None)))),
        }
    }
}

impl eframe::App for DesktopApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::left("nav")
            .resizable(false)
            .exact_size(232.0)
            .frame(
                Frame::new()
                    .fill(theme::PANEL)
                    .inner_margin(Margin::symmetric(12, 14))
                    .stroke(Stroke::new(1.0, theme::BORDER)),
            )
            .show(ui, |ui| self.sidebar(ui));
        egui::CentralPanel::default_margins()
            .frame(
                Frame::new()
                    .fill(theme::BG)
                    .inner_margin(Margin::symmetric(22, 16)),
            )
            .show(ui, |ui| {
                let cx = self.cx.clone();
                if let Some(e) = cx.cubi().err()
                    && self.views[self.active].id() != "precheck"
                {
                    ui::error_box(ui, &format!("cubi-Konfiguration: {e}"));
                    ui.add_space(6.0);
                }
                self.views[self.active].ui(ui, &cx);
            });
        #[cfg(feature = "screenshot")]
        self.screenshot(ui.ctx());
    }
}

impl DesktopApp {
    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            Frame::new()
                .fill(theme::ACCENT)
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::symmetric(8, 3))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new("c")
                            .size(20.0)
                            .strong()
                            .color(egui::Color32::WHITE),
                    );
                });
            ui.vertical(|ui| {
                ui.label(
                    RichText::new("cubi")
                        .size(19.0)
                        .strong()
                        .color(theme::STRONG),
                );
                ui.label(RichText::new("Desktop").size(11.5).color(theme::MUTED));
            });
        });
        ui.add_space(16.0);
        let groups: [(&str, &[&str]); 4] = [
            ("Entwicklung", &["precheck", "github"]),
            ("Cluster", &["argocd", "kubernetes", "helm"]),
            ("Zugang", &["connect", "tailscale", "login"]),
            ("Betrieb", &["tokens"]),
        ];
        for (label, ids) in groups {
            ui::section_title(ui, label);
            ui.add_space(2.0);
            for id in ids {
                let Some(i) = self.views.iter().position(|v| v.id() == *id) else {
                    continue;
                };
                let v = &self.views[i];
                let active = i == self.active;
                let resp = Frame::new()
                    .fill(if active {
                        theme::ACCENT.gamma_multiply(0.18)
                    } else {
                        egui::Color32::TRANSPARENT
                    })
                    .stroke(Stroke::new(
                        1.0,
                        if active {
                            theme::ACCENT.gamma_multiply(0.5)
                        } else {
                            egui::Color32::TRANSPARENT
                        },
                    ))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(v.icon()).size(16.0).color(if active {
                                theme::ACCENT
                            } else {
                                theme::MUTED
                            }));
                            ui.vertical(|ui| {
                                ui.label(RichText::new(v.title()).strong().color(if active {
                                    theme::STRONG
                                } else {
                                    theme::TEXT
                                }));
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(v.hint()).size(11.0).color(theme::MUTED),
                                    )
                                    .truncate(),
                                );
                            });
                        });
                    })
                    .response
                    .interact(egui::Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if resp.clicked() {
                    self.active = i;
                }
            }
            ui.add_space(10.0);
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            let cfg = match self.cx.cubi() {
                Ok(c) => c.path.display().to_string(),
                Err(e) => e,
            };
            ui.add(egui::Label::new(RichText::new(cfg).size(10.5).color(theme::MUTED)).truncate());
            ui.label(RichText::new("targets.yaml").size(10.5).color(theme::MUTED));
        });
    }

    #[cfg(feature = "screenshot")]
    fn screenshot(&mut self, ctx: &egui::Context) {
        let Some((path, frames, requested, slot)) = &mut self.shot else {
            return;
        };
        *frames += 1;
        ctx.request_repaint();
        let wait: u32 = std::env::var("CUBI_DESKTOP_SCREENSHOT_FRAMES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(240);
        if *frames >= wait && !*requested {
            *requested = true;
            let s = slot.clone();
            ctx.request_screenshot(move |img| {
                *s.lock().unwrap() = Some(img);
            });
        }
        let img = slot.lock().unwrap().take();
        if let Some(img) = img {
            let [w, h] = img.size;
            let bytes: Vec<u8> = img.pixels.iter().flat_map(|p| p.to_array()).collect();
            if let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, bytes) {
                let _ = buf.save(&*path);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
