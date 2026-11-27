//! Lighting engine: renders effects in Stellar and streams them to every
//! OpenRGB device, so ASUS Aura, Corsair and other controllers stay in sync.

use crate::config::{Effect, LightingSettings, OnboardMode, Rgb, local_app_data};
use crate::openrgb::{
    COLOR_MODE_RANDOM, COLOR_MODE_SPECIFIC, Client, Device, FLAG_HAS_BRIGHTNESS, FLAG_HAS_DIRECTION_HV, FLAG_HAS_DIRECTION_LR,
    FLAG_HAS_DIRECTION_UD, FLAG_HAS_MODE_SPECIFIC_COLOR, FLAG_HAS_RANDOM_COLOR, FLAG_HAS_SPEED, Mode, Zone, pack,
};
use anyhow::{Result, anyhow, bail};
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------- effects

pub fn hsv(h: f64, s: f64, v: f64) -> Rgb {
    let h = h.rem_euclid(1.0) * 6.0;
    let i = h.floor() as i32;
    let f = h - i as f64;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    let (r, g, b) = match i {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [(r * 255.0).round() as u8, (g * 255.0).round() as u8, (b * 255.0).round() as u8]
}

pub fn lerp(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    std::array::from_fn(|i| (a[i] as f64 + (b[i] as f64 - a[i] as f64) * t).round() as u8)
}

fn scale(c: Rgb, k: f64) -> Rgb {
    c.map(|v| (v as f64 * k.clamp(0.0, 1.0)).round() as u8)
}

/// Colour of LED `i` of `n` at time `t` seconds. `temps` maps sensor ids to °C.
pub fn render(effect: &Effect, t: f64, i: usize, n: usize, temps: &HashMap<String, f64>) -> Rgb {
    let pos = if n > 1 { i as f64 / (n - 1) as f64 } else { 0.0 };
    match effect {
        Effect::Static { color } => *color,
        Effect::Breathing { color, period_s } => {
            let phase = 0.5 - 0.5 * (std::f64::consts::TAU * t / period_s).cos();
            scale(*color, 0.06 + 0.94 * phase * phase)
        }
        Effect::Spectrum { period_s } => hsv(t / period_s, 1.0, 1.0),
        Effect::Wave { period_s, spread } => hsv(t / period_s + pos * spread, 1.0, 1.0),
        Effect::Gradient { from, to } => lerp(*from, *to, pos),
        Effect::Temperature { sensor, cool, hot, low, high } => match temps.get(sensor) {
            Some(v) => lerp(*cool, *hot, (v - low) / (high - low)),
            // Unknown temperature is shown as a slow hot-colour pulse, never "cool".
            None => scale(*hot, 0.3 + 0.7 * (0.5 - 0.5 * (std::f64::consts::TAU * t / 2.0).cos())),
        },
        Effect::Off => [0, 0, 0],
    }
}

pub fn effective<'a>(s: &'a LightingSettings, device: &str, quiet: bool) -> &'a Effect {
    static OFF: Effect = Effect::Off;
    let d = s.devices.get(device);
    if !s.on || quiet || d.is_some_and(|d| !d.enabled) {
        return &OFF;
    }
    d.and_then(|d| d.effect.as_ref()).unwrap_or(&s.effect)
}

pub fn frame(s: &LightingSettings, device: &str, leds: usize, t: f64, temps: &HashMap<String, f64>, quiet: bool) -> Vec<Rgb> {
    let e = effective(s, device, quiet);
    let k = s.brightness / 100.0;
    let mut out: Vec<Rgb> = (0..leds).map(|i| scale(render(e, t, i, leds, temps), k)).collect();
    // Painted keys sit on top of the effect (but never light a device that is off).
    if *e != Effect::Off
        && let Some(d) = s.devices.get(device)
    {
        for (&i, &c) in &d.keys {
            if let Some(slot) = out.get_mut(i as usize) {
                *slot = scale(c, k);
            }
        }
    }
    out
}

