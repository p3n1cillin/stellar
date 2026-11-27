//! Per-device lighting customisation, for any device OpenRGB supports:
//! built-in (hardware) effects with their own options and save-to-device,
//! and per-LED painting on the device's real layout (keyboards, mice, strips…).

use super::*;
use crate::config::{DeviceLighting, LightingSettings, OnboardMode};
use crate::lighting::{DeviceInfo, kind_label};
use crate::openrgb::{
    FLAG_HAS_BRIGHTNESS, FLAG_HAS_DIRECTION_HV, FLAG_HAS_DIRECTION_LR, FLAG_HAS_DIRECTION_UD, FLAG_HAS_MODE_SPECIFIC_COLOR,
    FLAG_HAS_RANDOM_COLOR, FLAG_HAS_SPEED, FLAG_MANUAL_SAVE, NO_LED, unpack,
};

/// What a device card asks the app to do.
pub(super) enum DeviceAction {
    SaveToDevice(String),
}

#[allow(clippy::too_many_arguments)]
pub(super) fn device_card(
    ui: &mut egui::Ui,
    d: &DeviceInfo,
    l: &mut LightingSettings,
    sensors: &[Sensor],
    temps: &HashMap<String, f64>,
    t: f64,
    brush: &mut Rgb,
    actions: &mut Vec<DeviceAction>,
) {
    let global = l.effect.clone();
    let preview_settings = LightingSettings { on: true, ..l.clone() };
    let preview = crate::lighting::frame(&preview_settings, &d.name, d.leds, t, temps, false);
    let entry = l.devices.entry(d.name.clone()).or_default();
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.checkbox(&mut entry.enabled, RichText::new(&d.name).strong().size(15.0));
            chip(ui, kind_label(d.kind), CYAN);
            ui.label(RichText::new(format!("{} · {} LED{}", d.vendor, d.leds, if d.leds == 1 { "" } else { "s" })).color(MUTED));
        });
        if !entry.enabled {
            ui.label(RichText::new("Off — Stellar keeps this device dark.").size(12.0).color(MUTED));
            return;
        }
        ui.horizontal(|ui| {
            ui.label("Lighting from");
            let mut onboard = entry.onboard.is_some();
            ui.selectable_value(&mut onboard, false, "Stellar (synced with everything)");
            ui.add_enabled_ui(!d.modes.is_empty(), |ui| {
                ui.selectable_value(&mut onboard, true, "The device's built-in effects")
                    .on_disabled_hover_text("This device has no built-in effects OpenRGB can set");
            });
            if onboard != entry.onboard.is_some() {
                entry.onboard = onboard.then(|| OnboardMode { mode: d.modes[0].name.clone(), ..Default::default() });
            }
        });
        if let Some(ob) = &mut entry.onboard {
            onboard_editor(ui, d, ob, actions);
            return;
        }
        ui.horizontal(|ui| {
            let mut own = entry.effect.is_some();
            if ui.checkbox(&mut own, "Own effect for this device").changed() {
                entry.effect = own.then(|| global.clone());
            }
            if entry.effect.is_none() {
                ui.label(RichText::new(format!("follows the global effect ({})", global.label())).size(12.0).color(MUTED));
            }
        });
        if let Some(e) = &mut entry.effect {
            ui.indent(("own", &d.name), |ui| effect_editor(ui, e, sensors, &d.name));
        }
        if d.leds > 0 {
            ui.add_space(4.0);
            // Keyboards (anything with a physical layout) open by default; long
            // strips stay folded until wanted.
            let has_layout = d.layout.iter().any(|z| z.matrix.is_some());
            egui::CollapsingHeader::new(RichText::new(format!("Per-LED colours ({} LED{})", d.leds, if d.leds == 1 { "" } else { "s" })).size(13.0))
                .id_salt(("paint", &d.name))
                .default_open(has_layout || !entry.keys.is_empty())
                .show(ui, |ui| led_painter(ui, d, entry, &preview, brush));
        }
    });
    // Zone sizes for addressable headers (where the LED count can't be detected).
    let resizable: Vec<_> = d.zones.iter().filter(|z| z.3 > z.2).collect();
    if !resizable.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("LED counts").size(12.0).color(MUTED));
            for (zone, count, min, max) in resizable {
                let sizes = l.zone_sizes.entry(d.name.clone()).or_default();
                let mut v = sizes.get(zone).copied().unwrap_or(*count);
                ui.label(RichText::new(zone).size(12.0));
                if ui.add(egui::DragValue::new(&mut v).range(*min..=*max).suffix(" LEDs")).changed() {
                    sizes.insert(zone.clone(), v);
                }
            }
        });
    }
    if l.devices.get(&d.name) == Some(&DeviceLighting::default()) {
        l.devices.remove(&d.name);
    }
}

