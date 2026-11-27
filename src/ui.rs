//! Stellar dashboard (egui).

mod cooling_page;
mod device_panel;

use crate::config::{AutoRule, Effect, FanMode, QuietHours, Rgb, Settings};
use crate::cooling::{Sensor, paired_rpm};
use crate::engine::{Cmd, State};
use crate::platform::{self, Window};
use crate::tray::{self, Action, Tone, Tray};
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Layout, Pos2, Rect, RichText, Sense, Stroke, pos2, vec2,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

// ---------------------------------------------------------------- palette

const BG: Color32 = Color32::from_rgb(9, 11, 22);
const PANEL: Color32 = Color32::from_rgb(17, 21, 40);
const CARD: Color32 = Color32::from_rgb(23, 28, 52);
const LINE: Color32 = Color32::from_rgb(42, 49, 84);
const TEXT: Color32 = Color32::from_rgb(226, 230, 245);
const MUTED: Color32 = Color32::from_rgb(134, 142, 176);
const VIOLET: Color32 = Color32::from_rgb(139, 108, 255);
const CYAN: Color32 = Color32::from_rgb(79, 209, 255);
const GOLD: Color32 = Color32::from_rgb(255, 204, 102);
const GOOD: Color32 = Color32::from_rgb(92, 224, 160);
const BAD: Color32 = Color32::from_rgb(255, 92, 82);

fn rgb(c: Rgb) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

const SWATCHES: [(Rgb, &str); 10] = [
    ([255, 255, 255], "White"),
    ([255, 236, 210], "Warm white"),
    ([255, 40, 40], "Red"),
    ([255, 128, 0], "Orange"),
    ([255, 204, 0], "Amber"),
    ([40, 255, 90], "Green"),
    ([0, 220, 255], "Cyan"),
    ([30, 80, 255], "Blue"),
    ([140, 70, 255], "Violet"),
    ([255, 60, 200], "Pink"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Overview,
    Cooling,
    Lighting,
    Profiles,
    Sensors,
    Settings,
}

pub fn run(state: State, tx: Sender<Cmd>, repaint: Arc<Mutex<Option<egui::Context>>>, start_hidden: bool) -> eframe::Result {
    let icon = egui::IconData { rgba: tray::star_rgba(64, Tone::Normal), width: 64, height: 64 };
    // Fit the screen: never open larger than ~90% of the usable area, so it
    // works on small laptops and at high display scaling.
    let (sw, sh) = platform::work_area().unwrap_or((1920.0, 1040.0));
    let mut initial = [1180.0f32.min(sw * 0.9), 780.0f32.min(sh * 0.88)];
    // Developer screenshots only: STELLAR_SNAPSHOT_SIZE=900x700
    if let Some((w, h)) = std::env::var("STELLAR_SNAPSHOT_SIZE").ok().and_then(|v| {
        let (w, h) = v.split_once('x')?;
        Some((w.parse().ok()?, h.parse().ok()?))
    }) {
        initial = [w, h];
    }
    let minimum = [760.0f32.min(sw * 0.8), 500.0f32.min(sh * 0.7)];
    let options = eframe::NativeOptions {
        centered: true,
        viewport: egui::ViewportBuilder::default()
            .with_title("Stellar")
            .with_inner_size(initial)
            .with_min_inner_size(minimum)
            .with_visible(!start_hidden)
            .with_icon(icon),
        ..Default::default()
    };
    eframe::run_native("Stellar", options, Box::new(move |cc| Ok(Box::new(App::new(cc, state, tx, repaint)))))
}

struct App {
    state: State,
    tx: Sender<Cmd>,
    tray: Option<Tray>,
    tray_rx: Receiver<Action>,
    window: Option<Window>,
    logo: egui::TextureHandle,
    page: Page,
    committed: Settings,
    draft: Settings,
    generation: u64,
    selected: Option<String>,
    profile_name: String,
    quitting: bool,
    pending_lighting: Option<Instant>,
    started: Instant,
    confirm_quit: bool,
    selected_curve: Option<String>,
    show_unused: bool,
    tune_dialog: Option<Vec<(String, String, bool)>>,
    brush: Rgb,
    installing: Arc<Mutex<Option<String>>>,
    /// Developer aid: STELLAR_SNAPSHOT=dir saves a PNG of every page and exits.
    snapshot: Option<(std::path::PathBuf, usize, u32)>,
    snapshot_scroll: Option<f32>,
    snapshot_bottom: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, state: State, tx: Sender<Cmd>, repaint: Arc<Mutex<Option<egui::Context>>>) -> Self {
        let ctx = cc.egui_ctx.clone();
        *repaint.lock().unwrap() = Some(ctx.clone());
        style(&ctx);
        let window = {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            cc.window_handle().ok().and_then(|h| match h.as_raw() {
                RawWindowHandle::Win32(w) => Some(Window(w.hwnd.get())),
                _ => None,
            })
        };
        let (atx, arx) = std::sync::mpsc::channel();
        let tray = {
            let c1 = ctx.clone();
            let w = window;
            Tray::new(atx.clone(), move || c1.request_repaint(), move || {
                if let Some(w) = w {
                    w.show();
                }
            })
            .map_err(|e| crate::log(&format!("Tray unavailable: {e}")))
            .ok()
        };
        {
            let c = ctx.clone();
            platform::watch(platform::SHOW_EVENT, move || {
                if let Some(w) = window {
                    w.show();
                }
                c.request_repaint();
            });
            let c = ctx.clone();
            let q = atx.clone();
            platform::watch(platform::QUIT_EVENT, move || {
                let _ = q.send(Action::Quit);
                c.request_repaint();
            });
        }
        let logo = ctx.load_texture(
            "logo",
            egui::ColorImage::from_rgba_unmultiplied([64, 64], &tray::star_rgba(64, Tone::Normal)),
            egui::TextureOptions::LINEAR,
        );
        Self {
            state,
            tx,
            tray,
            tray_rx: arx,
            window,
            logo,
            page: Page::Overview,
            committed: Settings::default(),
            draft: Settings::default(),
            generation: 0,
            selected: None,
            profile_name: String::new(),
            quitting: false,
            pending_lighting: None,
            started: Instant::now(),
            confirm_quit: false,
            selected_curve: None,
            show_unused: false,
            tune_dialog: None,
            brush: [255, 40, 40],
            installing: Arc::new(Mutex::new(None)),
            snapshot: std::env::var_os("STELLAR_SNAPSHOT").map(|d| (d.into(), 0, 0)),
            snapshot_scroll: None,
            snapshot_bottom: false,
        }
    }

    fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }

    fn cooling_dirty(&self) -> bool {
        self.draft.fans != self.committed.fans
            || self.draft.curves != self.committed.curves
            || self.draft.automation != self.committed.automation
            || self.draft.fallback_profile != self.committed.fallback_profile
            || self.draft.notifications != self.committed.notifications
    }

    fn sync_settings(&mut self) {
        let s = self.state.lock().unwrap();
        if s.generation == self.generation {
            return;
        }
        self.generation = s.generation;
        let dirty = self.cooling_dirty();
        self.committed = s.settings.clone();
        drop(s);
        if !dirty {
            self.draft = self.committed.clone();
        } else {
            self.draft.active_profile = self.committed.active_profile.clone();
        }
        if self.pending_lighting.is_none() {
            self.draft.lighting = self.committed.lighting.clone();
        }
    }

    fn lighting_changed(&mut self) {
        self.pending_lighting = Some(Instant::now());
    }

    fn flush_lighting(&mut self, force: bool) {
        if let Some(t) = self.pending_lighting
            && (force || t.elapsed().as_millis() > 250)
        {
            self.pending_lighting = None;
            let mut new = self.committed.clone();
            new.lighting = self.draft.lighting.clone();
            if new.validate().is_ok() {
                self.committed = new.clone();
                self.send(Cmd::Save(new));
            }
        }
    }

    fn handle_tray(&mut self, ctx: &egui::Context) {
        while let Ok(a) = self.tray_rx.try_recv() {
            match a {
                Action::Show => {
                    if let Some(w) = self.window {
                        w.show();
                    }
                }
                Action::ToggleLighting => self.send(Cmd::SetLighting(!self.committed.lighting.on)),
                Action::Profile(id) => {
                    if let Some(name) = tray::profile_from_action(&id) {
                        self.send(Cmd::ApplyProfile(name.to_string()));
                    }
                }
                Action::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        let s = self.state.lock().unwrap();
        let active_rules = s.settings.fans.iter().any(|r| r.mode != FanMode::Firmware);
        let tone = if s.settings.notifications && (s.emergency || (active_rules && !s.cooling_ok)) {
            Tone::Alert
        } else if !s.settings.lighting.on {
            Tone::Dim
        } else {
            Tone::Normal
        };
        let cpu = s.history.back().and_then(|h| h.0).map(|t| format!(" · CPU {t:.0}°C")).unwrap_or_default();
        let tip = format!("Stellar{cpu}\n{}\nLighting: {}", s.cooling_status, if s.settings.lighting.on { "on" } else { "off" });
        let (on, profiles, active) = (s.settings.lighting.on, s.profiles.clone(), s.settings.active_profile.clone());
        drop(s);
        if let Some(t) = &mut self.tray {
            t.sync(on, &profiles, active.as_deref(), tone, &tip);
        }
    }
}

fn style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = Color32::from_rgb(12, 15, 30);
    v.faint_bg_color = CARD;
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = VIOLET.gamma_multiply(0.55);
    v.selection.stroke = Stroke::new(1.0, VIOLET);
    v.hyperlink_color = CYAN;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    v.widgets.inactive.bg_fill = Color32::from_rgb(32, 38, 70);
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(32, 38, 70);
    v.widgets.hovered.bg_fill = Color32::from_rgb(48, 54, 98);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(48, 54, 98);
    v.widgets.active.bg_fill = VIOLET;
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = CornerRadius::same(7);
    }
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = vec2(8.0, 8.0);
        s.spacing.button_padding = vec2(12.0, 6.0);
        s.spacing.slider_width = 220.0;
    });
}

