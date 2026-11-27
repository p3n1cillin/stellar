//! Cooling page: fan cards, per-fan controls, the reusable curve library and
//! auto-tune — the Fan Control workflow, with safety built in.

use super::*;
use crate::config::{CalibrationPoint, CurveDef, CurveKind, CurvePoint, CurvePreset, FanMode, FanRule, Mix, preset};
use crate::cooling::{Decision, Sensor, paired_rpm, temperature_for_output, tune};
use crate::curves::{Evaluator, MixFn, interpolate, new_id};

impl App {
    pub(super) fn cooling(&mut self, ui: &mut egui::Ui) {
        let s = self.state.lock().unwrap();
        let sensors = s.snapshot.sensors.clone();
        let decisions = s.decisions.clone();
        let tuning = s.tuning.clone();
        let competing = s.competing;
        drop(s);
        let temps: HashMap<String, f64> = sensors.iter().filter(|x| x.kind == "Temperature").filter_map(|x| Some((x.id.clone(), x.value?))).collect();

        ui.horizontal(|ui| {
            heading(ui, "Cooling");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if tuning.is_none() {
                    let measured = self.committed.fans.iter().any(|r| !r.calibration.is_empty());
                    let b = egui::Button::new(RichText::new(if measured { "Re-run auto-tune" } else { "Auto-tune all fans" }).strong()).fill(VIOLET);
                    if ui
                        .add_enabled(competing.is_none() && !self.cooling_dirty(), b)
                        .on_hover_text("Sweeps every fan header from 100% to 0%, measures where each fan or hub stalls and restarts, then builds quiet CPU, GPU and case curves. About 1 minute per connected header; safeguards stay on.")
                        .on_disabled_hover_text("Save or discard changes first")
                        .clicked()
                    {
                        let sensors_now = self.state.lock().unwrap().snapshot.sensors.clone();
                        let all: Vec<String> = sensors_now.iter().filter(|s| s.kind == "Control" && s.can_control).map(|s| s.id.clone()).collect();
                        let default: Vec<String> = crate::tuning::targets(&sensors_now, &self.committed, None).into_iter().map(|t| t.control).collect();
                        self.tune_dialog = Some(
                            crate::tuning::targets(&sensors_now, &self.committed, Some(&all))
                                .into_iter()
                                .map(|t| {
                                    let on = default.contains(&t.control);
                                    (t.control, t.name, on)
                                })
                                .collect(),
                        );
                    }
                    if measured
                        && ui
                            .add_enabled(!self.cooling_dirty(), egui::Button::new("Rebuild curves"))
                            .on_hover_text("Re-applies tuning from the stored measurements for the selected cooler type. No sweep, fans keep running normally.")
                            .clicked()
                    {
                        self.send(Cmd::RebuildCurves(self.committed.cpu_cooler));
                    }
                    let mut cooler = self.committed.cpu_cooler;
                    egui::ComboBox::from_id_salt("cooler")
                        .selected_text(match cooler { crate::config::CpuCooler::Air => "CPU cooler: Air", crate::config::CpuCooler::Aio => "CPU cooler: AIO liquid" })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut cooler, crate::config::CpuCooler::Air, "Air cooler");
                            ui.selectable_value(&mut cooler, crate::config::CpuCooler::Aio, "AIO liquid cooler");
                        });
                    if cooler != self.committed.cpu_cooler {
                        // Saved immediately and, when measurements exist, curves are rebuilt.
                        if measured {
                            self.send(Cmd::RebuildCurves(cooler));
                        } else {
                            let mut new = self.committed.clone();
                            new.cpu_cooler = cooler;
                            self.send(Cmd::Save(new));
                        }
                    }
                }
            });
        });
        if let Some(c) = competing {
            ui.label(RichText::new(format!("{c} is running, so Stellar leaves every fan on firmware control. Quit it to let Stellar drive fans.")).color(GOLD));
        }
        if let Some(g) = self.state.lock().unwrap().gpu_competing {
            ui.label(RichText::new(format!("{g} is running, so the graphics card's fans are left to it.")).color(MUTED));
        }
        self.pawnio_banner(ui);
        self.tune_dialog_window(ui.ctx());
        if let Some(t) = &tuning {
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Auto-tune {} of {}: {} — {}", t.index, t.total, t.name, t.phase)).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Cancel").clicked() {
                            self.send(Cmd::CancelTuning);
                        }
                        ui.add(egui::ProgressBar::new((t.percent / 100.0) as f32).desired_width(200.0).text(format!("duty {:.0}%", t.percent)).fill(CYAN));
                    });
                });
                ui.label(RichText::new("Fans will change speed and briefly stop. Tuning aborts automatically if the CPU or GPU reaches 85°C.").size(12.0).color(MUTED));
            });
        }
        ui.add_space(6.0);

        // ---------------------------------------------------------- fan cards
        let mut outputs: Vec<(String, String, String)> = sensors
            .iter()
            .filter(|x| x.kind == "Control" && x.can_control)
            .map(|x| (x.id.clone(), x.name.clone(), x.hardware.clone()))
            .collect();
        for r in &self.draft.fans {
            if !outputs.iter().any(|o| o.0 == r.control_id) {
                outputs.push((r.control_id.clone(), r.name.clone(), "not detected".into()));
            }
        }
        let rpm_of = |rule: Option<&FanRule>, id: &str| {
            let rid = rule.and_then(|r| r.rpm_sensor_id.clone()).unwrap_or_else(|| paired_rpm(id));
            sensors.iter().find(|x| x.id == rid).and_then(|x| x.value)
        };
        let unused: Vec<String> = outputs
            .iter()
            .filter(|(id, ..)| self.draft.rule(id).is_none() && rpm_of(None, id).unwrap_or(0.0) < 1.0)
            .map(|o| o.0.clone())
            .collect();
        if self.selected.is_none() {
            self.selected = outputs.iter().find(|o| !unused.contains(&o.0)).map(|o| o.0.clone());
        }
        ui.horizontal(|ui| {
            sub(ui, "FANS");
            if !unused.is_empty() {
                ui.checkbox(&mut self.show_unused, format!("show {} unused headers", unused.len()));
            }
        });
        ui.horizontal_wrapped(|ui| {
            for (id, name, hw) in &outputs {
                if unused.contains(id) && !self.show_unused {
                    continue;
                }
                let rule = self.draft.rule(id);
                let duty = sensors.iter().find(|x| &x.id == id).and_then(|x| x.value);
                let rpm = rpm_of(rule, id);
                let selected = self.selected.as_deref() == Some(id);
                let title = rule.map(|r| r.name.clone()).unwrap_or_else(|| name.clone());
                let mode = rule.map(|r| r.mode);
                let curve_name = rule.and_then(|r| r.curve.as_ref()).and_then(|c| self.draft.curves.iter().find(|x| &x.id == c)).map(|c| c.name.clone());
                let emergency = decisions.get(id).is_some_and(|d| d.emergency);
                let resp = egui::Frame::new()
                    .fill(if selected { VIOLET.gamma_multiply(0.22) } else { CARD })
                    .stroke(Stroke::new(1.0, if emergency { BAD } else if selected { VIOLET } else { LINE }))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| ui.vertical(|ui| {
                        ui.set_width(210.0);
                        ui.label(RichText::new(&title).strong());
                        ui.label(RichText::new(short_hw(hw)).size(11.0).color(MUTED));
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(duty.map(|d| format!("{d:.0}%")).unwrap_or("—".into())).size(22.0));
                            ui.label(RichText::new(rpm.map(|r| format!("{r:.0} RPM")).unwrap_or_default()).color(MUTED));
                        });
                        let (text, color) = match mode {
                            Some(FanMode::Curve) => (format!("Curve · {}", curve_name.unwrap_or_else(|| "?".into())), CYAN),
                            Some(FanMode::Manual) => ("Manual".to_string(), CYAN),
                            Some(FanMode::Firmware) => ("Firmware".to_string(), MUTED),
                            None => ("Not managed".to_string(), MUTED),
                        };
                        if tuning.as_ref().is_some_and(|t| &t.control == id) {
                            chip(ui, "Tuning…", GOLD);
                        } else {
                            chip(ui, &text, if emergency { BAD } else { color });
                        }
                    }))
                    .response
                    .interact(Sense::click());
                if resp.clicked() {
                    self.selected = Some(id.clone());
                }
            }
        });
        ui.add_space(8.0);
        if let Some(id) = self.selected.clone() {
            self.fan_editor(ui, &id, &sensors, &temps, decisions.get(&id), tuning.is_some() || competing.is_some());
        }

        // ---------------------------------------------------------- curves
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            sub(ui, "CURVES");
            ui.label(RichText::new("Reusable — any number of fans can follow one curve.").size(12.0).color(MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let first_two: Vec<String> = self.draft.curves.iter().take(2).map(|c| c.id.clone()).collect();
                ui.menu_button("New curve", |ui| {
                    let cpu = crate::cooling::cpu_temperature(&sensors).map(|x| x.id.clone()).or_else(|| temps.keys().next().cloned()).unwrap_or_default();
                    let mut add = |name: &str, kind: CurveKind| {
                        let id = new_id();
                        self.draft.curves.push(CurveDef { response_s: 0.0, id: id.clone(), name: name.into(), kind });
                        self.selected_curve = Some(id);
                    };
                    if ui.button("Graph — temperature points").clicked() {
                        add("New graph", CurveKind::Graph { sensors: vec![cpu.clone()], mix: Mix::Max, offset: 0.0, points: preset(CurvePreset::Balanced) });
                        ui.close();
                    }
                    if ui.button("Linear — between two temperatures").clicked() {
                        add("New linear", CurveKind::Linear { sensor: cpu.clone(), low_temp: 40.0, high_temp: 80.0, low_percent: 30.0, high_percent: 100.0 });
                        ui.close();
                    }
                    if ui.button("Flat — constant speed").clicked() {
                        add("New flat", CurveKind::Flat { percent: 50.0 });
                        ui.close();
                    }
                    if ui.button("Trigger — idle/load with hysteresis").clicked() {
                        add("New trigger", CurveKind::Trigger { sensor: cpu.clone(), idle_temp: 55.0, load_temp: 70.0, idle_percent: 30.0, load_percent: 70.0 });
                        ui.close();
                    }
                    if ui.button("Mix — combine curves").clicked() {
                        add("New mix", CurveKind::Mix { curves: first_two.clone(), function: MixFn::Max });
                        ui.close();
                    }
                });
            });
        });
        if self.draft.curves.is_empty() {
            ui.label(RichText::new("No curves yet. Run Auto-tune to create tuned CPU, GPU and Case curves, or add one.").color(MUTED));
        }
        let mut eval = Evaluator::default();
        let draft_curves = self.draft.curves.clone();
        ui.horizontal_wrapped(|ui| {
            for c in &draft_curves {
                let selected = self.selected_curve.as_deref() == Some(c.id.as_str());
                let value = eval.eval(&draft_curves, &c.id, &temps);
                let users = self.draft.fans.iter().filter(|r| r.mode == FanMode::Curve && r.curve.as_deref() == Some(c.id.as_str())).count();
                let resp = egui::Frame::new()
                    .fill(if selected { VIOLET.gamma_multiply(0.22) } else { CARD })
                    .stroke(Stroke::new(1.0, if selected { VIOLET } else { LINE }))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| ui.vertical(|ui| {
                        ui.set_width(180.0);
                        ui.label(RichText::new(&c.name).strong());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(value.map(|v| format!("{v:.0}%")).unwrap_or("—".into())).size(20.0));
                            ui.label(RichText::new(format!("{} · {} fan{}", c.kind.label(), users, if users == 1 { "" } else { "s" })).size(11.5).color(MUTED));
                        });
                    }))
                    .response
                    .interact(Sense::click());
                if resp.clicked() {
                    self.selected_curve = Some(c.id.clone());
                }
            }
        });
        if let Some(cid) = self.selected_curve.clone() {
            ui.add_space(8.0);
            self.curve_editor_card(ui, &cid, &sensors, &temps);
        }
    }

    fn tune_dialog_window(&mut self, ctx: &egui::Context) {
        let Some(list) = &mut self.tune_dialog else { return };
        let mut start = None;
        let mut close = false;
        egui::Window::new("Auto-tune").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.label("Each ticked header is swept from 100% down to 0% and back up, so its fans speed up, slow down and briefly stop. About a minute per connected header.");
            ui.label(RichText::new("Untick any header that powers a pump (AIO or custom loop). Headers that look like pumps are unticked for you.").color(GOLD));
            ui.add_space(6.0);
            if list.is_empty() {
                ui.label(RichText::new("No fan headers with RPM sensors were found.").color(MUTED));
            }
            for (_, name, on) in list.iter_mut() {
                ui.checkbox(on, name.as_str());
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let chosen: Vec<String> = list.iter().filter(|x| x.2).map(|x| x.0.clone()).collect();
                if ui.add_enabled(!chosen.is_empty(), egui::Button::new(RichText::new("Start auto-tune").strong()).fill(VIOLET)).clicked() {
                    start = Some(chosen);
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if let Some(c) = start {
            self.send(Cmd::AutoTune(Some(c)));
            close = true;
        }
        if close {
            self.tune_dialog = None;
        }
    }

    /// Offer to install PawnIO when motherboard sensors are missing.
    pub(super) fn pawnio_banner(&mut self, ui: &mut egui::Ui) {
        let (pawnio, board) = {
            let s = self.state.lock().unwrap();
            (s.pawnio, s.snapshot.sensors.iter().any(|x| x.id.starts_with("/lpc/")))
        };
        if pawnio || board {
            return;
        }
        card(ui, |ui| {
            ui.label(RichText::new("Motherboard fans and CPU temperature need the PawnIO driver").strong().color(GOLD));
            ui.label(RichText::new("PawnIO is the free, signed driver LibreHardwareMonitor uses to reach fan headers and CPU sensors. Until it's installed, only graphics card fans can be controlled.").size(12.5).color(MUTED));
            ui.horizontal(|ui| {
                let busy = self.installing.lock().unwrap().clone();
                match busy {
                    Some(what) => {
                        ui.spinner();
                        ui.label(format!("Installing {what}…"));
                    }
                    None => {
                        if ui.add(egui::Button::new("Install PawnIO").fill(VIOLET)).clicked() {
                            self.install_tool("PawnIO", "namazso.PawnIO", true);
                        }
                        ui.hyperlink_to("pawnio.eu", "https://pawnio.eu");
                    }
                }
            });
        });
        ui.add_space(6.0);
    }

    /// Install a winget package in the background; optionally restart the
    /// hardware host afterwards so new sensors appear.
    pub(super) fn install_tool(&mut self, label: &str, id: &str, restart_hw: bool) {
        let flag = self.installing.clone();
        *flag.lock().unwrap() = Some(label.to_string());
        let tx = self.tx.clone();
        let (label, id) = (label.to_string(), id.to_string());
        std::thread::spawn(move || {
            let r = crate::install::winget_install(&id);
            if let Err(e) = r {
                crate::log(&format!("Installing {label}: {e:#}"));
            }
            if restart_hw {
                let _ = tx.send(Cmd::RestartHardware);
            } else {
                let _ = tx.send(Cmd::ReconnectLighting);
            }
            *flag.lock().unwrap() = None;
        });
    }

    fn fan_editor(&mut self, ui: &mut egui::Ui, id: &str, sensors: &[Sensor], temps: &HashMap<String, f64>, decision: Option<&Decision>, busy: bool) {
        let Some(idx) = self.draft.fans.iter().position(|r| r.control_id == id) else {
            card(ui, |ui| {
                ui.label(RichText::new("This header is not managed by Stellar and stays on motherboard/GPU firmware control.").color(MUTED));
                if ui.add(egui::Button::new("Manage this fan").fill(VIOLET)).clicked() {
                    let out = sensors.iter().find(|x| x.id == id);
                    let rpm = paired_rpm(id);
                    self.draft.fans.push(FanRule {
                        control_id: id.to_string(),
                        name: out.map(|o| o.name.clone()).unwrap_or_else(|| id.to_string()),
                        rpm_sensor_id: sensors.iter().any(|x| x.id == rpm).then_some(rpm),
                        sensors: out.and_then(|o| temperature_for_output(sensors, o)).map(|t| t.id.clone()).into_iter().collect(),
                        ..Default::default()
                    });
                }
            });
            return;
        };
        let curves = self.draft.curves.clone();
        let mut remove = false;
        let r = &mut self.draft.fans[idx];
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut r.name).desired_width(240.0).font(FontId::proportional(17.0)));
                for (m, l) in [(FanMode::Firmware, "Firmware"), (FanMode::Curve, "Curve"), (FanMode::Manual, "Manual")] {
                    ui.selectable_value(&mut r.mode, m, l);
                }
                if let Some(d) = decision {
                    chip(ui, &match d.percent { Some(p) => format!("{p:.0}% · {}", d.reason), None => d.reason.clone() }, if d.emergency { BAD } else { CYAN });
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.small_button("Stop managing").on_hover_text("Returns this header to firmware control after Save").clicked() {
                        remove = true;
                    }
                });
            });
            match r.mode {
                FanMode::Firmware => {
                    ui.label(RichText::new("The motherboard or graphics card controls this fan.").color(MUTED));
                }
                FanMode::Curve => {
                    ui.horizontal(|ui| {
                        ui.label("Follows curve");
                        let current = r.curve.as_ref().and_then(|c| curves.iter().find(|x| &x.id == c)).map(|c| c.name.clone()).unwrap_or_else(|| "Choose…".into());
                        egui::ComboBox::from_id_salt("fan-curve").selected_text(current).width(200.0).show_ui(ui, |ui| {
                            for c in &curves {
                                ui.selectable_value(&mut r.curve, Some(c.id.clone()), format!("{} ({})", c.name, c.kind.label()));
                            }
                        });
                        if let Some(c) = &r.curve
                            && ui.small_button("Edit curve").clicked()
                        {
                            self.selected_curve = Some(c.clone());
                        }
                    });
                }
                FanMode::Manual => {
                    ui.add(egui::Slider::new(&mut r.manual_percent, 0.0..=100.0).suffix("%").text("speed"));
                }
            }
        });
        ui.add_space(6.0);
        ui.columns(2, |c| {
            card(&mut c[0], |ui| {
                sub(ui, "SPEED LIMITS & RESPONSE");
                egui::Grid::new("limits").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                    ui.label("Minimum (stop)").on_hover_text("Lowest speed while running. Auto-tune measures this.");
                    ui.add(egui::DragValue::new(&mut r.minimum_percent).range(0.0..=100.0).suffix("%"));
                    ui.end_row();
                    ui.label("Start").on_hover_text("Speed used to spin the fan up from standstill.");
                    ui.add(egui::DragValue::new(&mut r.start_percent).range(0.0..=100.0).suffix("%"));
                    ui.end_row();
                    ui.label("Allow fan to stop");
                    ui.checkbox(&mut r.allow_stop, "below minimum");
                    ui.end_row();
                    ui.label("Offset");
                    ui.add(egui::DragValue::new(&mut r.offset_percent).range(-50.0..=50.0).suffix("%"));
                    ui.end_row();
                    ui.label("Step up");
                    ui.add(egui::DragValue::new(&mut r.step_up_per_sec).range(1.0..=100.0).suffix("%/s"));
                    ui.end_row();
                    ui.label("Step down");
                    ui.add(egui::DragValue::new(&mut r.ramp_down_per_sec).range(0.5..=100.0).speed(0.2).suffix("%/s"));
                    ui.end_row();
                    ui.label("Hysteresis");
                    ui.add(egui::DragValue::new(&mut r.hysteresis).range(0.0..=10.0).speed(0.1).suffix("°C"));
                    ui.end_row();
                });
            });
            card(&mut c[1], |ui| {
                sub(ui, "SAFETY");
                egui::Grid::new("safety").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                    ui.label("Critical temperature");
                    ui.add(egui::DragValue::new(&mut r.critical_temp).range(50.0..=100.0).suffix("°C"));
                    ui.end_row();
                    ui.label("RPM sensor");
                    let current = r.rpm_sensor_id.as_ref().and_then(|id| sensors.iter().find(|x| &x.id == id)).map(|x| x.name.clone()).unwrap_or_else(|| "None".into());
                    egui::ComboBox::from_id_salt("rpm").selected_text(current).width(160.0).truncate().show_ui(ui, |ui| {
                        ui.selectable_value(&mut r.rpm_sensor_id, None, "None");
                        for f in sensors.iter().filter(|x| x.kind == "Fan") {
                            ui.selectable_value(&mut r.rpm_sensor_id, Some(f.id.clone()), format!("{} · {}  {}", short_hw(&f.hardware), f.name, fmt_sensor(f)));
                        }
                    });
                    ui.end_row();
                });
                ui.collapsing(format!("Extra failsafe sensors ({})", r.sensors.len()), |ui| {
                    for t in sensors.iter().filter(|x| x.kind == "Temperature") {
                        let mut on = r.sensors.contains(&t.id);
                        if ui.checkbox(&mut on, format!("{} · {}  {}", short_hw(&t.hardware), t.name, fmt_sensor(t))).changed() {
                            if on { r.sensors.push(t.id.clone()) } else { r.sensors.retain(|x| x != &t.id) }
                        }
                    }
                });
                ui.label(RichText::new("Any watched sensor at the critical limit, missing or stale sensors, or a stalled fan (<100 RPM for 15 s) force 100% instantly.").size(11.5).color(MUTED));
                let _ = temps;
            });
        });
        ui.add_space(6.0);
        card(ui, |ui| {
            ui.horizontal(|ui| {
                sub(ui, "CALIBRATION");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add_enabled(!busy, egui::Button::new("Measure this fan")).on_hover_text("Sweeps only this header (≈1 minute) and stores its RPM table and limits.").clicked() {
                        let _ = self.tx.send(Cmd::TuneFan(id.to_string()));
                    }
                    if let Some(t) = tune(&r.calibration, None)
                        && (r.minimum_percent != t.minimum || r.start_percent != t.start)
                        && ui.button(format!("Use measured limits ({:.0}% / {:.0}%)", t.minimum, t.start)).clicked()
                    {
                        r.minimum_percent = t.minimum;
                        r.start_percent = t.start;
                    }
                });
            });
            if r.calibration.is_empty() {
                ui.label(RichText::new("Not measured yet.").color(MUTED));
            } else {
                if let Some(t) = tune(&r.calibration, None) {
                    ui.label(RichText::new(&t.summary).size(12.0).color(MUTED));
                }
                calibration_chart(ui, &r.calibration, r.minimum_percent);
            }
        });
        if remove {
            self.draft.fans.remove(idx);
        } else if let Err(e) = self.draft.fans[idx].validate_with(&self.draft.curves) {
            ui.label(RichText::new(format!("{e}")).color(BAD));
        }
    }

    fn curve_editor_card(&mut self, ui: &mut egui::Ui, cid: &str, sensors: &[Sensor], temps: &HashMap<String, f64>) {
        let Some(idx) = self.draft.curves.iter().position(|c| c.id == cid) else {
            self.selected_curve = None;
            return;
        };
        let users: Vec<String> = self.draft.fans.iter().filter(|r| r.curve.as_deref() == Some(cid)).map(|r| r.name.clone()).collect();
        let mixers: Vec<String> = self
            .draft
            .curves
            .iter()
            .filter(|c| matches!(&c.kind, CurveKind::Mix { curves, .. } if curves.iter().any(|x| x == cid)))
            .map(|c| c.name.clone())
            .collect();
        let others: Vec<(String, String)> = self.draft.curves.iter().filter(|c| c.id != cid).map(|c| (c.id.clone(), c.name.clone())).collect();
        let snapshot = self.draft.curves.clone();
        let live = Evaluator::default().eval(&snapshot, cid, temps);
        let mut delete = false;
        let temp_sensors: Vec<&Sensor> = sensors.iter().filter(|x| x.kind == "Temperature").collect();
        let label = |id: &str| sensors.iter().find(|x| x.id == id).map(|x| format!("{} · {}", short_hw(&x.hardware), x.name)).unwrap_or_else(|| format!("missing: {id}"));
        let c = &mut self.draft.curves[idx];
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut c.name).desired_width(220.0).font(FontId::proportional(17.0)));
                chip(ui, c.kind.label(), CYAN);
                if !matches!(c.kind, CurveKind::Flat { .. } | CurveKind::Mix { .. }) {
                    ui.label("Response");
                    ui.add(egui::DragValue::new(&mut c.response_s).range(0.0..=60.0).speed(0.5).suffix(" s"))
                        .on_hover_text("Smooths the temperature so fans follow the trend, not spikes. Safety limits always use the raw reading.");
                }
                if let Some(v) = live {
                    chip(ui, &format!("now {v:.0}%"), GOOD);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let in_use = !users.is_empty() || !mixers.is_empty();
                    let why = format!("Used by {}", users.iter().chain(mixers.iter()).cloned().collect::<Vec<_>>().join(", "));
                    if ui.add_enabled(!in_use, egui::Button::new("Delete curve")).on_disabled_hover_text(why).clicked() {
                        delete = true;
                    }
                });
            });
            let sensor_combo = |ui: &mut egui::Ui, value: &mut String, salt: &str| {
                egui::ComboBox::from_id_salt(salt).selected_text(label(value)).width(280.0).show_ui(ui, |ui| {
                    for x in &temp_sensors {
                        ui.selectable_value(value, x.id.clone(), format!("{} · {}  {}", short_hw(&x.hardware), x.name, fmt_sensor(x)));
                    }
                });
            };
            match &mut c.kind {
                CurveKind::Graph { sensors: srcs, mix, offset, points } => {
                    ui.horizontal(|ui| {
                        ui.menu_button(format!("Sensors ({})", srcs.len()), |ui| {
                            for x in &temp_sensors {
                                let mut on = srcs.contains(&x.id);
                                if ui.checkbox(&mut on, format!("{} · {}  {}", short_hw(&x.hardware), x.name, fmt_sensor(x))).changed() {
                                    if on { srcs.push(x.id.clone()) } else { srcs.retain(|s| s != &x.id) }
                                }
                            }
                        });
                        ui.label(RichText::new(srcs.iter().map(|s| label(s)).collect::<Vec<_>>().join(", ")).size(12.0).color(MUTED));
                    });
                    ui.horizontal(|ui| {
                        ui.label("Combine");
                        ui.selectable_value(mix, Mix::Max, "Hottest");
                        ui.selectable_value(mix, Mix::Average, "Average");
                        ui.label("Offset");
                        ui.add(egui::DragValue::new(offset).range(-20.0..=20.0).speed(0.2).suffix("°C"));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            for (p, l) in [(CurvePreset::Performance, "Performance"), (CurvePreset::Balanced, "Balanced"), (CurvePreset::Silent, "Silent")] {
                                if ui.small_button(l).clicked() {
                                    *points = preset(p);
                                }
                            }
                        });
                    });
                    let vals: Vec<f64> = srcs.iter().filter_map(|s| temps.get(s).copied()).collect();
                    let t = (!vals.is_empty()).then(|| match mix {
                        Mix::Max => vals.iter().copied().fold(f64::MIN, f64::max),
                        Mix::Average => vals.iter().sum::<f64>() / vals.len() as f64,
                    } + *offset);
                    curve_editor(ui, points, t);
                    ui.label(RichText::new("Drag points · double-click to add · right-click a point to remove").size(11.5).color(MUTED));
                }
                CurveKind::Linear { sensor, low_temp, high_temp, low_percent, high_percent } => {
                    sensor_combo(ui, sensor, "lin-sensor");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(low_percent).range(0.0..=100.0).suffix("%"));
                        ui.label("at");
                        ui.add(egui::DragValue::new(low_temp).range(0.0..=109.0).suffix("°C"));
                        ui.label("rising to");
                        ui.add(egui::DragValue::new(high_percent).range(0.0..=100.0).suffix("%"));
                        ui.label("at");
                        ui.add(egui::DragValue::new(high_temp).range(1.0..=110.0).suffix("°C"));
                    });
                    *high_temp = high_temp.max(*low_temp + 1.0);
                    *high_percent = high_percent.max(*low_percent);
                }
                CurveKind::Flat { percent } => {
                    ui.add(egui::Slider::new(percent, 0.0..=100.0).suffix("%"));
                }
                CurveKind::Trigger { sensor, idle_temp, load_temp, idle_percent, load_percent } => {
                    sensor_combo(ui, sensor, "trig-sensor");
                    ui.horizontal(|ui| {
                        ui.label("Load");
                        ui.add(egui::DragValue::new(load_percent).range(0.0..=100.0).suffix("%"));
                        ui.label("from");
                        ui.add(egui::DragValue::new(load_temp).range(1.0..=110.0).suffix("°C"));
                        ui.label("· back to idle");
                        ui.add(egui::DragValue::new(idle_percent).range(0.0..=100.0).suffix("%"));
                        ui.label("at");
                        ui.add(egui::DragValue::new(idle_temp).range(0.0..=109.0).suffix("°C"));
                    });
                    *load_temp = load_temp.max(*idle_temp + 1.0);
                    *load_percent = load_percent.max(*idle_percent);
                }
                CurveKind::Mix { curves: refs, function } => {
                    ui.horizontal(|ui| {
                        for (f, l) in [(MixFn::Max, "Highest"), (MixFn::Average, "Average"), (MixFn::Min, "Lowest"), (MixFn::Sum, "Sum")] {
                            ui.selectable_value(function, f, l);
                        }
                    });
                    for (oid, name) in &others {
                        let mut on = refs.contains(oid);
                        if ui.checkbox(&mut on, name).changed() {
                            if on { refs.push(oid.clone()) } else { refs.retain(|x| x != oid) }
                        }
                    }
                }
            }
        });
        if delete {
            self.draft.curves.remove(idx);
            self.selected_curve = None;
        } else if let Err(e) = crate::config::validate_curves(&self.draft.curves) {
            ui.label(RichText::new(format!("{e:#}")).color(BAD));
        }
    }
}