fn onboard_editor(ui: &mut egui::Ui, d: &DeviceInfo, ob: &mut OnboardMode, actions: &mut Vec<DeviceAction>) {
    ui.label(RichText::new("The device runs this effect itself — smooth, no CPU use, and it can be saved onto the device.").size(12.0).color(MUTED));
    ui.horizontal_wrapped(|ui| {
        for m in &d.modes {
            if ui.selectable_label(ob.mode == m.name, &m.name).clicked() && ob.mode != m.name {
                *ob = OnboardMode { mode: m.name.clone(), ..Default::default() };
            }
        }
    });
    let Some(m) = d.modes.iter().find(|m| m.name == ob.mode) else {
        ui.label(RichText::new(format!("{} isn't available on this device any more", ob.mode)).color(BAD));
        return;
    };
    egui::Grid::new(("onboard", &d.name)).num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
        if m.flags & FLAG_HAS_SPEED != 0 {
            ui.label("Speed");
            range_slider(ui, &mut ob.speed, m.speed, m.speed_min, m.speed_max);
            ui.end_row();
        }
        if m.flags & FLAG_HAS_BRIGHTNESS != 0 {
            ui.label("Brightness");
            range_slider(ui, &mut ob.brightness, m.brightness, m.brightness_min, m.brightness_max);
            ui.end_row();
        }
        let dirs: Vec<(u32, &str)> = [
            (FLAG_HAS_DIRECTION_LR, [(0, "Left"), (1, "Right")].as_slice()),
            (FLAG_HAS_DIRECTION_UD, [(2, "Up"), (3, "Down")].as_slice()),
            (FLAG_HAS_DIRECTION_HV, [(4, "Horizontal"), (5, "Vertical")].as_slice()),
        ]
        .into_iter()
        .filter(|(f, _)| m.flags & f != 0)
        .flat_map(|(_, v)| v.iter().copied())
        .collect();
        if !dirs.is_empty() {
            ui.label("Direction");
            ui.horizontal(|ui| {
                let cur = ob.direction.unwrap_or(m.direction);
                for (v, label) in dirs {
                    if ui.selectable_label(cur == v, label).clicked() {
                        ob.direction = Some(v);
                    }
                }
            });
            ui.end_row();
        }
        if m.flags & FLAG_HAS_MODE_SPECIFIC_COLOR != 0 {
            ui.label("Colours");
            ui.horizontal(|ui| {
                if m.flags & FLAG_HAS_RANDOM_COLOR != 0 {
                    ui.checkbox(&mut ob.random, "Random");
                }
                if !ob.random {
                    if ob.colors.is_empty() {
                        ob.colors = m.colors.iter().map(|&c| unpack(c)).collect();
                        if ob.colors.is_empty() {
                            ob.colors.push([255, 255, 255]);
                        }
                    }
                    for c in ob.colors.iter_mut() {
                        ui.color_edit_button_srgb(c);
                    }
                    let (lo, hi) = (m.colors_min.max(1) as usize, (m.colors_max as usize).max(1));
                    if ob.colors.len() < hi && ui.small_button("+").on_hover_text("Add a colour").clicked() {
                        ob.colors.push(*ob.colors.last().unwrap_or(&[255, 255, 255]));
                    }
                    if ob.colors.len() > lo && ui.small_button("−").on_hover_text("Remove a colour").clicked() {
                        ob.colors.pop();
                    }
                }
            });
            ui.end_row();
        }
    });
    if m.flags & FLAG_MANUAL_SAVE != 0 {
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new("Save to device")).on_hover_text("Stores this effect in the device's own memory, so it keeps it with Stellar closed or on another PC").clicked() {
                actions.push(DeviceAction::SaveToDevice(d.name.clone()));
            }
            ui.label(RichText::new("Keeps this effect even without Stellar").size(12.0).color(MUTED));
        });
    }
}