/// Width taken by a tile's frame (margins + stroke) around its content.
const TILE_CHROME: f32 = 28.0;

fn tile_frame(selected: bool, alert: bool) -> egui::Frame {
    egui::Frame::new()
        .fill(if selected { VIOLET.gamma_multiply(0.22) } else { CARD })
        .stroke(Stroke::new(1.0, if alert { BAD } else if selected { VIOLET } else { LINE }))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::same(12))
}

/// Lays out `n` equal-width tiles in rows that always fit the available
/// width (at least `min_w` each), so card rows never overflow the window.
fn tiles(ui: &mut egui::Ui, min_w: f32, n: usize, mut f: impl FnMut(&mut egui::Ui, usize, f32)) {
    let gap = ui.spacing().item_spacing.x;
    let avail = ui.available_width();
    let per_row = (((avail + gap) / (min_w + gap)).floor() as usize).max(1);
    let w = ((avail - gap * (per_row - 1) as f32) / per_row as f32).floor().max(min_w.min(avail));
    let idx: Vec<usize> = (0..n).collect();
    for row in idx.chunks(per_row) {
        ui.horizontal(|ui| {
            for &i in row {
                ui.vertical(|ui| {
                    ui.set_width(w);
                    f(ui, i, w);
                });
            }
        });
    }
}

fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            // Panels span the full width available, so stacked panels line up,
            // and use a plain top-down layout so wrapped text isn't justified.
            ui.set_min_width(ui.available_width());
            ui.with_layout(Layout::top_down(Align::Min), add).inner
        })
        .inner
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(22.0).strong().color(TEXT));
}

fn sub(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(13.0).strong().color(MUTED));
}