/// The device's built-in mode configured by `ob`, with its index.
pub fn onboard_mode(d: &Device, ob: &OnboardMode) -> Option<(usize, Mode)> {
    let i = d.modes.iter().position(|m| m.name == ob.mode)?;
    let mut m = d.modes[i].clone();
    let fit = |v: u32, a: u32, b: u32| v.clamp(a.min(b), a.max(b));
    if m.flags & FLAG_HAS_SPEED != 0
        && let Some(v) = ob.speed
    {
        m.speed = fit(v, m.speed_min, m.speed_max);
    }
    if m.flags & FLAG_HAS_BRIGHTNESS != 0
        && let Some(v) = ob.brightness
    {
        m.brightness = fit(v, m.brightness_min, m.brightness_max);
    }
    if m.flags & (FLAG_HAS_DIRECTION_LR | FLAG_HAS_DIRECTION_UD | FLAG_HAS_DIRECTION_HV) != 0
        && let Some(v) = ob.direction
    {
        m.direction = v.min(5);
    }
    if ob.random && m.flags & FLAG_HAS_RANDOM_COLOR != 0 {
        m.color_mode = COLOR_MODE_RANDOM;
    } else if m.flags & FLAG_HAS_MODE_SPECIFIC_COLOR != 0 && !ob.colors.is_empty() {
        // Respect the device's colour count: pad by repeating, trim extras.
        let lo = m.colors_min.max(1) as usize;
        let hi = (m.colors_max as usize).max(lo);
        let n = ob.colors.len().clamp(lo, hi);
        m.color_mode = COLOR_MODE_SPECIFIC;
        m.colors = (0..n).map(|i| pack(ob.colors[i % ob.colors.len()])).collect();
    }
    Some((i, m))
}

// ---------------------------------------------------------------- ownership lease

/// Exclusive lock shared with the previous RGB apps so only one program
/// writes lighting at a time.
pub struct Lease {
    _files: Vec<File>,
}

impl Lease {
    pub fn acquire(paths: &[PathBuf]) -> Result<Self> {
        let mut files = vec![];
        let mut seen = HashSet::new();
        for p in paths {
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d)?;
            }
            let f = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(p)?;
            // Redirection can make two paths name one file; lock it once.
            let real = std::fs::canonicalize(p)?;
            if !seen.insert(real.to_string_lossy().to_lowercase()) {
                continue;
            }
            f.try_lock().map_err(|_| anyhow!("lighting lock held: {}", p.display()))?;
            files.push(f);
        }
        Ok(Self { _files: files })
    }

    pub fn legacy_paths() -> Vec<PathBuf> {
        let base = local_app_data();
        let mut v = vec![base.join("RGBTrayController").join("app.lock")];
        let packaged = base.join("Packages").join("OpenAI.Codex_2p2nqsd0c76g0").join("LocalCache").join("Local").join("RGBTrayController").join("app.lock");
        if packaged.exists() {
            v.push(packaged);
        }
        v
    }
}

// ---------------------------------------------------------------- engine

#[derive(Clone, Debug, Default)]
pub struct DeviceInfo {
    pub name: String,
    pub vendor: String,
    /// OpenRGB device type (5 = keyboard, 6 = mouse, 1 = RAM, …); see `kind_label`.
    pub kind: i32,
    pub leds: usize,
    pub zones: Vec<(String, u32, u32, u32)>, // name, count, min, max
    /// Zones with LED ranges and optional physical layout, for per-LED painting.
    pub layout: Vec<Zone>,
    pub led_names: Vec<String>,
    /// Built-in effects (excluding Direct/Off, which Stellar handles itself).
    pub modes: Vec<Mode>,
}

/// Settings key per device: its name, or "Name (2)", "Name (3)"… when several
/// devices share a name (common for identical RAM sticks or fans).
pub fn device_keys(names: &[&str]) -> Vec<String> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    names
        .iter()
        .map(|n| {
            let c = seen.entry(n).or_insert(0);
            *c += 1;
            if *c == 1 { n.to_string() } else { format!("{n} ({c})") }
        })
        .collect()
}