/// Slider over a device range that may run backwards (some devices use
/// lower numbers for faster speeds); `None` means the device default.
fn range_slider(ui: &mut egui::Ui, value: &mut Option<u32>, default: u32, a: u32, b: u32) {
    let mut v = value.unwrap_or(default);
    let (lo, hi) = (a.min(b), a.max(b));
    if ui.add(egui::Slider::new(&mut v, lo..=hi).show_value(true)).changed() {
        *value = Some(v);
    }
}

fn short_label(name: &str) -> String {
    let n = name.trim_start_matches("Key: ").trim_start_matches("LED ").trim();
    const ABBREV: [(&str, &str); 30] = [
        ("Backspace", "Bksp"), ("Delete", "Del"), ("Insert", "Ins"), ("Page Up", "PgUp"), ("PageUp", "PgUp"),
        ("Page Down", "PgDn"), ("PageDown", "PgDn"), ("Escape", "Esc"), ("Caps Lock", "Caps"), ("Print Screen", "PrtSc"),
        ("Scroll Lock", "ScrLk"), ("Pause/Break", "Pause"), ("Num Lock", "Num"), ("Enter", "Enter"), ("Space", "Space"),
        ("Brightness Down", "Bri-"), ("Brightness Up", "Bri+"), ("Media Previous", "Prev"), ("Media Next", "Next"),
        ("Media Play/Pause", "Play"), ("Media Mute", "Mute"), ("Media Volume -", "Vol-"), ("Media Volume +", "Vol+"),
        ("Left Windows", "Win"), ("Right Windows", "Win"), ("Context Menu", "Menu"), ("Right Fn", "Fn"), ("Left Fn", "Fn"),
        ("Control", "Ctrl"), ("Windows", "Win"),
    ];
    if let Some((_, a)) = ABBREV.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)) {
        return a.to_string();
    }
    if let Some(x) = n.strip_prefix("Number Pad ") {
        return format!("N{}", short_label(x));
    }
    // "Left Shift" -> "Shift", "Right Control" -> "Ctrl", "Left Arrow" -> "Left".
    let n = n.strip_suffix(" Arrow").unwrap_or(n);
    let n = n.strip_prefix("Left ").or_else(|| n.strip_prefix("Right ")).map(short_label).unwrap_or_else(|| n.to_string());
    if n.chars().count() <= 5 { n } else { n.chars().take(5).collect() }
}