fn chip(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.18))
        .corner_radius(CornerRadius::same(20))
        .inner_margin(egui::Margin::symmetric(9, 3))
        .show(ui, |ui| ui.label(RichText::new(text).size(12.0).color(color)));
}

fn unit(kind: &str) -> &'static str {
    match kind {
        "Temperature" => "°C",
        "Fan" => " RPM",
        "Control" | "Load" => "%",
        "Power" => " W",
        "Clock" => " MHz",
        _ => "",
    }
}

fn fmt_sensor(s: &Sensor) -> String {
    match s.value {
        Some(v) if s.kind == "Fan" || s.kind == "Clock" => format!("{v:.0}{}", unit(&s.kind)),
        Some(v) => format!("{v:.1}{}", unit(&s.kind)),
        None => "—".into(),
    }
}

fn temp_color(t: f64) -> Color32 {
    if t >= 85.0 {
        BAD
    } else if t >= 70.0 {
        GOLD
    } else {
        GOOD
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.sync_settings();
        self.handle_tray(ctx);
        self.flush_lighting(false);
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            // Closing hides to the tray; cooling keeps running.
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if let Some(w) = self.window {
                w.hide();
            }
        }
        if self.state.lock().unwrap().stopped {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if let Some((dir, page, frames)) = &mut self.snapshot
            && self.started.elapsed().as_secs() >= 18
        {
            // Each page is captured twice: top, then scrolled to the bottom.
            const PAGES: [Page; 6] = [Page::Overview, Page::Cooling, Page::Lighting, Page::Profiles, Page::Sensors, Page::Settings];
            *frames += 1;
            self.page = PAGES[(*page / 2).min(5)];
            self.snapshot_bottom = *page % 2 == 1;
            if !self.snapshot_bottom {
                self.snapshot_scroll = Some(0.0);
            }

            if *frames == 40 {
                let path = dir.join(format!("page{page}.png"));
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
                    let raw: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                    let _ = image::save_buffer(&path, &raw, img.width() as u32, img.height() as u32, image::ColorType::Rgba8);
                })));
            } else if *frames == 50 {
                *frames = 0;
                *page += 1;
                if *page >= PAGES.len() * 2 {
                    let _ = std::fs::write(dir.join("done"), b"");
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            ctx.request_repaint();
        }
        let animated = self.page == Page::Lighting || self.page == Page::Overview;
        ctx.request_repaint_after(std::time::Duration::from_millis(if animated { 50 } else { 500 }));

        egui::Panel::left("nav").exact_size(210.0).resizable(false).frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::same(16))).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend); // move whole widgets to the next line instead of squashing them
                ui.add(egui::Image::new(&self.logo).fit_to_exact_size(vec2(34.0, 34.0)));
                ui.vertical(|ui| {
                    ui.label(RichText::new("Stellar").size(22.0).strong());
                    ui.label(RichText::new("cooling & lighting").size(11.0).color(MUTED));
                });
            });
            ui.add_space(18.0);
            for (p, label) in [
                (Page::Overview, "Overview"),
                (Page::Cooling, "Cooling"),
                (Page::Lighting, "Lighting"),
                (Page::Profiles, "Profiles & automation"),
                (Page::Sensors, "Sensors"),
                (Page::Settings, "Settings"),
            ] {
                let selected = self.page == p;
                let text = RichText::new(label).size(14.5).color(if selected { TEXT } else { MUTED });
                let b = egui::Button::new(text)
                    .fill(if selected { VIOLET.gamma_multiply(0.28) } else { Color32::TRANSPARENT })
                    .stroke(Stroke::NONE)
                    .min_size(vec2(ui.available_width(), 36.0));
                if ui.add(b).clicked() {
                    self.page = p;
                }
            }
            ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                let s = self.state.lock().unwrap();
                let profile = s.settings.active_profile.clone().unwrap_or_else(|| "Custom".into());
                ui.label(RichText::new(format!("Profile: {profile}")).size(12.0).color(MUTED));
                let (c, t) = if s.emergency {
                    (BAD, "Cooling safeguard active")
                } else if s.competing.is_some() {
                    (GOLD, "Fans on firmware (other app)")
                } else if s.cooling_ok {
                    (GOOD, "Cooling OK")
                } else {
                    (GOLD, "Cooling needs attention")
                };
                dot_label(ui, c, t);
                let (c, t) = if s.lighting_ok { (GOOD, "Lighting connected") } else { (GOLD, "Lighting waiting") };
                dot_label(ui, c, t);
            });
        });

        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::same(22))).show(ui, |ui| {
            if self.cooling_dirty() {
                egui::Frame::new().fill(VIOLET.gamma_multiply(0.2)).corner_radius(CornerRadius::same(10)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend); // move whole widgets to the next line instead of squashing them
                        ui.label(RichText::new("Unsaved cooling or automation changes").strong());
                        {
                            let valid = self.draft.validate();
                            if ui.add_enabled(valid.is_ok(), egui::Button::new(RichText::new("Save").strong()).fill(VIOLET)).clicked() {
                                let mut new = self.draft.clone();
                                new.lighting = self.committed.lighting.clone();
                                self.send(Cmd::Save(new));
                            }
                            if ui.button("Discard").clicked() {
                                let l = self.draft.lighting.clone();
                                self.draft = self.committed.clone();
                                self.draft.lighting = l;
                            }
                            if let Err(e) = valid {
                                ui.label(RichText::new(format!("{e:#}")).color(BAD));
                            }
                        }
                    });
                });
                ui.add_space(8.0);
            }
            let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
            if let Some(y) = self.snapshot_scroll {
                area = area.vertical_scroll_offset(y);
            }
            let out = area.show(ui, |ui| match self.page {
                Page::Overview => self.overview(ui),
                Page::Cooling => self.cooling(ui),
                Page::Lighting => self.lighting(ui),
                Page::Profiles => self.profiles(ui),
                Page::Sensors => self.sensors(ui),
                Page::Settings => self.settings_page(ui),
            });
            if self.snapshot.is_some() && self.snapshot_bottom {
                self.snapshot_scroll = Some((out.content_size.y - out.inner_rect.height()).max(0.0));
            }
        });

        if self.confirm_quit {
            egui::Window::new("Quit Stellar?").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(&ctx, |ui| {
                ui.label("Fans return to motherboard/GPU firmware control and lighting stops updating.");
                if self.cooling_dirty() {
                    ui.label(RichText::new("Unsaved cooling changes will be lost.").color(GOLD));
                }
                ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend); // move whole widgets to the next line instead of squashing them
                    if ui.add(egui::Button::new("Quit").fill(BAD.gamma_multiply(0.7))).clicked() {
                        self.quitting = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm_quit = false;
                    }
                });
            });
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.flush_lighting(true);
    }
}