pub(super) fn curve_editor(ui: &mut egui::Ui, points: &mut Vec<CurvePoint>, live_temp: Option<f64>) {
    let (resp, p) = ui.allocate_painter(vec2(ui.available_width(), 240.0), Sense::click_and_drag());
    let rect = resp.rect.shrink2(vec2(34.0, 12.0));
    let (t0, t1) = (20.0, 105.0);
    let to_screen = |t: f64, pc: f64| pos2(rect.left() + ((t - t0) / (t1 - t0)) as f32 * rect.width(), rect.bottom() - (pc / 100.0) as f32 * rect.height());
    let from_screen = |q: Pos2| (t0 + ((q.x - rect.left()) / rect.width()) as f64 * (t1 - t0), ((rect.bottom() - q.y) / rect.height()) as f64 * 100.0);
    p.rect_filled(rect, CornerRadius::same(6), Color32::from_rgb(14, 17, 34));
    for pc in [25.0, 50.0, 75.0, 100.0] {
        let a = to_screen(t0, pc);
        p.line_segment([a, pos2(rect.right(), a.y)], Stroke::new(1.0, LINE));
        p.text(pos2(rect.left() - 4.0, a.y), Align2::RIGHT_CENTER, format!("{pc:.0}%"), FontId::proportional(10.0), MUTED);
    }
    for t in (30..=100).step_by(10) {
        let a = to_screen(t as f64, 0.0);
        p.line_segment([a, pos2(a.x, rect.top())], Stroke::new(1.0, LINE.gamma_multiply(0.6)));
        p.text(pos2(a.x, rect.bottom() + 2.0), Align2::CENTER_TOP, format!("{t}°"), FontId::proportional(10.0), MUTED);
    }
    let id = resp.id.with("drag");
    let mut dragging: Option<usize> = ui.ctx().data(|d| d.get_temp(id)).flatten();
    if resp.drag_started()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        dragging = points.iter().enumerate().map(|(i, pt)| (i, to_screen(pt.temp, pt.percent).distance(pos))).filter(|x| x.1 < 14.0).min_by(|a, b| a.1.total_cmp(&b.1)).map(|x| x.0);
    }
    if resp.dragged()
        && let (Some(i), Some(pos)) = (dragging, resp.interact_pointer_pos())
        && i < points.len()
    {
        let (t, pc) = from_screen(pos);
        let lo_t = if i > 0 { points[i - 1].temp + 1.0 } else { 0.0 };
        let hi_t = if i + 1 < points.len() { points[i + 1].temp - 1.0 } else { 110.0 };
        let lo_p = if i > 0 { points[i - 1].percent } else { 0.0 };
        let hi_p = if i + 1 < points.len() { points[i + 1].percent } else { 100.0 };
        points[i] = CurvePoint { temp: t.clamp(lo_t, hi_t.max(lo_t)).round(), percent: pc.clamp(lo_p, hi_p.max(lo_p)).round() };
    }
    if resp.drag_stopped() {
        dragging = None;
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, dragging));
    if resp.double_clicked()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let t = from_screen(pos).0.round();
        if !points.iter().any(|q| (q.temp - t).abs() < 1.0) && (0.0..=110.0).contains(&t) {
            let pc = interpolate(points, t).round();
            let at = points.iter().position(|q| q.temp > t).unwrap_or(points.len());
            points.insert(at, CurvePoint { temp: t, percent: pc });
        }
    }
    if resp.secondary_clicked()
        && let Some(pos) = resp.interact_pointer_pos()
        && points.len() > 2
        && let Some(i) = points.iter().position(|pt| to_screen(pt.temp, pt.percent).distance(pos) < 12.0)
    {
        points.remove(i);
    }
    let mut line: Vec<Pos2> = vec![to_screen(t0, points[0].percent)];
    line.extend(points.iter().map(|q| to_screen(q.temp.clamp(t0, t1), q.percent)));
    line.push(to_screen(t1, points.last().map(|q| q.percent).unwrap_or(100.0)));
    p.add(egui::Shape::line(line, Stroke::new(2.5, VIOLET)));
    let hover = resp.hover_pos();
    for (i, q) in points.iter().enumerate() {
        let c = to_screen(q.temp, q.percent);
        let hot = dragging == Some(i) || hover.is_some_and(|h| h.distance(c) < 12.0);
        p.circle_filled(c, if hot { 7.0 } else { 5.5 }, if hot { CYAN } else { TEXT });
        if hot {
            p.text(c + vec2(0.0, -10.0), Align2::CENTER_BOTTOM, format!("{:.0}°C: {:.0}%", q.temp, q.percent), FontId::proportional(11.0), TEXT);
        }
    }
    if let Some(t) = live_temp {
        let tc = t.clamp(t0, t1);
        let x = to_screen(tc, 0.0).x;
        p.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0, CYAN.gamma_multiply(0.6)));
        p.circle_filled(to_screen(tc, interpolate(points, t)), 5.0, CYAN);
        p.text(pos2(x + 4.0, rect.top() + 2.0), Align2::LEFT_TOP, format!("{t:.1}°C"), FontId::proportional(10.5), CYAN);
    }
}

