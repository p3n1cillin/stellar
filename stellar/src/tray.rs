//! System-tray icon and menu.

use std::sync::mpsc::Sender;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

#[derive(Debug, Clone)]
pub enum Action {
    Show,
    ToggleLighting,
    Profile(String),
    Quit,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Normal,
    Alert,
    Dim,
}

/// A four-point star, Stellar's mark, rendered at any size.
pub fn star_rgba(size: u32, tone: Tone) -> Vec<u8> {
    let (core, glow): ([f32; 3], [f32; 3]) = match tone {
        Tone::Normal => ([1.0, 0.97, 0.88], [0.55, 0.42, 1.0]),
        Tone::Alert => ([1.0, 0.9, 0.85], [1.0, 0.25, 0.2]),
        Tone::Dim => ([0.8, 0.8, 0.85], [0.35, 0.35, 0.45]),
    };
    let mut out = vec![0u8; (size * size * 4) as usize];
    let c = (size as f32 - 1.0) / 2.0;
    for y in 0..size {
        for x in 0..size {
            let dx = (x as f32 - c).abs() / c;
            let dy = (y as f32 - c).abs() / c;
            // Astroid-like star: |x|^0.5 + |y|^0.5 <= 1
            let d = dx.sqrt() + dy.sqrt();
            let star = (1.0 - d).clamp(0.0, 0.12) / 0.12;
            let r = (dx * dx + dy * dy).sqrt();
            let halo = ((1.0 - r) * 0.55).clamp(0.0, 1.0).powf(1.6);
            let a = star.max(halo);
            if a <= 0.0 {
                continue;
            }
            let t = star / a.max(1e-3);
            let i = ((y * size + x) * 4) as usize;
            for k in 0..3 {
                out[i + k] = ((core[k] * t + glow[k] * (1.0 - t)) * 255.0) as u8;
            }
            out[i + 3] = (a * 255.0) as u8;
        }
    }
    out
}

pub fn icon(tone: Tone) -> Icon {
    Icon::from_rgba(star_rgba(32, tone), 32, 32).expect("valid icon")
}

pub struct Tray {
    pub icon: TrayIcon,
    lighting: CheckMenuItem,
    profiles: Submenu,
    profile_items: Vec<CheckMenuItem>,
    profile_names: Vec<String>,
    tone: Tone,
    open: MenuId,
    quit: MenuId,
}

impl Tray {
    pub fn new(tx: Sender<Action>, wake: impl Fn() + Send + Sync + 'static, show: impl Fn() + Send + Sync + 'static) -> anyhow::Result<Self> {
        let menu = Menu::new();
        let open = MenuItem::new("Open Stellar", true, None);
        let lighting = CheckMenuItem::new("Lighting", true, true, None);
        let profiles = Submenu::new("Profiles", true);
        let quit = MenuItem::new("Quit Stellar", true, None);
        menu.append(&open)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&lighting)?;
        menu.append(&profiles)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&quit)?;
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip("Stellar")
            .with_icon(icon(Tone::Normal))
            .build()?;

        let wake = std::sync::Arc::new(wake);
        let show = std::sync::Arc::new(show);
        {
            let (tx, wake, show) = (tx.clone(), wake.clone(), show.clone());
            TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
                let open = matches!(e, TrayIconEvent::DoubleClick { button: MouseButton::Left, .. })
                    || matches!(e, TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. });
                if open {
                    show();
                    let _ = tx.send(Action::Show);
                    wake();
                }
            }));
        }
        {
            let (open_id, quit_id, light_id) = (open.id().clone(), quit.id().clone(), lighting.id().clone());
            MenuEvent::set_event_handler(Some(std::sync::Arc::new(move |e: MenuEvent| {
                let action = if e.id == open_id {
                    show();
                    Action::Show
                } else if e.id == quit_id {
                    Action::Quit
                } else if e.id == light_id {
                    Action::ToggleLighting
                } else {
                    Action::Profile(e.id.0.clone())
                };
                let _ = tx.send(action);
                wake();
            })));
        }
        Ok(Self {
            icon,
            open: open.id().clone(),
            quit: quit.id().clone(),
            lighting,
            profiles,
            profile_items: vec![],
            profile_names: vec![],
            tone: Tone::Normal,
        })
    }

    pub fn sync(&mut self, lighting_on: bool, profiles: &[String], active: Option<&str>, tone: Tone, tooltip: &str) {
        if self.lighting.is_checked() != lighting_on {
            self.lighting.set_checked(lighting_on);
        }
        if profiles != self.profile_names.as_slice() {
            for item in self.profile_items.drain(..) {
                let _ = self.profiles.remove(&item);
            }
            for name in profiles {
                let id = MenuId::new(format!("profile:{name}"));
                let item = CheckMenuItem::with_id(id, name, true, active == Some(name.as_str()), None);
                let _ = self.profiles.append(&item);
                self.profile_items.push(item);
            }
            self.profile_names = profiles.to_vec();
        }
        for c in &self.profile_items {
            let want = active.is_some_and(|a| c.id().0.strip_prefix("profile:") == Some(a));
            if c.is_checked() != want {
                c.set_checked(want);
            }
        }
        if tone != self.tone {
            let _ = self.icon.set_icon(Some(icon(tone)));
            self.tone = tone;
        }
        let _ = self.icon.set_tooltip(Some(tooltip));
        let _ = (&self.open, &self.quit);
    }
}

pub fn profile_from_action(id: &str) -> Option<&str> {
    id.strip_prefix("profile:")
}