// ---------------------------------------------------------------- pages

impl App {
    fn overview(&mut self, ui: &mut egui::Ui) {
        heading(ui, "Overview");
        ui.add_space(6.0);
        self.pawnio_banner(ui);
        let s = self.state.lock().unwrap();
        let cpu = s.history.back().and_then(|h| h.0);
        let gpu = s.history.back().and_then(|h| h.1);
        let cpu_load = s.snapshot.sensors.iter().find(|x| x.kind == "Load" && x.id.contains("cpu") && x.name == "CPU Total").and_then(|x| x.value);
        let gpu_load = s.snapshot.sensors.iter().find(|x| x.kind == "Load" && x.id.starts_with("/gpu-nvidia") && x.name == "GPU Core").and_then(|x| x.value);
        tiles(ui, 175.0, 4, |ui, i, _| match i {
            0 => stat(ui, "CPU", cpu, cpu_load),
            1 => stat(ui, "GPU", gpu, gpu_load),
            2 => card(ui, |ui| {
                sub(ui, "COOLING");
                ui.label(RichText::new(&s.cooling_status).size(16.0).color(if s.emergency { BAD } else if s.cooling_ok { TEXT } else { GOLD }));
                if let Some(c) = s.competing {
                    ui.label(RichText::new(format!("{c} is running")).color(GOLD).size(12.0));
                }
            }),
            _ => card(ui, |ui| {
                sub(ui, "LIGHTING");
                let l = &s.settings.lighting;
                let text = if !l.managed { "Not controlled" } else if l.on { l.effect.label() } else { "Off" };
                ui.label(RichText::new(text).size(16.0));
                if l.managed {
                    ui.label(RichText::new(&s.lighting_status).color(MUTED).size(12.0));
                }
            }),
        });
        ui.add_space(8.0);
        card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend); // move whole widgets to the next line instead of squashing them
                sub(ui, "TEMPERATURES · LAST 10 MINUTES");
                {
                    dot_label(ui, CYAN, "GPU");
                    dot_label(ui, VIOLET, "CPU");
                }
            });
            history_chart(ui, &s.history);
        });
        ui.add_space(8.0);
        card(ui, |ui| {
            sub(ui, "FANS");
            let outputs: Vec<&Sensor> = s.snapshot.sensors.iter().filter(|x| x.kind == "Control" && s.snapshot.controls.contains_key(&x.id)).collect();
            if outputs.is_empty() {
                ui.label(RichText::new("No controllable fan outputs detected yet. Stellar needs administrator rights to reach motherboard fan headers.").color(MUTED));
            }
            let shown: Vec<&Sensor> = outputs
                .into_iter()
                .filter(|o| {
                    let rule = s.settings.rule(&o.id);
                    let rpm_id = rule.and_then(|r| r.rpm_sensor_id.clone()).unwrap_or_else(|| paired_rpm(&o.id));
                    // Unused headers live in the sensor library.
                    rule.is_some() || s.sensor(&rpm_id).and_then(|x| x.value).unwrap_or(0.0) >= 1.0
                })
                .collect();
            tiles(ui, 230.0, shown.len(), |ui, i, w| {
                let o = shown[i];
                let rule = s.settings.rule(&o.id);
                let rpm_id = rule.and_then(|r| r.rpm_sensor_id.clone()).unwrap_or_else(|| paired_rpm(&o.id));
                let rpm = s.sensor(&rpm_id).and_then(|x| x.value);
                let d = s.decisions.get(&o.id);
                let emergency = d.is_some_and(|d| d.emergency);
                tile_frame(false, emergency).show(ui, |ui| {
                    ui.set_width(w - TILE_CHROME);
                    let name = rule.map(|r| r.name.clone()).unwrap_or_else(|| format!("{} · {}", short_hw(&o.hardware), o.name));
                    ui.add(egui::Label::new(RichText::new(name).strong()).truncate());
                    let duty = o.value.unwrap_or(0.0);
                    ui.add(egui::ProgressBar::new((duty / 100.0) as f32).desired_width(w - TILE_CHROME).text(format!("{duty:.0}%")).fill(VIOLET));
                    let expected = rule.and_then(|r| r.expected_rpm(duty));
                    ui.label(match (rpm, expected) {
                        (Some(r), Some(x)) => format!("{r:.0} RPM  ·  calibrated ≈{x:.0}"),
                        (Some(r), None) => format!("{r:.0} RPM"),
                        _ => "—".into(),
                    });
                    let mode = rule.map(|r| r.mode).unwrap_or_default();
                    ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend); // move whole widgets to the next line instead of squashing them
                        chip(ui, match mode { FanMode::Firmware => "Firmware", FanMode::Curve => "Curve", FanMode::Manual => "Manual" }, if mode == FanMode::Firmware { MUTED } else { CYAN });
                        ui.label(RichText::new(d.map(|d| d.reason.as_str()).unwrap_or("Firmware control")).size(12.0).color(if emergency { BAD } else { MUTED }));
                    });
                });
            });
        });
        ui.add_space(8.0);
        card(ui, |ui| {
            sub(ui, "ACTIVITY");
            if s.notices.is_empty() {
                ui.label(RichText::new("Nothing to report.").color(MUTED));
            }
            for n in s.notices.iter().take(8) {
                ui.label(RichText::new(n).size(12.5).color(if n.contains('⚠') { GOLD } else { MUTED }));
            }
        });
    }

    fn lighting(&mut self, ui: &mut egui::Ui) {
        let s = self.state.lock().unwrap();
        let mut devices = s.devices.clone();
        if std::env::var_os("STELLAR_SNAPSHOT_DEMO").is_some() {
            let kb = demo_keyboard();
            let mut mouse = demo_keyboard();
            mouse.name = "Demo device with built-in effects".into();
            mouse.kind = 6;
            self.draft.lighting.devices.entry(mouse.name.clone()).or_default().onboard =
                Some(crate::config::OnboardMode { mode: "Breathing".into(), colors: vec![[255, 0, 120]], ..Default::default() });
            self.draft.lighting.devices.entry(kb.name.clone()).or_default().keys.extend((33..37).map(|i| (i, [255, 40, 40])));
            devices.insert(0, kb);
            devices.insert(0, mouse);
        }
        let sensors: Vec<Sensor> = s.snapshot.sensors.iter().filter(|x| x.kind == "Temperature").cloned().collect();
        let status = (s.lighting_ok, s.lighting_status.clone());
        let openrgb = s.openrgb_installed;
        drop(s);
        let before = self.draft.lighting.clone();
        let t = self.started.elapsed().as_secs_f64();
        let temps: HashMap<String, f64> = sensors.iter().filter_map(|x| Some((x.id.clone(), x.value?))).collect();
        heading(ui, "Lighting");
        if !self.committed.lighting.managed {
            card(ui, |ui| {
                ui.label(RichText::new("Stellar isn't controlling your RGB lighting").strong());
                ui.label(RichText::new("Stellar drives every RGB device OpenRGB supports — motherboard headers, RAM, GPUs, fan controllers, keyboards — with synced effects, including colours that follow your temperatures. While it's in control, other RGB apps (Armoury Crate, iCUE, SignalRGB…) should be closed so they don't fight over the lights.").size(12.5).color(MUTED));
                if ui.add(egui::Button::new(RichText::new("Let Stellar control lighting").strong()).fill(VIOLET)).clicked() {
                    let mut new = self.committed.clone();
                    new.lighting.managed = true;
                    self.send(Cmd::Save(new));
                }
            });
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend); // move whole widgets to the next line instead of squashing them
            chip(ui, &status.1, if status.0 { GOOD } else { GOLD });
            if ui.small_button("Reconnect").clicked() {
                self.send(Cmd::ReconnectLighting);
            }
            {
                if ui.small_button("Stop controlling lighting").on_hover_text("Stellar lets go of all RGB devices; other apps can take over.").clicked() {
                    let mut new = self.committed.clone();
                    new.lighting.managed = false;
                    self.send(Cmd::Save(new));
                }
            }
        });
        if !openrgb {
            card(ui, |ui| {
                ui.label(RichText::new("Lighting needs OpenRGB").strong().color(GOLD));
                ui.label(RichText::new("OpenRGB is the free, open-source backend Stellar uses to talk to RGB devices from any brand. Stellar starts it in the background; you never need to open it.").size(12.5).color(MUTED));
                let busy = self.installing.lock().unwrap().clone();
                ui.horizontal(|ui| match busy {
                    Some(what) => {
                        ui.spinner();
                        ui.label(format!("Installing {what}…"));
                    }
                    None => {
                        if ui.add(egui::Button::new("Install OpenRGB").fill(VIOLET)).clicked() {
                            self.install_tool("OpenRGB", "OpenRGB.OpenRGB", false);
                        }
                        ui.hyperlink_to("openrgb.org", "https://openrgb.org");
                    }
                });
            });
        }
        ui.add_space(6.0);
        let l = &mut self.draft.lighting;
        card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend); // move whole widgets to the next line instead of squashing them
                let label = if l.on { "Lights on" } else { "Lights off" };
                if ui.add(egui::Button::new(RichText::new(label).size(16.0).strong()).fill(if l.on { VIOLET } else { Color32::from_rgb(40, 44, 70) }).min_size(vec2(140.0, 38.0))).clicked() {
                    l.on = !l.on;
                }
                ui.add_space(20.0);
                ui.label("Brightness");
                ui.add(egui::Slider::new(&mut l.brightness, 0.0..=100.0).suffix("%"));
            });
            ui.add_space(6.0);
            preview_strip(ui, l, "", t, &temps);
        });
        ui.add_space(8.0);
        card(ui, |ui| {
            sub(ui, "EFFECT · ALL DEVICES");
            effect_editor(ui, &mut l.effect, &sensors, "global");
        });
        ui.add_space(8.0);
        sub(ui, "DEVICES");
        if devices.is_empty() {
            ui.label(RichText::new("No RGB devices yet. Stellar starts OpenRGB in the background; devices appear once it has found them.").color(MUTED));
        }
        let mut actions = vec![];
        for d in &devices {
            device_panel::device_card(ui, d, l, &sensors, &temps, t, &mut self.brush, &mut actions);
            ui.add_space(8.0);
        }
        ui.add_space(8.0);
        card(ui, |ui| {
            sub(ui, "QUIET HOURS");
            let mut on = l.quiet_hours.is_some();
            if ui.checkbox(&mut on, "Turn lights off on a schedule").changed() {
                l.quiet_hours = on.then_some(QuietHours { start_min: 23 * 60, end_min: 7 * 60 });
            }
            if let Some(q) = &mut l.quiet_hours {
                ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend); // move whole widgets to the next line instead of squashing them
                    ui.label("From");
                    time_edit(ui, &mut q.start_min, "qs");
                    ui.label("until");
                    time_edit(ui, &mut q.end_min, "qe");
                });
            }
        });
        if self.draft.lighting != before {
            self.lighting_changed();
        }
        for a in actions {
            match a {
                device_panel::DeviceAction::SaveToDevice(name) => {
                    // Commit the latest settings first so the device gets exactly what is shown.
                    self.flush_lighting(true);
                    self.send(Cmd::SaveToDevice(name));
                }
            }
        }
    }

    fn profiles(&mut self, ui: &mut egui::Ui) {
        let s = self.state.lock().unwrap();
        let profiles = s.profiles.clone();
        let auto = s.auto_profile.clone();
        drop(s);
        heading(ui, "Profiles & automation");
        ui.add_space(6.0);
        card(ui, |ui| {
            sub(ui, "PROFILES");
            ui.label(RichText::new("A profile stores every fan rule and the full lighting setup.").color(MUTED).size(12.5));
            ui.label(format!("Active: {}", self.committed.active_profile.clone().unwrap_or_else(|| "none (custom)".into())));
            if let Some(a) = &auto {
                ui.label(RichText::new(format!("Selected by automation: {a}")).color(CYAN));
            }
            for p in &profiles {
                ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend); // move whole widgets to the next line instead of squashing them
                    let active = self.committed.active_profile.as_deref() == Some(p);
                    ui.label(RichText::new(p).strong().color(if active { VIOLET } else { TEXT }));
                    {
                        if ui.small_button("Delete").clicked() {
                            let _ = self.tx.send(Cmd::DeleteProfile(p.clone()));
                        }
                        if ui.small_button("Overwrite").on_hover_text("Save the current settings into this profile").clicked() {
                            let _ = self.tx.send(Cmd::SaveProfile(p.clone()));
                        }
                        if ui.add(egui::Button::new("Apply").fill(VIOLET.gamma_multiply(0.6))).clicked() {
                            let _ = self.tx.send(Cmd::ApplyProfile(p.clone()));
                        }
                    }
                });
            }
            ui.separator();
            ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend); // move whole widgets to the next line instead of squashing them
                ui.add(egui::TextEdit::singleline(&mut self.profile_name).hint_text("New profile name").desired_width(220.0));
                let valid = crate::config::validate_profile_name(&self.profile_name);
                if ui.add_enabled(valid.is_ok() && !self.cooling_dirty(), egui::Button::new("Save current as profile")).on_disabled_hover_text(if self.cooling_dirty() { "Save or discard cooling changes first".to_string() } else { valid.err().map(|e| e.to_string()).unwrap_or_default() }).clicked() {
                    let _ = self.tx.send(Cmd::SaveProfile(std::mem::take(&mut self.profile_name)));
                }
            });
        });
        ui.add_space(8.0);
        card(ui, |ui| {
            sub(ui, "AUTOMATION");
            ui.label(RichText::new("Switch profile automatically while a program runs — e.g. a performance profile for a game. The first matching rule wins.").color(MUTED).size(12.5));
            let mut remove = None;
            egui::Grid::new("auto").num_columns(3).show(ui, |ui| {
                for (i, a) in self.draft.automation.iter_mut().enumerate() {
                    ui.add(egui::TextEdit::singleline(&mut a.process).hint_text("program.exe").desired_width(220.0));
                    profile_combo(ui, &mut a.profile, &profiles, &format!("ap{i}"));
                    if ui.small_button("Remove").clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
            if let Some(i) = remove {
                self.draft.automation.remove(i);
            }
            if profiles.is_empty() {
                ui.label(RichText::new("Save a profile above first — automation switches between your profiles.").size(12.5).color(GOLD));
            }
            if ui.add_enabled(!profiles.is_empty(), egui::Button::new("Add rule")).on_disabled_hover_text("Save a profile first — automation switches between profiles").clicked() {
                self.draft.automation.push(AutoRule { process: String::new(), profile: profiles[0].clone() });
            }
            ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend); // move whole widgets to the next line instead of squashing them
                ui.label("When no rule matches, switch to");
                let mut fb = self.draft.fallback_profile.clone().unwrap_or_default();
                egui::ComboBox::from_id_salt("fallback").selected_text(if fb.is_empty() { "(stay as is)" } else { fb.as_str() }).show_ui(ui, |ui| {
                    ui.selectable_value(&mut fb, String::new(), "(stay as is)");
                    for p in &profiles {
                        ui.selectable_value(&mut fb, p.clone(), p);
                    }
                });
                self.draft.fallback_profile = (!fb.is_empty()).then_some(fb);
            });
        });
    }

    fn sensors(&mut self, ui: &mut egui::Ui) {
        let s = self.state.lock().unwrap();
        heading(ui, "Sensors");
        ui.label(RichText::new(format!("{} readings · updated every second", s.snapshot.sensors.len())).color(MUTED));
        let mut groups: BTreeMap<&str, Vec<&Sensor>> = BTreeMap::new();
        for x in &s.snapshot.sensors {
            groups.entry(x.hardware.as_str()).or_default().push(x);
        }
        for (hw, list) in groups {
            ui.add_space(6.0);
            card(ui, |ui| {
                ui.label(RichText::new(hw).strong().size(15.0));
                egui::Grid::new(hw).num_columns(4).spacing([24.0, 4.0]).striped(true).show(ui, |ui| {
                    for x in list {
                        ui.label(&x.name);
                        ui.label(RichText::new(&x.kind).color(MUTED));
                        let c = match (x.kind.as_str(), x.value) {
                            ("Temperature", Some(v)) => temp_color(v),
                            _ => TEXT,
                        };
                        ui.label(RichText::new(fmt_sensor(x)).color(c).monospace());
                        ui.label(RichText::new(&x.id).size(11.0).color(MUTED).monospace());
                        ui.end_row();
                    }
                });
            });
        }
    }

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        let s = self.state.lock().unwrap();
        heading(ui, "Settings");
        ui.add_space(6.0);
        card(ui, |ui| {
            sub(ui, "SYSTEM");
            egui::Grid::new("sys").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
                ui.label("Hardware host");
                let host = crate::hardware::host_path().map(|p| p.display().to_string()).unwrap_or_else(|| "missing".into());
                ui.add(egui::Label::new(host.clone()).truncate()).on_hover_text(host);
                ui.end_row();
                ui.label("Competing fan software");
                ui.label(s.competing.unwrap_or("none"));
                ui.end_row();
                ui.label("Data folder");
                let dir = crate::config::Store::default_location().dir;
                if ui.add(egui::Label::new(RichText::new(dir.display().to_string()).color(CYAN)).truncate().sense(Sense::click())).on_hover_text("Open in File Explorer").clicked() {
                    let _ = std::process::Command::new("explorer").arg(&dir).spawn();
                }
                ui.end_row();
                ui.label("Version");
                ui.label(env!("CARGO_PKG_VERSION"));
                ui.end_row();
            });
        });
        drop(s);
        ui.add_space(8.0);
        card(ui, |ui| {
            sub(ui, "BEHAVIOUR");
            ui.checkbox(&mut self.draft.notifications, "Turn the tray icon red while a cooling safeguard is active");
            ui.label(RichText::new("Closing the window keeps Stellar in the tray so cooling continues. Stellar starts with Windows through its own elevated sign-in task, so there is no prompt at startup. To remove it, use Windows Settings › Apps › Stellar.").color(MUTED).size(12.5));
        });
        ui.add_space(8.0);
        if ui.add(egui::Button::new(RichText::new("Quit Stellar").strong()).fill(BAD.gamma_multiply(0.5))).clicked() {
            self.confirm_quit = true;
        }
    }
}