pub fn kind_label(kind: i32) -> &'static str {
    match kind {
        0 => "Motherboard",
        1 => "Memory",
        2 => "Graphics card",
        3 => "Cooler",
        4 => "LED strip",
        5 => "Keyboard",
        6 => "Mouse",
        7 => "Mousemat",
        8 => "Headset",
        9 => "Headset stand",
        10 => "Gamepad",
        11 => "Light",
        12 => "Speaker",
        14 => "Storage",
        15 => "Case",
        16 => "Microphone",
        17 => "Accessory",
        18 => "Keypad",
        _ => "Device",
    }
}

/// Modes Stellar shows as "built-in effects": everything except per-LED
/// direct control and off, which Stellar's own engine already provides.
pub fn is_builtin_effect(m: &Mode) -> bool {
    let n = m.name.to_lowercase();
    !(n == "direct" || n == "custom" || n == "static custom" || n == "off" || n == "static" && m.flags & crate::openrgb::FLAG_HAS_PER_LED_COLOR != 0)
}

#[derive(PartialEq, Clone)]
enum Applied {
    Unknown,
    Custom,
    OffMode,
    Onboard(Mode),
}

pub struct Lighting {
    client: Option<Client>,
    devices: Vec<Device>,
    applied: Vec<Applied>,
    last: Vec<Vec<Rgb>>,
    last_resend: Instant,
    lease: Option<Lease>,
    server: Option<Child>,
    last_server_attempt: Option<Instant>,
    retry_at: Instant,
    prepared_zones: Option<std::collections::BTreeMap<String, std::collections::BTreeMap<String, u32>>>,
    pub status: String,
    pub ok: bool,
}

impl Default for Lighting {
    fn default() -> Self {
        Self::new()
    }
}

impl Lighting {
    pub fn new() -> Self {
        Self {
            client: None,
            devices: vec![],
            applied: vec![],
            last: vec![],
            last_resend: Instant::now(),
            lease: None,
            server: None,
            last_server_attempt: None,
            retry_at: Instant::now(),
            prepared_zones: None,
            status: "Connecting to OpenRGB…".into(),
            ok: false,
        }
    }

    pub fn devices(&self) -> Vec<DeviceInfo> {
        self.devices
            .iter()
            .zip(self.keys())
            .map(|(d, key)| DeviceInfo {
                name: key,
                vendor: d.vendor.clone(),
                kind: d.kind,
                leds: d.led_count,
                zones: d.zones.iter().map(|z| (z.name.clone(), z.leds_count, z.leds_min, z.leds_max)).collect(),
                layout: d.zones.clone(),
                led_names: d.led_names.clone(),
                modes: d.modes.iter().filter(|m| is_builtin_effect(m)).cloned().collect(),
            })
            .collect()
    }

    fn keys(&self) -> Vec<String> {
        device_keys(&self.devices.iter().map(|d| d.name.as_str()).collect::<Vec<_>>())
    }

    /// Whether this engine holds a connection, lock or server.
    pub fn active(&self) -> bool {
        self.client.is_some() || self.lease.is_some() || self.server.is_some()
    }

    pub fn reconnect(&mut self) {
        self.client = None;
        self.retry_at = Instant::now();
    }