fn calibration_chart(ui: &mut egui::Ui, cal: &[CalibrationPoint], minimum: f64) {
    let (resp, p) = ui.allocate_painter(vec2(ui.available_width(), 110.0), Sense::hover());
    let rect = resp.rect.shrink(8.0);
    let max = cal.iter().map(|c| c.rpm).fold(1.0, f64::max);
    let mut pts = cal.to_vec();
    pts.sort_by(|a, b| a.percent.total_cmp(&b.percent));
    let to = |pc: f64, rpm: f64| pos2(rect.left() + (pc / 100.0) as f32 * rect.width(), rect.bottom() - (rpm / max) as f32 * rect.height());
    let mx = to(minimum, 0.0).x;
    p.rect_filled(Rect::from_min_max(rect.left_top(), pos2(mx, rect.bottom())), CornerRadius::same(4), GOLD.gamma_multiply(0.08));
    p.line_segment([pos2(mx, rect.top()), pos2(mx, rect.bottom())], Stroke::new(1.0, GOLD.gamma_multiply(0.6)));
    p.text(pos2(mx + 3.0, rect.bottom()), Align2::LEFT_BOTTOM, format!("minimum {minimum:.0}%"), FontId::proportional(10.5), GOLD);
    p.add(egui::Shape::line(pts.iter().map(|c| to(c.percent, c.rpm)).collect(), Stroke::new(2.0, CYAN)));
    for c in &pts {
        p.circle_filled(to(c.percent, c.rpm), 3.0, CYAN);
    }
    p.text(rect.left_top(), Align2::LEFT_TOP, format!("max {max:.0} RPM"), FontId::proportional(10.5), MUTED);
    p.text(rect.right_bottom(), Align2::RIGHT_BOTTOM, "duty 0–100%", FontId::proportional(10.5), MUTED);
}