// ---------------------------------------------------------------- widgets

/// Snapshot-only demo device: a Keychron V3 Max's real 87-key layout.
fn demo_keyboard() -> crate::lighting::DeviceInfo {
    use crate::openrgb::{Mode, NO_LED, Zone};
    let rows: [&[i32]; 6] = [
        &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, -1, 13, 14, 15],
        &[16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32],
        &[33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49],
        &[50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, -1, -1, -1, -1],
        &[63, -1, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, -1, -1, 75, -1],
        &[76, 77, 78, -1, -1, -1, 79, -1, -1, -1, 80, 81, 82, 83, 84, 85, 86],
    ];
    let map = rows.iter().flat_map(|r| r.iter().map(|&v| if v < 0 { NO_LED } else { v as u32 })).collect();
    let legends = "Esc F1 F2 F3 F4 F5 F6 F7 F8 F9 F10 F11 F12 PrtSc Lock Light ` 1 2 3 4 5 6 7 8 9 0 - = Backspace Insert Home PageUp Tab Q W E R T Y U I O P [ ] \\ Delete End PageDown Caps A S D F G H J K L ; ' Enter Shift Z X C V B N M , . / Shift Up Ctrl Win Alt Space Alt Fn Menu Ctrl Left Down Right";
    let led_names = legends.split(' ').map(|k| format!("Key: {k}")).collect();
    let mode = |name: &str, flags: u32| Mode {
        name: name.into(), value: 0, flags, speed_min: 0, speed_max: 255, brightness_min: 0, brightness_max: 255, colors_min: 1, colors_max: 1,
        speed: 127, brightness: 255, direction: 0, color_mode: 0, colors: vec![0xff],
    };
    crate::lighting::DeviceInfo {
        name: "Keychron V3 Max (demo)".into(),
        vendor: "Keychron".into(),
        kind: 5,
        leds: 87,
        zones: vec![("Keyboard".into(), 87, 87, 87)],
        layout: vec![Zone { name: "Keyboard".into(), kind: 2, leds_min: 87, leds_max: 87, leds_count: 87, matrix: Some((6, 17, map)) }],
        led_names,
        modes: vec![mode("Solid Color", 0x140), mode("Breathing", 0x141), mode("Cycle All", 0x191), mode("Typing Heatmap", 0x190), mode("Splash", 0x191)],
    }
}