/// Paint individual LEDs: a grid for devices with a physical layout,
/// otherwise each zone as a row of LEDs. Left drag paints, right drag erases.
fn led_painter(ui: &mut egui::Ui, d: &DeviceInfo, entry: &mut DeviceLighting, preview: &[Rgb], brush: &mut Rgb) {
    ui.horizontal(|ui| {
        ui.label("Brush");
        ui.color_edit_button_srgb(brush);
        if ui.small_button("Fill all").clicked() {
            for i in 0..d.leds as u32 {
                entry.keys.insert(i, *brush);
            }
        }
        if ui.add_enabled(!entry.keys.is_empty(), egui::Button::new("Clear painted").small()).clicked() {
            entry.keys.clear();
        }
        ui.label(RichText::new(format!("{} painted · drag to paint, right-drag to erase", entry.keys.len())).size(11.5).color(MUTED));
    });
    // Lay out every zone: (rect, led index) pairs.
    let mut cells: Vec<(Rect, u32)> = vec![];
    let width = ui.available_width();
    let mut y = 0.0f32;
    let mut start = 0u32;
    let mut rows_meta: Vec<(f32, String)> = vec![];
    for z in &d.layout {
        let count = z.leds_count;
        if count == 0 {
            continue;
        }
        if d.layout.len() > 1 {
            rows_meta.push((y, z.name.clone()));
            y += 16.0;
        }
        match &z.matrix {
            Some((h, w, map)) if *w > 0 && *h > 0 => {
                let cell = (width / *w as f32).clamp(14.0, 44.0);
                for r in 0..*h {
                    for c in 0..*w {
                        let v = map[(r * w + c) as usize];
                        if v != NO_LED && v < count {
                            cells.push((Rect::from_min_size(pos2(c as f32 * cell, y + r as f32 * cell), vec2(cell - 3.0, cell - 3.0)), start + v));
                        }
                    }
                }
                y += *h as f32 * cell + 6.0;
            }
            _ => {
                let cell = 18.0f32;
                let per_row = ((width / cell) as u32).max(1);
                for i in 0..count {
                    cells.push((Rect::from_min_size(pos2((i % per_row) as f32 * cell, y + (i / per_row) as f32 * cell), vec2(cell - 4.0, cell - 4.0)), start + i));
                }
                y += count.div_ceil(per_row) as f32 * cell + 6.0;
            }
        }
        start += count;
    }
    let (resp, p) = ui.allocate_painter(vec2(width, y.max(10.0)), Sense::click_and_drag());
    let origin = resp.rect.min.to_vec2();
    for (yy, name) in rows_meta {
        p.text(pos2(resp.rect.left(), resp.rect.top() + yy), Align2::LEFT_TOP, name, FontId::proportional(11.0), MUTED);
    }
    let hover = resp.hover_pos();
    let mut hovered = None;
    for (r, i) in &cells {
        let r = r.translate(origin);
        let color = preview.get(*i as usize).copied().unwrap_or([0, 0, 0]);
        let painted = entry.keys.contains_key(i);
        p.rect_filled(r, CornerRadius::same(if r.width() > 20.0 { 5 } else { 8 }), rgb(color));
        p.rect_stroke(r, CornerRadius::same(5), Stroke::new(if painted { 1.5 } else { 1.0 }, if painted { TEXT } else { LINE }), egui::StrokeKind::Inside);
        if r.width() >= 24.0 {
            let label = short_label(d.led_names.get(*i as usize).map(String::as_str).unwrap_or(""));
            let lum = 0.299 * color[0] as f32 + 0.587 * color[1] as f32 + 0.114 * color[2] as f32;
            p.text(r.center(), Align2::CENTER_CENTER, label, FontId::proportional((r.width() * 0.3).min(12.0)), if lum > 140.0 { Color32::BLACK } else { TEXT });
        }
        if hover.is_some_and(|h| r.contains(h)) {
            hovered = Some(*i);
        }
    }
    if let Some(i) = hovered {
        let name = d.led_names.get(i as usize).cloned().filter(|n| !n.is_empty()).unwrap_or_else(|| format!("LED {}", i + 1));
        resp.clone().on_hover_text(name);
        let (primary, secondary) = ui.input(|inp| (inp.pointer.primary_down(), inp.pointer.secondary_down()));
        if resp.is_pointer_button_down_on() || resp.dragged() || resp.clicked() || resp.secondary_clicked() {
            if secondary || resp.secondary_clicked() {
                entry.keys.remove(&i);
            } else if primary || resp.clicked() {
                entry.keys.insert(i, *brush);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(short_label("Key: Escape"), "Esc");
        assert_eq!(short_label("Key: A"), "A");
        assert_eq!(short_label("Key: Left Shift"), "Shift");
        assert_eq!(short_label("Key: Right Control"), "Ctrl");
        assert_eq!(short_label("Key: Backspace"), "Bksp");
        assert_eq!(short_label("Key: Left Arrow"), "Left");
        assert_eq!(short_label("Key: Number Pad 7"), "N7");
        assert_eq!(short_label("Key: Media Volume -"), "Vol-");
        assert_eq!(short_label("Key: F12"), "F12");
        assert_eq!(short_label(""), "");
    }
}
