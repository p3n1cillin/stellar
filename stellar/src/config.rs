//! Settings model, validation, atomic persistence, named profiles and
//! migration from the previous Cooling & Lighting app.

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub use crate::curves::{CurveDef, CurveKind, curve_sensors, validate_curves};

pub const VERSION: u32 = 2;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FanMode {
    #[default]
    Firmware,
    Curve,
    Manual,
}

/// The CPU cooler type changes how auto-tune shapes the CPU curve: an AIO's
/// coolant lags the CPU, so its radiator fans follow a smoothed trend.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CpuCooler {
    #[default]
    Air,
    Aio,
}

/// How several temperature sources combine into one control temperature.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mix {
    #[default]
    Max,
    Average,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct CurvePoint {
    pub temp: f64,
    pub percent: f64,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct CalibrationPoint {
    pub percent: f64,
    pub rpm: f64,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct FanRule {
    pub control_id: String,
    pub name: String,
    pub rpm_sensor_id: Option<String>,
    pub calibration: Vec<CalibrationPoint>,
    /// Restart speed measured by auto-tune, kept so curves can be rebuilt.
    pub measured_start: Option<f64>,
    pub mode: FanMode,
    /// Curve followed in Curve mode.
    pub curve: Option<String>,
    pub manual_percent: f64,
    /// Lowest speed while running ("stop %" in Fan Control terms).
    pub minimum_percent: f64,
    /// Speed used to start the fan from standstill.
    pub start_percent: f64,
    /// Let the fan stop when the curve asks for less than the minimum.
    pub allow_stop: bool,
    /// Added to the curve's output.
    pub offset_percent: f64,
    pub step_up_per_sec: f64,
    pub ramp_down_per_sec: f64,
    /// Extra sensors watched by the thermal failsafe, on top of the curve's.
    pub sensors: Vec<String>,
    pub critical_temp: f64,
    /// °C the hottest watched sensor must fall before the fan slows down.
    pub hysteresis: f64,
}

impl Default for FanRule {
    fn default() -> Self {
        Self {
            control_id: String::new(),
            name: String::new(),
            rpm_sensor_id: None,
            calibration: vec![],
            measured_start: None,
            mode: FanMode::Firmware,
            curve: None,
            manual_percent: 50.0,
            minimum_percent: 30.0,
            start_percent: 40.0,
            allow_stop: false,
            offset_percent: 0.0,
            step_up_per_sec: 25.0,
            ramp_down_per_sec: 5.0,
            sensors: vec![],
            critical_temp: 90.0,
            hysteresis: 2.0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CurvePreset {
    Silent,
    Balanced,
    Performance,
}

pub fn preset(p: CurvePreset) -> Vec<CurvePoint> {
    let pts: &[(f64, f64)] = match p {
        CurvePreset::Silent => &[(40.0, 30.0), (60.0, 40.0), (75.0, 65.0), (85.0, 100.0)],
        CurvePreset::Balanced => &[(35.0, 40.0), (55.0, 55.0), (70.0, 80.0), (85.0, 100.0)],
        CurvePreset::Performance => &[(30.0, 50.0), (50.0, 70.0), (65.0, 90.0), (75.0, 100.0)],
    };
    pts.iter().map(|&(temp, percent)| CurvePoint { temp, percent }).collect()
}

fn finite_in(v: f64, lo: f64, hi: f64) -> bool {
    v.is_finite() && v >= lo && v <= hi
}

impl FanRule {
    pub fn validate(&self) -> Result<()> {
        if self.control_id.trim().is_empty() {
            bail!("Select a fan output");
        }
        if self.sensors.iter().any(|s| s.trim().is_empty()) {
            bail!("Invalid sensor data");
        }
        if self.calibration.iter().any(|p| !finite_in(p.percent, 0.0, 100.0) || !finite_in(p.rpm, 0.0, 30000.0)) {
            bail!("Invalid calibration data");
        }
        for v in [self.manual_percent, self.minimum_percent, self.start_percent] {
            if !finite_in(v, 0.0, 100.0) {
                bail!("Fan speeds must be 0–100%");
            }
        }
        if !finite_in(self.offset_percent, -50.0, 50.0) {
            bail!("Speed offset must be within ±50%");
        }
        if !finite_in(self.step_up_per_sec, 1.0, 100.0) {
            bail!("Step-up rate must be 1–100%/s");
        }
        if !finite_in(self.critical_temp, 50.0, 100.0) {
            bail!("Critical temperature must be 50–100°C");
        }
        if !finite_in(self.hysteresis, 0.0, 10.0) || !self.ramp_down_per_sec.is_finite()
            || self.ramp_down_per_sec <= 0.0 || self.ramp_down_per_sec > 100.0
        {
            bail!("Invalid smoothing settings");
        }
        Ok(())
    }

    /// Sensors the thermal failsafe watches: the curve's plus any extras.
    pub fn watched(&self, curves: &[CurveDef]) -> Vec<String> {
        let mut v = self.sensors.clone();
        if self.mode == FanMode::Curve
            && let Some(c) = &self.curve
        {
            v.extend(curve_sensors(curves, c));
        }
        v.sort();
        v.dedup();
        v
    }

    /// Full validation, including the curve this fan follows.
    pub fn validate_with(&self, curves: &[CurveDef]) -> Result<()> {
        self.validate()?;
        if self.mode == FanMode::Curve {
            let Some(id) = &self.curve else { bail!("Choose a curve") };
            if !curves.iter().any(|c| &c.id == id) {
                bail!("The selected curve no longer exists");
            }
        }
        if self.mode != FanMode::Firmware && self.watched(curves).is_empty() {
            bail!("A temperature sensor is required for the thermal failsafe");
        }
        Ok(())
    }

    /// Estimated RPM for a duty cycle from the calibration table.
    pub fn expected_rpm(&self, percent: f64) -> Option<f64> {
        let mut c = self.calibration.clone();
        if c.len() < 2 {
            return None;
        }
        c.sort_by(|a, b| a.percent.total_cmp(&b.percent));
        if percent <= c[0].percent {
            return Some(c[0].rpm);
        }
        for w in c.windows(2) {
            if percent <= w[1].percent {
                let span = (w[1].percent - w[0].percent).max(1e-9);
                return Some(w[0].rpm + (w[1].rpm - w[0].rpm) * (percent - w[0].percent) / span);
            }
        }
        c.last().map(|p| p.rpm)
    }
}

pub type Rgb = [u8; 3];

/// A lighting effect. Every effect is rendered by Stellar itself and streamed
/// to OpenRGB, so it looks identical across ASUS, Corsair and other devices.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "type")]
pub enum Effect {
    Static { color: Rgb },
    Breathing { color: Rgb, period_s: f64 },
    Spectrum { period_s: f64 },
    Wave { period_s: f64, spread: f64 },
    Gradient { from: Rgb, to: Rgb },
    /// Colour follows a temperature sensor: cool colour at `low`, hot at `high`.
    Temperature { sensor: String, cool: Rgb, hot: Rgb, low: f64, high: f64 },
    Off,
}

impl Default for Effect {
    fn default() -> Self {
        Effect::Static { color: [255, 255, 255] }
    }
}

impl Effect {
    pub fn label(&self) -> &'static str {
        match self {
            Effect::Static { .. } => "Static",
            Effect::Breathing { .. } => "Breathing",
            Effect::Spectrum { .. } => "Spectrum cycle",
            Effect::Wave { .. } => "Rainbow wave",
            Effect::Gradient { .. } => "Gradient",
            Effect::Temperature { .. } => "Temperature reactive",
            Effect::Off => "Off",
        }
    }
    pub fn animated(&self) -> bool {
        matches!(self, Effect::Breathing { .. } | Effect::Spectrum { .. } | Effect::Wave { .. } | Effect::Temperature { .. })
    }
    fn validate(&self) -> Result<()> {
        let period_ok = |p: f64| finite_in(p, 0.5, 120.0);
        match self {
            Effect::Breathing { period_s, .. } | Effect::Spectrum { period_s } if !period_ok(*period_s) => {
                bail!("Effect speed must be 0.5–120 s")
            }
            Effect::Wave { period_s, spread } if !period_ok(*period_s) || !finite_in(*spread, 0.05, 10.0) => {
                bail!("Invalid wave settings")
            }
            Effect::Temperature { sensor, low, high, .. }
                if sensor.trim().is_empty() || !finite_in(*low, 0.0, 110.0) || !finite_in(*high, 0.0, 110.0) || high <= low =>
            {
                bail!("Temperature lighting needs a sensor and low < high")
            }
            _ => Ok(()),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct DeviceLighting {
    pub enabled: bool,
    /// None follows the global effect.
    pub effect: Option<Effect>,
}

impl Default for DeviceLighting {
    fn default() -> Self {
        Self { enabled: true, effect: None }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct QuietHours {
    /// Minutes after midnight, local time. A window may wrap past midnight.
    pub start_min: u32,
    pub end_min: u32,
}

impl QuietHours {
    pub fn contains(&self, minute: u32) -> bool {
        if self.start_min == self.end_min {
            false
        } else if self.start_min < self.end_min {
            minute >= self.start_min && minute < self.end_min
        } else {
            minute >= self.start_min || minute < self.end_min
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct LightingSettings {
    pub on: bool,
    pub brightness: f64,
    pub effect: Effect,
    pub devices: BTreeMap<String, DeviceLighting>,
    pub zone_sizes: BTreeMap<String, BTreeMap<String, u32>>,
    pub quiet_hours: Option<QuietHours>,
}

impl Default for LightingSettings {
    fn default() -> Self {
        Self {
            on: true,
            brightness: 100.0,
            effect: Effect::default(),
            devices: BTreeMap::new(),
            zone_sizes: BTreeMap::new(),
            quiet_hours: None,
        }
    }
}

impl LightingSettings {
    pub fn validate(&self) -> Result<()> {
        if !finite_in(self.brightness, 0.0, 100.0) {
            bail!("Brightness must be 0–100%");
        }
        self.effect.validate()?;
        for d in self.devices.values() {
            if let Some(e) = &d.effect {
                e.validate()?;
            }
        }
        if let Some(q) = self.quiet_hours
            && (q.start_min >= 1440 || q.end_min >= 1440)
        {
            bail!("Invalid quiet hours");
        }
        if self.zone_sizes.values().flat_map(|z| z.values()).any(|&v| v > 10000) {
            bail!("Invalid LED counts");
        }
        Ok(())
    }
}

/// Switch to a profile while a named program runs (e.g. a game).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct AutoRule {
    pub process: String,
    pub profile: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub curves: Vec<CurveDef>,
    pub fans: Vec<FanRule>,
    pub lighting: LightingSettings,
    pub active_profile: Option<String>,
    pub automation: Vec<AutoRule>,
    /// Profile restored when no automation rule matches; None leaves the
    /// current configuration alone.
    pub fallback_profile: Option<String>,
    pub notifications: bool,
    pub cpu_cooler: CpuCooler,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: VERSION,
            curves: vec![],
            fans: vec![],
            lighting: LightingSettings::default(),
            active_profile: None,
            automation: vec![],
            fallback_profile: None,
            notifications: true,
            cpu_cooler: CpuCooler::Air,
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Result<()> {
        if self.version != VERSION {
            bail!("Unsupported settings version {}", self.version);
        }
        validate_curves(&self.curves)?;
        let mut ids = std::collections::HashSet::new();
        for r in &self.fans {
            r.validate_with(&self.curves).with_context(|| format!("Fan '{}'", r.name))?;
            if !ids.insert(&r.control_id) {
                bail!("Duplicate fan rules");
            }
        }
        self.lighting.validate()?;
        for p in self.active_profile.iter().chain(self.fallback_profile.iter()) {
            validate_profile_name(p)?;
        }
        for a in &self.automation {
            if a.process.trim().is_empty() {
                bail!("Automation rules need a program name");
            }
            validate_profile_name(&a.profile)?;
        }
        Ok(())
    }
    pub fn rule(&self, control_id: &str) -> Option<&FanRule> {
        self.fans.iter().find(|r| r.control_id == control_id)
    }
}

/// The part of the settings a named profile captures.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct Profile {
    pub curves: Vec<CurveDef>,
    pub fans: Vec<FanRule>,
    pub lighting: LightingSettings,
}

pub fn validate_profile_name(name: &str) -> Result<()> {
    if name.trim().is_empty() || name.chars().count() > 60 || name != name.trim()
        || !name.chars().all(|c| c.is_alphanumeric() || c == ' ' || c == '-' || c == '_')
    {
        bail!("Use a profile name of 1–60 letters, numbers, spaces, hyphens or underscores");
    }
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1",
        "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(name)) {
        bail!("Reserved profile name");
    }
    Ok(())
}

// ---------------------------------------------------------------- storage

pub struct Store {
    pub dir: PathBuf,
    pub warning: Option<String>,
}

pub fn local_app_data() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

impl Store {
    /// `%LOCALAPPDATA%\Programs\Stellar\Data`. Programs is excluded from
    /// packaged-host AppData redirection, so elevated scheduled launches and
    /// interactive launches see the same files.
    pub fn default_location() -> Self {
        Self::at(local_app_data().join("Programs").join("Stellar").join("Data"))
    }
    pub fn at(dir: PathBuf) -> Self {
        Self { dir, warning: None }
    }
    pub fn settings_path(&self) -> PathBuf {
        self.dir.join("settings.json")
    }
    pub fn profiles_dir(&self) -> PathBuf {
        self.dir.join("profiles")
    }

    pub fn load(&mut self) -> Settings {
        let legacy = local_app_data().join("Programs").join("CoolingLighting").join("Data");
        self.load_with_legacy(&legacy)
    }

    pub fn load_with_legacy(&mut self, legacy_dir: &Path) -> Settings {
        self.warning = None;
        let path = self.settings_path();
        for (i, candidate) in [path.clone(), bak(&path)].iter().enumerate() {
            if let Ok(s) = read_json::<Settings>(candidate).and_then(|s| s.validate().map(|_| s)) {
                if i == 1 {
                    self.warning = Some("Recovered settings from the backup copy".into());
                }
                return s;
            }
        }
        if path.exists() || bak(&path).exists() {
            self.warning =
                Some("Settings could not be read. Cooling stays in firmware mode; damaged files were left untouched.".into());
            return Settings::default();
        }
        match migrate_legacy(legacy_dir) {
            Ok(Some((settings, profiles))) => {
                for (name, p) in profiles {
                    let _ = self.save_profile(&name, &p);
                }
                if let Err(e) = self.save(&settings) {
                    self.warning = Some(format!("Imported previous settings but could not save them: {e}"));
                } else {
                    self.warning = Some("Imported settings from Cooling & Lighting".into());
                }
                settings
            }
            Ok(None) => Settings::default(),
            Err(e) => {
                self.warning = Some(format!("Previous Cooling & Lighting settings could not be imported: {e}"));
                Settings::default()
            }
        }
    }

    pub fn save(&mut self, settings: &Settings) -> Result<()> {
        settings.validate()?;
        save_json(&self.settings_path(), settings, &mut self.warning)
    }

    pub fn profile_names(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(self.profiles_dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if p.extension()? != "json" {
                    return None;
                }
                Some(p.file_stem()?.to_string_lossy().into_owned())
            })
            .filter(|n| validate_profile_name(n).is_ok())
            .collect();
        v.sort_by_key(|s| s.to_lowercase());
        v
    }

    pub fn save_profile(&mut self, name: &str, profile: &Profile) -> Result<()> {
        validate_profile_name(name)?;
        let probe = Settings { curves: profile.curves.clone(), fans: profile.fans.clone(), lighting: profile.lighting.clone(), ..Settings::default() };
        probe.validate()?;
        save_json(&self.profiles_dir().join(format!("{name}.json")), profile, &mut self.warning)
    }

    pub fn load_profile(&self, name: &str) -> Result<Profile> {
        validate_profile_name(name)?;
        let path = self.profiles_dir().join(format!("{name}.json"));
        for c in [path.clone(), bak(&path)] {
            if let Ok(p) = read_json::<Profile>(&c) {
                let probe = Settings { curves: p.curves.clone(), fans: p.fans.clone(), lighting: p.lighting.clone(), ..Settings::default() };
                if probe.validate().is_ok() {
                    return Ok(p);
                }
            }
        }
        Err(anyhow!("Profile '{name}' cannot be read; the current settings have not changed"))
    }

    pub fn delete_profile(&self, name: &str) -> Result<()> {
        validate_profile_name(name)?;
        let path = self.profiles_dir().join(format!("{name}.json"));
        fs::remove_file(&path)?;
        let _ = fs::remove_file(bak(&path));
        Ok(())
    }
}

fn bak(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".bak");
    PathBuf::from(s)
}

fn read_json<T: for<'de> Deserialize<'de>>(p: &Path) -> Result<T> {
    Ok(serde_json::from_slice(&fs::read(p)?)?)
}

fn save_json<T: Serialize>(path: &Path, value: &T, warning: &mut Option<String>) -> Result<()> {
    if let Some(d) = path.parent() {
        fs::create_dir_all(d)?;
    }
    let payload = serde_json::to_vec_pretty(value)?;
    atomic_write(path, &payload)?;
    // The primary commit succeeded; a backup failure must not report a failed save.
    if let Err(e) = atomic_write(&bak(path), &payload) {
        *warning = Some(format!("Settings saved, but the recovery copy could not be updated: {e}"));
    }
    Ok(())
}

pub fn atomic_write(path: &Path, payload: &[u8]) -> Result<()> {
    let tmp = {
        let mut s = path.as_os_str().to_owned();
        s.push(format!(".{}.tmp", std::process::id() as u64 ^ nanos()));
        PathBuf::from(s)
    };
    let result = (|| -> Result<()> {
        let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(payload)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)?; // MoveFileEx(REPLACE_EXISTING) on Windows
        Ok(())
    })();
    if tmp.exists() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn nanos() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0)
}

// ---------------------------------------------------------------- migration

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct V1Settings {
    version: u32,
    lighting_on: bool,
    active_profile: Option<String>,
    color: String, // System.Text.Json writes byte[] as base64
    #[serde(default)]
    zone_sizes: BTreeMap<String, BTreeMap<String, u32>>,
    #[serde(default)]
    fans: Vec<V1Rule>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct V1Rule {
    control_id: String,
    name: String,
    rpm_sensor_id: Option<String>,
    #[serde(default)]
    calibration: Vec<V1Cal>,
    #[serde(default)]
    temperature_sensor_ids: Vec<String>,
    mode: String,
    manual_percent: f64,
    minimum_percent: f64,
    start_percent: f64,
    critical_temperature: f64,
    hysteresis: f64,
    ramp_down_per_second: f64,
    points: Vec<V1Point>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct V1Cal {
    percent: f64,
    rpm: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct V1Point {
    temperature: f64,
    percent: f64,
}

fn convert_v1(v: V1Settings) -> Result<Settings> {
    if v.version != 1 {
        bail!("unknown version");
    }
    let color = base64_decode(&v.color).filter(|c| c.len() == 3).ok_or_else(|| anyhow!("invalid colour"))?;
    let mut curves = vec![];
    let mut fans = vec![];
    for (i, r) in v.fans.into_iter().enumerate() {
        let mode = match r.mode.as_str() {
            "Curve" => FanMode::Curve,
            "Manual" => FanMode::Manual,
            "Firmware" => FanMode::Firmware,
            m => bail!("unknown fan mode {m}"),
        };
        // Each old per-fan curve becomes a named, reusable curve.
        let curve_id = format!("imported{i}");
        if mode == FanMode::Curve {
            curves.push(CurveDef { response_s: 0.0,
                id: curve_id.clone(),
                name: format!("{} curve", r.name),
                kind: CurveKind::Graph {
                    sensors: r.temperature_sensor_ids.clone(),
                    mix: Mix::Max,
                    offset: 0.0,
                    points: r.points.iter().map(|p| CurvePoint { temp: p.temperature, percent: p.percent }).collect(),
                },
            });
        }
        fans.push(FanRule {
            control_id: r.control_id,
            name: r.name,
            rpm_sensor_id: r.rpm_sensor_id.filter(|s| !s.trim().is_empty()),
            calibration: r.calibration.into_iter().map(|c| CalibrationPoint { percent: c.percent, rpm: c.rpm }).collect(),
            mode,
            curve: (mode == FanMode::Curve).then_some(curve_id),
            sensors: r.temperature_sensor_ids,
            manual_percent: r.manual_percent,
            minimum_percent: r.minimum_percent,
            start_percent: r.start_percent,
            critical_temp: r.critical_temperature,
            hysteresis: r.hysteresis,
            ramp_down_per_sec: r.ramp_down_per_second,
            step_up_per_sec: 100.0, // the previous app applied increases immediately
            ..FanRule::default()
        });
    }

    let s = Settings {
        curves,
        fans,
        lighting: LightingSettings {
            on: v.lighting_on,
            effect: Effect::Static { color: [color[0], color[1], color[2]] },
            zone_sizes: v.zone_sizes,
            ..LightingSettings::default()
        },
        active_profile: v.active_profile,
        ..Settings::default()
    };
    s.validate()?;
    Ok(s)
}

/// Read-only import of the previous app's settings and profiles. Its files
/// are never modified.
pub fn migrate_legacy(dir: &Path) -> Result<Option<(Settings, Vec<(String, Profile)>)>> {
    let path = dir.join("settings.json");
    if !path.exists() && !bak(&path).exists() {
        return Ok(None);
    }
    let mut last = anyhow!("no readable copy");
    let mut settings = None;
    for c in [path.clone(), bak(&path)] {
        match read_json::<V1Settings>(&c).and_then(convert_v1) {
            Ok(s) => {
                settings = Some(s);
                break;
            }
            Err(e) => last = e,
        }
    }
    let settings = settings.ok_or(last)?;
    let mut profiles = vec![];
    for e in fs::read_dir(dir.join("profiles")).into_iter().flatten().flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "json")
            && let Some(name) = p.file_stem().map(|s| s.to_string_lossy().into_owned())
            && let Ok(s) = read_json::<V1Settings>(&p).and_then(convert_v1)
        {
            profiles.push((name, Profile { curves: s.curves, fans: s.fans, lighting: s.lighting }));
        }
    }
    Ok(Some((settings, profiles)))
}

pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let bytes: Vec<u8> = s.bytes().filter(|&c| c != b'=').collect();
    let mut out = vec![];
    for chunk in bytes.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= val(c)? << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("stellar-test-{}", nanos()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn curve_rule() -> FanRule {
        FanRule { control_id: "/lpc/x/control/0".into(), name: "A".into(), sensors: vec!["t".into()], mode: FanMode::Manual, ..Default::default() }
    }

    fn graph(id: &str) -> CurveDef {
        CurveDef { response_s: 0.0, id: id.into(), name: id.into(), kind: CurveKind::Graph { sensors: vec!["cpu".into()], mix: Mix::Max, offset: 0.0, points: preset(CurvePreset::Balanced) } }
    }

    #[test]
    fn base64() {
        assert_eq!(base64_decode("////"), Some(vec![255, 255, 255]));
        assert_eq!(base64_decode("AAEC"), Some(vec![0, 1, 2]));
    }

    #[test]
    fn validation() {
        assert!(curve_rule().validate_with(&[]).is_ok());
        assert!(FanRule { sensors: vec![], ..curve_rule() }.validate_with(&[]).is_err(), "manual needs failsafe sensor");
        assert!(FanRule { sensors: vec![], mode: FanMode::Firmware, ..curve_rule() }.validate_with(&[]).is_ok());
        let follows = FanRule { sensors: vec![], mode: FanMode::Curve, curve: Some("cpu".into()), ..curve_rule() };
        assert!(follows.validate_with(&[graph("cpu")]).is_ok(), "curve sensors feed the failsafe");
        assert_eq!(follows.watched(&[graph("cpu")]), vec!["cpu"]);
        assert!(follows.validate_with(&[]).is_err(), "missing curve");
        let mut bad = graph("x");
        if let CurveKind::Graph { points, .. } = &mut bad.kind {
            points[1].percent = 10.0;
        }
        assert!(Settings { curves: vec![bad], ..Default::default() }.validate().is_err(), "decreasing curve");
        assert!(FanRule { critical_temp: 120.0, ..curve_rule() }.validate().is_err());
        assert!(FanRule { minimum_percent: f64::NAN, ..curve_rule() }.validate().is_err());
        assert!(FanRule { control_id: " ".into(), ..curve_rule() }.validate().is_err());
        let dup = Settings { fans: vec![curve_rule(), curve_rule()], ..Default::default() };
        assert!(dup.validate().is_err());
        assert!(validate_profile_name("Gaming 1").is_ok());
        assert!(validate_profile_name("com1").is_err());
        assert!(validate_profile_name("../x").is_err());
        assert!(validate_profile_name(" x").is_err());
    }

    #[test]
    fn quiet_hours() {
        let q = QuietHours { start_min: 23 * 60, end_min: 7 * 60 };
        assert!(q.contains(0) && q.contains(23 * 60) && !q.contains(12 * 60) && !q.contains(7 * 60));
        let d = QuietHours { start_min: 60, end_min: 120 };
        assert!(d.contains(60) && !d.contains(120));
    }

    #[test]
    fn expected_rpm() {
        let r = FanRule {
            calibration: vec![CalibrationPoint { percent: 100.0, rpm: 2000.0 }, CalibrationPoint { percent: 0.0, rpm: 0.0 }],
            ..Default::default()
        };
        assert_eq!(r.expected_rpm(50.0), Some(1000.0));
        assert_eq!(FanRule::default().expected_rpm(50.0), None);
    }

    #[test]
    fn save_load_backup_and_corruption() {
        let d = tmp();
        let mut s = Store::at(d.clone());
        let settings = Settings { fans: vec![curve_rule()], ..Default::default() };
        s.save(&settings).unwrap();
        assert_eq!(s.load_with_legacy(&d.join("none")), settings);
        fs::write(s.settings_path(), b"{garbage").unwrap();
        assert_eq!(s.load_with_legacy(&d.join("none")), settings);
        assert!(s.warning.as_deref().unwrap().contains("backup"));
        fs::write(bak(&s.settings_path()), b"{garbage").unwrap();
        let loaded = s.load_with_legacy(&d.join("none"));
        assert!(loaded.fans.is_empty(), "corrupt copies default to firmware");
        assert_eq!(fs::read(s.settings_path()).unwrap(), b"{garbage", "damaged files untouched");
        assert!(s.save(&Settings { version: 9, ..Default::default() }).is_err());
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn profiles() {
        let d = tmp();
        let mut s = Store::at(d.clone());
        let p = Profile { fans: vec![curve_rule()], ..Default::default() };
        s.save_profile("Gaming", &p).unwrap();
        assert_eq!(s.profile_names(), vec!["Gaming"]);
        assert_eq!(s.load_profile("Gaming").unwrap(), p);
        assert!(s.save_profile("bad/name", &p).is_err());
        fs::write(s.profiles_dir().join("Gaming.json"), b"x").unwrap();
        fs::write(s.profiles_dir().join("Gaming.json.bak"), b"x").unwrap();
        assert!(s.load_profile("Gaming").is_err(), "corrupt profile never silently becomes defaults");
        s.delete_profile("Gaming").unwrap();
        assert!(s.profile_names().is_empty());
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn migrates_previous_app() {
        let d = tmp();
        let legacy = d.join("legacy");
        fs::create_dir_all(legacy.join("profiles")).unwrap();
        let v1 = r#"{"Version":1,"LightingOn":false,"ActiveProfile":null,"Color":"/wAA",
            "ZoneSizes":{"ASUS":{"Aura Addressable 1":120}},
            "Fans":[{"ControlId":"/lpc/nct6799d/0/control/1","Name":"Fan #2","RpmSensorId":"/lpc/nct6799d/0/fan/1",
            "Calibration":[{"Percent":0,"Rpm":755}],"TemperatureSensorIds":[],"Mode":"Firmware","ManualPercent":50,
            "MinimumPercent":30,"StartPercent":40,"CriticalTemperature":85,"Hysteresis":2,"RampDownPerSecond":5,
            "Points":[{"Temperature":35,"Percent":40},{"Temperature":85,"Percent":100}]}]}"#;
        fs::write(legacy.join("settings.json"), v1).unwrap();
        fs::write(legacy.join("profiles").join("Quiet.json"), v1).unwrap();
        let mut s = Store::at(d.join("new"));
        let loaded = s.load_with_legacy(&legacy);
        assert!(!loaded.lighting.on);
        assert_eq!(loaded.lighting.effect, Effect::Static { color: [255, 0, 0] });
        assert_eq!(loaded.lighting.zone_sizes["ASUS"]["Aura Addressable 1"], 120);
        assert_eq!(loaded.fans[0].mode, FanMode::Firmware, "imported fans stay in firmware mode");
        assert_eq!(loaded.fans[0].calibration[0].rpm, 755.0);
        let curve_v1 = v1.replace(r#""TemperatureSensorIds":[],"Mode":"Firmware""#, r#""TemperatureSensorIds":["/amdcpu/0/temperature/2"],"Mode":"Curve""#);
        let converted = convert_v1(serde_json::from_str(&curve_v1).unwrap()).unwrap();
        assert_eq!(converted.fans[0].mode, FanMode::Curve);
        assert_eq!(converted.fans[0].watched(&converted.curves), vec!["/amdcpu/0/temperature/2"], "old curve became a reusable curve");
        assert_eq!(s.profile_names(), vec!["Quiet"]);
        assert!(s.settings_path().exists(), "imported settings persisted");
        assert_eq!(fs::read_to_string(legacy.join("settings.json")).unwrap(), v1, "legacy untouched");
        fs::remove_dir_all(d).unwrap();
    }
}