fn dot_label(ui: &mut egui::Ui, color: Color32, text: &str) {
    ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend); // move whole widgets to the next line instead of squashing them
        ui.spacing_mut().item_spacing.x = 6.0;
        let (rect, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 4.0, color);
        ui.label(RichText::new(text).size(12.0).color(color));
    });
}

fn short_hw(h: &str) -> String {
    h.replace("NVIDIA GeForce ", "").replace("AMD Ryzen ", "Ryzen ").replace("Nuvoton ", "")
}

fn stat(ui: &mut egui::Ui, label: &str, temp: Option<f64>, load: Option<f64>) {
    card(ui, |ui| {
        sub(ui, label);
        ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend); // move whole widgets to the next line instead of squashing them
            match temp {
                Some(t) => ui.label(RichText::new(format!("{t:.0}°")).size(34.0).strong().color(temp_color(t))),
                None => ui.label(RichText::new("—").size(34.0).color(MUTED)),
            };
            if let Some(l) = load {
                ui.label(RichText::new(format!("{l:.0}% load")).color(MUTED));
            }
        });
    });
}

fn history_chart(ui: &mut egui::Ui, h: &std::collections::VecDeque<(Option<f64>, Option<f64>)>) {
    let (resp, p) = ui.allocate_painter(vec2(ui.available_width(), 170.0), Sense::hover());
    let r = resp.rect;
    let (lo, hi) = (20.0, 100.0);
    let y = |t: f64| r.bottom() - ((t - lo) / (hi - lo)).clamp(0.0, 1.0) as f32 * r.height();
    for t in [40.0, 60.0, 80.0] {
        p.line_segment([pos2(r.left(), y(t)), pos2(r.right(), y(t))], Stroke::new(1.0, LINE));
        p.text(pos2(r.left() + 2.0, y(t) - 2.0), Align2::LEFT_BOTTOM, format!("{t:.0}°"), FontId::proportional(10.0), MUTED);
    }
    let n = crate::engine::HISTORY as f32;
    let x = |i: usize, len: usize| r.right() - (len - 1 - i) as f32 / (n - 1.0) * r.width();
    for (sel, color) in [(0, VIOLET), (1, CYAN)] {
        let mut seg: Vec<Pos2> = vec![];
        for (i, e) in h.iter().enumerate() {
            match if sel == 0 { e.0 } else { e.1 } {
                Some(t) => seg.push(pos2(x(i, h.len()), y(t))),
                None => {
                    if seg.len() > 1 {
                        p.add(egui::Shape::line(std::mem::take(&mut seg), Stroke::new(2.0, color)));
                    }
                    seg.clear();
                }
            }
        }
        if seg.len() > 1 {
            p.add(egui::Shape::line(seg, Stroke::new(2.0, color)));
        }
    }
}