    fn start_server(&mut self) {
        if self.last_server_attempt.is_some_and(|t| t.elapsed() < Duration::from_secs(30)) {
            return;
        }
        self.last_server_attempt = Some(Instant::now());
        if self.server.as_mut().is_some_and(|c| c.try_wait().ok().flatten().is_none()) {
            return;
        }
        if crate::hardware::running_processes().iter().any(|p| p.starts_with("openrgb")) {
            return;
        }
        let pf = |v: &str| std::env::var_os(v).map(|p| Path::new(&p).join("OpenRGB").join("OpenRGB.exe"));
        let candidate = std::env::var_os("OPENRGB_PATH")
            .map(PathBuf::from)
            .into_iter()
            .chain(pf("ProgramFiles"))
            .chain(pf("ProgramFiles(x86)"))
            .find(|p| p.exists());
        let Some(path) = candidate else {
            self.status = "OpenRGB is not installed".into();
            return;
        };
        let mut cmd = Command::new(path);
        cmd.args(["--server", "--server-host", "127.0.0.1", "--startminimized"]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        match cmd.spawn() {
            Ok(c) => {
                self.server = Some(c);
                crate::log("Started OpenRGB local SDK server");
            }
            Err(e) => crate::log(&format!("Could not start OpenRGB: {e}")),
        }
    }

    fn prepare(&mut self, s: &LightingSettings) -> Result<()> {
        if self.lease.is_none() {
            self.lease = Some(Lease::acquire(&Lease::legacy_paths()).map_err(|_| {
                anyhow!("Another lighting app (Cooling & Lighting or the RGB tray) still owns lighting. Stellar takes over when it exits.")
            })?);
        }
        if self.client.is_none() {
            match Client::connect("127.0.0.1:6742", "Stellar") {
                Ok(c) => self.client = Some(c),
                Err(e) => {
                    self.start_server();
                    bail!("Waiting for OpenRGB ({e})");
                }
            }
            self.prepared_zones = None;
        }
        if self.prepared_zones.as_ref() == Some(&s.zone_sizes) {
            return Ok(());
        }
        let c = self.client.as_mut().expect("connected");
        let mut devices = c.devices()?;
        if devices.is_empty() {
            bail!("OpenRGB has not detected any devices yet");
        }
        for (i, d) in devices.iter_mut().enumerate() {
            let Some(sizes) = s.zone_sizes.get(&d.name) else { continue };
            let mut changed = false;
            for (zi, z) in d.zones.iter().enumerate() {
                if let Some(&size) = sizes.get(&z.name)
                    && size != z.leds_count
                {
                    if size < z.leds_min || size > z.leds_max {
                        bail!("Saved LED count for {} · {} is outside {}–{}", d.name, z.name, z.leds_min, z.leds_max);
                    }
                    c.resize_zone(i as u32, zi as u32, size)?;
                    changed = true;
                }
            }
            if changed {
                *d = c.device(i as u32)?;
            }
        }
        self.applied = vec![Applied::Unknown; devices.len()];
        self.last = vec![vec![]; devices.len()];
        self.devices = devices;
        self.prepared_zones = Some(s.zone_sizes.clone());
        Ok(())
    }

    /// Render and send one frame. Cheap when nothing changed.
    pub fn tick(&mut self, s: &LightingSettings, t: f64, temps: &HashMap<String, f64>, quiet: bool) {
        if self.client.is_none() && Instant::now() < self.retry_at {
            return;
        }
        if let Err(e) = self.tick_inner(s, t, temps, quiet) {
            self.client = None;
            self.prepared_zones = None;
            self.ok = false;
            self.retry_at = Instant::now() + Duration::from_secs(5);
            let msg = e.to_string();
            if msg != self.status {
                crate::log(&format!("Lighting: {msg}"));
            }
            self.status = msg;
        }
    }

    fn tick_inner(&mut self, s: &LightingSettings, t: f64, temps: &HashMap<String, f64>, quiet: bool) -> Result<()> {
        self.prepare(s)?;
        let resend = self.last_resend.elapsed() > Duration::from_secs(15);
        if resend {
            self.last_resend = Instant::now();
        }
        let keys = self.keys();
        let c = self.client.as_mut().expect("prepared");
        for (i, d) in self.devices.iter().enumerate() {
            let key = &keys[i];
            let e = effective(s, key, quiet);
            let off_mode = d.modes.iter().position(|m| m.name.eq_ignore_ascii_case("off"));
            if *e == Effect::Off
                && let Some(m) = off_mode
            {
                if self.applied[i] != Applied::OffMode || resend {
                    c.update_mode(i as u32, m as i32, &d.modes[m])?;
                    self.applied[i] = Applied::OffMode;
                    self.last[i].clear();
                }
                continue;
            }
            if let Some(ob) = s.devices.get(key).and_then(|x| x.onboard.as_ref())
                && let Some((mi, mode)) = onboard_mode(d, ob)
            {
                // The device renders its own effect; resend only on change.
                let want = Applied::Onboard(mode.clone());
                if self.applied[i] != want {
                    c.update_mode(i as u32, mi as i32, &mode)?;
                    self.applied[i] = want;
                    self.last[i].clear();
                }
                continue;
            }
            if self.applied[i] != Applied::Custom {
                c.set_custom_mode(i as u32)?;
                self.applied[i] = Applied::Custom;
                self.last[i].clear();
            }
            if d.led_count == 0 {
                continue;
            }
            let colors = frame(s, key, d.led_count, t, temps, quiet);
            if colors != self.last[i] || resend {
                c.update_leds(i as u32, &colors)?;
                self.last[i] = colors;
            }
        }
        if !self.ok {
            let n: usize = self.devices.iter().map(|d| d.led_count).sum();
            self.status = format!("{} devices · {} LEDs", self.devices.len(), n);
            self.ok = true;
            crate::log(&format!("Lighting connected: {}", self.status));
        }
        Ok(())
    }

    /// Store the device's configured built-in effect in its own memory.
    pub fn save_onboard(&mut self, s: &LightingSettings, device: &str) -> Result<()> {
        let ob = s.devices.get(device).and_then(|d| d.onboard.as_ref()).ok_or_else(|| anyhow!("Choose a built-in effect first"))?;
        let i = self.keys().iter().position(|k| k == device).ok_or_else(|| anyhow!("{device} is not connected"))?;
        let c = self.client.as_mut().ok_or_else(|| anyhow!("Lighting isn't connected"))?;
        let d = &self.devices[i];
        let (mi, mode) = onboard_mode(d, ob).ok_or_else(|| anyhow!("{device} has no mode called {}", ob.mode))?;
        if mode.flags & crate::openrgb::FLAG_MANUAL_SAVE == 0 {
            bail!("{device} can't store effects in its own memory");
        }
        c.update_mode(i as u32, mi as i32, &mode)?;
        c.save_mode(i as u32, mi as i32, &mode)?;
        self.applied[i] = Applied::Onboard(mode);
        crate::log(&format!("Saved {} to {device}", ob.mode));
        Ok(())
    }

    pub fn shutdown(&mut self) {
        self.client = None;
        if let Some(mut c) = self.server.take() {
            // Only stop a server this instance started, never a shared one.
            let _ = c.kill();
        }
        self.lease = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DeviceLighting, QuietHours};

    #[test]
    fn effects() {
        let none = HashMap::new();
        assert_eq!(render(&Effect::Static { color: [1, 2, 3] }, 9.0, 0, 1, &none), [1, 2, 3]);
        assert_eq!(hsv(0.0, 1.0, 1.0), [255, 0, 0]);
        assert_eq!(hsv(1.0 / 3.0, 1.0, 1.0), [0, 255, 0]);
        let g = Effect::Gradient { from: [0, 0, 0], to: [200, 100, 0] };
        assert_eq!(render(&g, 0.0, 0, 3, &none), [0, 0, 0]);
        assert_eq!(render(&g, 0.0, 2, 3, &none), [200, 100, 0]);
        let b = Effect::Breathing { color: [200, 200, 200], period_s: 4.0 };
        assert!(render(&b, 0.0, 0, 1, &none)[0] < 20 && render(&b, 2.0, 0, 1, &none)[0] == 200);
        let te = Effect::Temperature { sensor: "cpu".into(), cool: [0, 0, 255], hot: [255, 0, 0], low: 40.0, high: 80.0 };
        let temps: HashMap<String, f64> = [("cpu".to_string(), 60.0)].into();
        assert_eq!(render(&te, 0.0, 0, 1, &temps), [128, 0, 128]);
        assert_eq!(render(&te, 1.0, 0, 1, &none)[2], 0, "unknown temperature never shows cool");
    }

    #[test]
    fn device_overrides_quiet_and_brightness() {
        let mut s = LightingSettings { brightness: 50.0, effect: Effect::Static { color: [200, 0, 0] }, ..Default::default() };
        s.devices.insert("B".into(), DeviceLighting { effect: Some(Effect::Static { color: [0, 200, 0] }), ..Default::default() });
        s.devices.insert("C".into(), DeviceLighting { enabled: false, ..Default::default() });
        let n = HashMap::new();
        assert_eq!(frame(&s, "A", 2, 0.0, &n, false), vec![[100, 0, 0]; 2]);
        assert_eq!(frame(&s, "B", 1, 0.0, &n, false), vec![[0, 100, 0]]);
        assert_eq!(*effective(&s, "C", false), Effect::Off);
        assert_eq!(*effective(&s, "A", true), Effect::Off, "quiet hours");
        s.on = false;
        assert_eq!(*effective(&s, "B", false), Effect::Off);
        let _ = QuietHours { start_min: 0, end_min: 0 };
    }

    #[test]
    fn duplicate_device_names_get_their_own_keys() {
        assert_eq!(device_keys(&["RAM", "GPU", "RAM", "RAM"]), vec!["RAM", "GPU", "RAM (2)", "RAM (3)"]);
    }

    #[test]
    fn painted_keys_overlay_the_effect() {
        let mut s = LightingSettings { brightness: 100.0, effect: Effect::Static { color: [0, 0, 255] }, ..Default::default() };
        let mut d = DeviceLighting::default();
        d.keys.insert(1, [255, 0, 0]);
        d.keys.insert(99, [255, 0, 0]); // beyond the device: ignored
        s.devices.insert("KB".into(), d);
        let n = HashMap::new();
        assert_eq!(frame(&s, "KB", 3, 0.0, &n, false), vec![[0, 0, 255], [255, 0, 0], [0, 0, 255]]);
        assert_eq!(frame(&s, "KB", 3, 0.0, &n, true), vec![[0, 0, 0]; 3], "quiet hours turn painted keys off too");
    }

    #[test]
    fn onboard_modes_respect_device_limits() {
        use crate::openrgb::*;
        let mode = Mode {
            name: "Breathing".into(), value: 2, flags: FLAG_HAS_SPEED | FLAG_HAS_MODE_SPECIFIC_COLOR | FLAG_HAS_DIRECTION_LR,
            speed_min: 255, speed_max: 0, brightness_min: 0, brightness_max: 0, colors_min: 1, colors_max: 2,
            speed: 127, brightness: 0, direction: 0, color_mode: 0, colors: vec![],
        };
        let dev = Device { kind: 5, name: "KB".into(), vendor: String::new(), description: String::new(), modes: vec![mode], active_mode: 0, zones: vec![], led_count: 0, led_names: vec![] };
        let ob = OnboardMode { mode: "Breathing".into(), speed: Some(999), brightness: Some(50), direction: Some(1), colors: vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]], random: false };
        let (i, m) = onboard_mode(&dev, &ob).unwrap();
        assert_eq!(i, 0);
        assert_eq!(m.speed, 255, "reversed speed range is clamped");
        assert_eq!(m.brightness, 0, "brightness ignored without the flag");
        assert_eq!(m.direction, 1);
        assert_eq!(m.colors, vec![pack([255, 0, 0]), pack([0, 255, 0])], "trimmed to the device's maximum colours");
        assert_eq!(m.color_mode, COLOR_MODE_SPECIFIC);
        assert!(onboard_mode(&dev, &OnboardMode { mode: "Nope".into(), ..Default::default() }).is_none());
    }

    #[test]
    fn lease_excludes_and_dedupes() {
        let d = std::env::temp_dir().join(format!("stellar-lease-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let a = d.join("a.lock");
        let b = d.join("b.lock");
        {
            let _l = Lease::acquire(&[a.clone(), d.join(".").join("a.lock")]).expect("same file locked once");
        }
        let held = Lease::acquire(std::slice::from_ref(&b)).unwrap();
        assert!(Lease::acquire(&[a.clone(), b.clone()]).is_err(), "competing writer blocked");
        let _a = Lease::acquire(std::slice::from_ref(&a)).expect("failed acquisition released earlier locks");
        drop(held);
        drop(_a);
        Lease::acquire(&[a, b]).expect("released on drop");
        let _ = std::fs::remove_dir_all(d);
    }
}