fn preview_strip(ui: &mut egui::Ui, l: &crate::config::LightingSettings, device: &str, t: f64, temps: &HashMap<String, f64>) {
    let n = 48;
    let preview = crate::config::LightingSettings { on: true, ..l.clone() };
    let colors = crate::lighting::frame(&preview, device, n, t, temps, false);
    let (resp, p) = ui.allocate_painter(vec2(ui.available_width(), 22.0), Sense::hover());
    let w = resp.rect.width() / n as f32;
    for (i, c) in colors.iter().enumerate() {
        let r = Rect::from_min_size(pos2(resp.rect.left() + i as f32 * w, resp.rect.top()), vec2(w - 2.0, 22.0));
        p.rect_filled(r, CornerRadius::same(4), rgb(*c));
    }
}

fn color_row(ui: &mut egui::Ui, c: &mut Rgb) {
    ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend); // move whole widgets to the next line instead of squashing them
        ui.color_edit_button_srgb(c);
        for (sw, name) in SWATCHES {
            let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
            let p = ui.painter();
            p.circle_filled(rect.center(), 10.0, rgb(sw));
            if *c == sw {
                p.circle_stroke(rect.center(), 11.5, Stroke::new(2.0, TEXT));
            }
            if resp.on_hover_text(name).clicked() {
                *c = sw;
            }
        }
    });
}

fn effect_editor(ui: &mut egui::Ui, e: &mut Effect, sensors: &[Sensor], salt: &str) {
    let current = e.label();
    ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend); // move whole widgets to the next line instead of squashing them
        let color = match e {
            Effect::Static { color } | Effect::Breathing { color, .. } => *color,
            _ => [255, 255, 255],
        };
        let cpu = crate::cooling::cpu_temperature(sensors).or_else(|| sensors.first()).map(|x| x.id.clone()).unwrap_or_default();
        let options: [Effect; 7] = [
            Effect::Static { color },
            Effect::Breathing { color, period_s: 4.0 },
            Effect::Spectrum { period_s: 12.0 },
            Effect::Wave { period_s: 6.0, spread: 1.0 },
            Effect::Gradient { from: [140, 70, 255], to: [0, 220, 255] },
            Effect::Temperature { sensor: cpu, cool: [0, 140, 255], hot: [255, 30, 30], low: 40.0, high: 85.0 },
            Effect::Off,
        ];
        for o in options {
            if ui.selectable_label(current == o.label(), o.label()).clicked() && current != o.label() {
                *e = o;
            }
        }
    });
    match e {
        Effect::Static { color } => color_row(ui, color),
        Effect::Breathing { color, period_s } => {
            color_row(ui, color);
            ui.add(egui::Slider::new(period_s, 1.0..=20.0).text("seconds per breath"));
        }
        Effect::Spectrum { period_s } => {
            ui.add(egui::Slider::new(period_s, 2.0..=120.0).text("seconds per cycle"));
        }
        Effect::Wave { period_s, spread } => {
            ui.add(egui::Slider::new(period_s, 1.0..=60.0).text("seconds per cycle"));
            ui.add(egui::Slider::new(spread, 0.1..=4.0).text("rainbows across strip"));
        }
        Effect::Gradient { from, to } => {
            ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend); // move whole widgets to the next line instead of squashing them
                ui.label("From");
                ui.color_edit_button_srgb(from);
                ui.label("to");
                ui.color_edit_button_srgb(to);
            });
        }
        Effect::Temperature { sensor, cool, hot, low, high } => {
            let label = sensors.iter().find(|x| &x.id == sensor).map(|x| format!("{} · {}", short_hw(&x.hardware), x.name)).unwrap_or_else(|| sensor.clone());
            egui::ComboBox::from_id_salt(("tsens", salt)).selected_text(label).width(300.0).show_ui(ui, |ui| {
                for x in sensors {
                    ui.selectable_value(sensor, x.id.clone(), format!("{} · {}  {}", short_hw(&x.hardware), x.name, fmt_sensor(x)));
                }
            });
            ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend); // move whole widgets to the next line instead of squashing them
                ui.color_edit_button_srgb(cool);
                ui.label("at");
                ui.add(egui::DragValue::new(low).range(0.0..=105.0).suffix("°C"));
                ui.label("to");
                ui.color_edit_button_srgb(hot);
                ui.label("at");
                ui.add(egui::DragValue::new(high).range(1.0..=110.0).suffix("°C"));
            });
            if *high <= *low {
                *high = *low + 1.0;
            }
        }
        Effect::Off => {}
    }
}

fn time_edit(ui: &mut egui::Ui, minutes: &mut u32, salt: &str) {
    let mut h = *minutes / 60;
    let mut m = *minutes % 60;
    ui.push_id(salt, |ui| {
        ui.add(egui::DragValue::new(&mut h).range(0..=23).custom_formatter(|v, _| format!("{v:02}")));
        ui.label(":");
        ui.add(egui::DragValue::new(&mut m).range(0..=59).speed(1.0).custom_formatter(|v, _| format!("{v:02}")));
    });
    *minutes = h * 60 + m;
}

fn profile_combo(ui: &mut egui::Ui, value: &mut String, profiles: &[String], salt: &str) {
    egui::ComboBox::from_id_salt(salt).selected_text(value.as_str()).show_ui(ui, |ui| {
        for p in profiles {
            ui.selectable_value(value, p.clone(), p);
        }
    });
}
