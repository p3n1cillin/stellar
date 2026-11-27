//! The control loop. Runs on its own thread so cooling keeps working whether
//! or not the dashboard is open.

use crate::config::{FanMode, Profile, Settings, Store};
use crate::cooling::{Decision, Outputs, Sensor, cpu_temperature, gpu_temperature};
use crate::tuning::{AutoTune, Progress};
use crate::hardware::{Host, Snapshot, competing_writer, running_processes};
use crate::lighting::{DeviceInfo, Lighting};
use chrono::Timelike;
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const HISTORY: usize = 600; // 10 minutes at 1 Hz

pub enum Cmd {
    Save(Settings),
    ApplyProfile(String),
    SaveProfile(String),
    DeleteProfile(String),
    /// Measure one fan and store its calibration and limits.
    TuneFan(String),
    /// Measure every fan and apply tuned curves.
    AutoTune,
    CancelTuning,
    /// Rebuild tuned curves from stored measurements (no sweep), e.g. after
    /// changing the CPU cooler type.
    RebuildCurves(crate::config::CpuCooler),
    SetLighting(bool),
    ReconnectLighting,
    Quit,
}

#[derive(Default)]
pub struct Shared {
    pub settings: Settings,
    pub generation: u64,
    pub snapshot: Snapshot,
    pub decisions: HashMap<String, Decision>,
    pub history: VecDeque<(Option<f64>, Option<f64>)>,
    pub cooling_status: String,
    pub cooling_ok: bool,
    pub lighting_status: String,
    pub lighting_ok: bool,
    pub devices: Vec<DeviceInfo>,
    pub profiles: Vec<String>,
    pub tuning: Option<Progress>,
    pub notices: VecDeque<String>,
    pub warning: Option<String>,
    pub competing: Option<&'static str>,
    pub auto_profile: Option<String>,
    pub emergency: bool,
    pub stopped: bool,
}

impl Shared {
    pub fn notice(&mut self, msg: impl Into<String>) {
        let m = format!("{} · {}", chrono::Local::now().format("%H:%M:%S"), msg.into());
        crate::log(&m);
        self.notices.push_front(m);
        self.notices.truncate(50);
    }
    pub fn sensor(&self, id: &str) -> Option<&Sensor> {
        self.snapshot.sensors.iter().find(|s| s.id == id)
    }
}

pub type State = Arc<Mutex<Shared>>;

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

struct Engine {
    /// When the hardware host first answered; sensors need a moment to populate.
    first_read: Option<Instant>,
    state: State,
    store: Store,
    settings: Settings,
    host: Host,
    outputs: Outputs,
    lighting: Lighting,
    competing: Option<&'static str>,
    last_competing_check: Option<Instant>,
    last_auto_check: Instant,
    tuning: Option<AutoTune>,
    temps: HashMap<String, f64>,
    was_emergency: bool,
    repaint: Box<dyn Fn() + Send>,
}

pub fn spawn(state: State, rx: Receiver<Cmd>, repaint: Box<dyn Fn() + Send>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("stellar-engine".into())
        .spawn(move || {
            let mut store = Store::default_location();
            let settings = store.load();
            {
                let mut s = state.lock().unwrap();
                s.settings = settings.clone();
                s.generation += 1;
                s.profiles = store.profile_names();
                if let Some(w) = store.warning.take() {
                    s.notice(w.clone());
                    s.warning = Some(w);
                }
                s.cooling_status = "Starting hardware host…".into();
            }
            let mut e = Engine {
                state,
                store,
                settings,
                host: Host::new(),
                outputs: Outputs::default(),
                lighting: Lighting::new(),
                competing: None,
                last_competing_check: None,
                last_auto_check: Instant::now(),
                first_read: None,
                tuning: None,
                temps: HashMap::new(),
                was_emergency: false,
                repaint,
            };
            e.run(rx);
        })
        .expect("engine thread")
}

impl Engine {
    fn run(&mut self, rx: Receiver<Cmd>) {
        let start = Instant::now();
        let mut last_hw: Option<Instant> = None;
        loop {
            let mut quit = false;
            while let Ok(cmd) = rx.try_recv() {
                if matches!(cmd, Cmd::Quit) {
                    quit = true;
                } else {
                    self.command(cmd);
                }
            }
            if quit {
                break;
            }
            if last_hw.is_none_or(|t| t.elapsed() >= Duration::from_millis(1000)) {
                if last_hw.is_some_and(|t| t.elapsed() > Duration::from_secs(10)) {
                    // The loop stalled, almost always because Windows slept: forget
                    // smoothing history so stale anchors never hold a low speed.
                    self.outputs.policy.reset();
                    self.state.lock().unwrap().notice("Resumed — cooling policy restarted");
                }
                last_hw = Some(Instant::now());
                self.hardware_tick();
                if self.last_auto_check.elapsed() >= Duration::from_secs(3) {
                    self.last_auto_check = Instant::now();
                    self.automation();
                }
                (self.repaint)();
            }
            let minute = { let n = chrono::Local::now(); n.hour() * 60 + n.minute() };
            let quiet = self.settings.lighting.quiet_hours.is_some_and(|q| q.contains(minute));
            self.lighting.tick(&self.settings.lighting, start.elapsed().as_secs_f64(), &self.temps, quiet);
            {
                let mut s = self.state.lock().unwrap();
                if s.lighting_status != self.lighting.status || s.lighting_ok != self.lighting.ok {
                    s.lighting_status = self.lighting.status.clone();
                    s.lighting_ok = self.lighting.ok;
                    s.devices = self.lighting.devices();
                }
            }
            let animated = self.settings.lighting.on
                && (self.settings.lighting.effect.animated()
                    || self.settings.lighting.devices.values().any(|d| d.effect.as_ref().is_some_and(|e| e.animated())));
            std::thread::sleep(Duration::from_millis(if self.lighting.ok && animated { 33 } else { 100 }));
        }
        let mut errors = vec![];
        self.outputs.release_all(&mut self.host, &mut errors);
        for e in errors {
            crate::log(&e);
        }
        self.host.shutdown();
        self.lighting.shutdown();
        let mut s = self.state.lock().unwrap();
        s.stopped = true;
        crate::log("Engine stopped; fans returned to firmware control");
    }

    fn publish_settings(&mut self) {
        let mut s = self.state.lock().unwrap();
        s.settings = self.settings.clone();
        s.generation += 1;
        s.profiles = self.store.profile_names();
        if let Some(w) = self.store.warning.take() {
            s.notice(w);
        }
    }

    fn commit(&mut self, new: Settings) -> anyhow::Result<()> {
        self.store.save(&new)?;
        self.settings = new;
        self.publish_settings();
        Ok(())
    }

    fn apply_profile(&mut self, name: &str) -> anyhow::Result<()> {
        let p = self.store.load_profile(name)?;
        let mut new = self.settings.clone();
        new.curves = p.curves;
        new.fans = p.fans;
        new.lighting = p.lighting;
        new.active_profile = Some(name.to_string());
        self.commit(new)
    }

    fn command(&mut self, cmd: Cmd) {
        let result: anyhow::Result<Option<String>> = (|| {
            Ok(match cmd {
                Cmd::Save(new) => {
                    self.commit(new)?;
                    Some("Settings saved".into())
                }
                Cmd::ApplyProfile(name) => {
                    self.apply_profile(&name)?;
                    self.state.lock().unwrap().auto_profile = None;
                    Some(format!("Profile '{name}' applied"))
                }
                Cmd::SaveProfile(name) => {
                    let p = Profile { curves: self.settings.curves.clone(), fans: self.settings.fans.clone(), lighting: self.settings.lighting.clone() };
                    self.store.save_profile(&name, &p)?;
                    let mut new = self.settings.clone();
                    new.active_profile = Some(name.clone());
                    self.commit(new)?;
                    Some(format!("Profile '{name}' saved"))
                }
                Cmd::DeleteProfile(name) => {
                    self.store.delete_profile(&name)?;
                    let mut new = self.settings.clone();
                    if new.active_profile.as_deref() == Some(&name) {
                        new.active_profile = None;
                    }
                    if new.fallback_profile.as_deref() == Some(&name) {
                        new.fallback_profile = None;
                    }
                    new.automation.retain(|a| a.profile != name);
                    self.commit(new)?;
                    Some(format!("Profile '{name}' deleted"))
                }
                Cmd::SetLighting(on) => {
                    let mut new = self.settings.clone();
                    new.lighting.on = on;
                    self.commit(new)?;
                    None
                }
                Cmd::ReconnectLighting => {
                    self.lighting.reconnect();
                    Some("Reconnecting lighting".into())
                }
                Cmd::TuneFan(control) => self.start_tuning(Some(control))?,
                Cmd::AutoTune => self.start_tuning(None)?,
                Cmd::RebuildCurves(cooler) => {
                    if self.tuning.is_some() {
                        anyhow::bail!("Wait for auto-tune to finish");
                    }
                    let mut base = self.settings.clone();
                    base.cpu_cooler = cooler;
                    let measured = crate::tuning::stored(&base);
                    if measured.is_empty() {
                        anyhow::bail!("No measurements yet — run Auto-tune first");
                    }
                    let backup = Profile { curves: self.settings.curves.clone(), fans: self.settings.fans.clone(), lighting: self.settings.lighting.clone() };
                    self.store.save_profile("Before auto-tune", &backup)?;
                    let sensors = self.state.lock().unwrap().snapshot.sensors.clone();
                    let (mut new, _) = crate::tuning::build(&base, &measured, &sensors, true);
                    new.active_profile = None;
                    self.commit(new)?;
                    self.outputs.policy.reset();
                    Some(format!("Tuned curves rebuilt for {} cooling", if cooler == crate::config::CpuCooler::Aio { "AIO" } else { "air" }))
                }
                Cmd::CancelTuning => {
                    self.stop_tuning("Auto-tune cancelled; fans returned to normal control");
                    None
                }
                Cmd::Quit => None,
            })
        })();
        let mut s = self.state.lock().unwrap();
        match result {
            Ok(Some(msg)) => s.notice(msg),
            Ok(None) => {}
            Err(e) => s.notice(format!("⚠ {e:#}")),
        }
    }

    fn start_tuning(&mut self, only: Option<String>) -> anyhow::Result<Option<String>> {
        if self.tuning.is_some() {
            anyhow::bail!("Auto-tune is already running");
        }
        if let Some(c) = self.competing {
            anyhow::bail!("{c} is running; quit it before tuning fans");
        }
        let sensors = self.state.lock().unwrap().snapshot.sensors.clone();
        let targets = crate::tuning::targets(&sensors, &self.settings, only.as_deref());
        if targets.is_empty() {
            anyhow::bail!("No fan outputs with RPM sensors were found");
        }
        let n = targets.len();
        self.tuning = Some(AutoTune::new(targets, only.is_none()));
        Ok(Some(format!("Auto-tune started for {n} fan header{} — about {} minutes. Fans will speed up, slow down and briefly stop.", if n == 1 { "" } else { "s" }, (n * 75).div_ceil(60))))
    }

    fn stop_tuning(&mut self, why: &str) {
        if self.tuning.take().is_some() {
            self.outputs.overrides.clear();
            self.outputs.policy.reset();
            let mut s = self.state.lock().unwrap();
            s.tuning = None;
            s.notice(why);
        }
    }

    fn tuning_tick(&mut self, sensors: &HashMap<String, Sensor>, decisions: &HashMap<String, Decision>, snapshot: &[Sensor]) {
        let Some(t) = &mut self.tuning else { return };
        // Never sweep a fan down while anything is hot or a safeguard fired.
        let hot = [cpu_temperature(snapshot), gpu_temperature(snapshot, None)]
            .into_iter()
            .flatten()
            .filter_map(|s| s.value)
            .any(|v| v >= crate::tuning::ABORT_TEMP);
        let tripped = t.current_control().is_some_and(|c| decisions.get(c).is_some_and(|d| d.emergency));
        if hot || tripped {
            self.stop_tuning("⚠ Auto-tune stopped: temperatures too high. Fans returned to normal control.");
            return;
        }
        match t.tick(sensors) {
            crate::tuning::Step::Hold(control, pct) => {
                self.outputs.overrides.clear();
                self.outputs.overrides.insert(control, pct);
                let p = t.progress();
                self.state.lock().unwrap().tuning = p;
            }
            crate::tuning::Step::Done(measured) => {
                let apply = t.apply_curves;
                self.tuning = None;
                self.outputs.overrides.clear();
                self.outputs.policy.reset();
                self.state.lock().unwrap().tuning = None;
                if apply {
                    let backup = Profile { curves: self.settings.curves.clone(), fans: self.settings.fans.clone(), lighting: self.settings.lighting.clone() };
                    if let Err(e) = self.store.save_profile("Before auto-tune", &backup) {
                        self.state.lock().unwrap().notice(format!("⚠ Could not back up settings: {e}"));
                    }
                }
                let (new, notes) = crate::tuning::build(&self.settings, &measured, snapshot, apply);
                for n in &notes {
                    self.state.lock().unwrap().notice(n.clone());
                }
                let mut new = new;
                if apply {
                    new.active_profile = None;
                }
                match self.commit(new) {
                    Ok(()) => self.state.lock().unwrap().notice(if apply {
                        "Auto-tune complete — tuned curves are active (previous setup saved as profile 'Before auto-tune')"
                    } else {
                        "Calibration saved"
                    }),
                    Err(e) => self.state.lock().unwrap().notice(format!("⚠ Tuning results could not be saved: {e:#}")),
                }
            }
        }
    }

    fn hardware_tick(&mut self) {
        if self.last_competing_check.is_none_or(|t| t.elapsed() >= Duration::from_secs(5)) {
            self.last_competing_check = Some(Instant::now());
            self.competing = competing_writer(&running_processes());
        }
        let mut errors = vec![];
        let (snapshot, read_error) = match self.host.read() {
            Ok(s) => (s, None),
            Err(e) => (Snapshot::default(), Some(e.to_string())),
        };
        // Timestamp after the read: the host stamps readings when it takes
        // them, and a slow read (the first one takes ~2 s) must not make them
        // look like they come from the future.
        let now = now_ms();
        let sensors: HashMap<String, Sensor> = snapshot.sensors.iter().map(|s| (s.id.clone(), s.clone())).collect();
        self.temps = snapshot
            .sensors
            .iter()
            .filter(|s| s.kind == "Temperature")
            .filter_map(|s| Some((s.id.clone(), s.value?)))
            .collect();
        if read_error.is_none() && self.first_read.is_none() {
            self.first_read = Some(Instant::now());
        }
        // Right after start-up LibreHardwareMonitor's first readings can be
        // empty. Rather than treat that as a failed sensor (100% blast), leave
        // fans on firmware control for up to 15 s until every watched sensor
        // reports. After that, missing sensors trigger the failsafe as usual.
        let warming = self.first_read.is_some_and(|t| t.elapsed() < Duration::from_secs(15))
            && self.settings.fans.iter().filter(|r| r.mode != FanMode::Firmware).flat_map(|r| r.watched(&self.settings.curves)).any(|id| !self.temps.contains_key(&id));
        let applied = if warming {
            Ok(HashMap::new())
        } else if read_error.is_some() {
            // Without telemetry nothing is written; the host's watchdog has
            // already returned outputs to firmware if it is unresponsive.
            Err(anyhow::anyhow!(read_error.clone().unwrap()))
        } else {
            self.outputs.apply(&self.settings.fans, &self.settings.curves, &sensors, &mut self.host, snapshot.admin, self.competing, now, &mut errors)
        };
        let (decisions, status, ok) = match applied {
            Ok(d) => {
                let active = self.settings.fans.iter().filter(|r| r.mode != FanMode::Firmware).count();
                let status = if warming {
                    "Starting — waiting for sensors".to_string()
                } else if active == 0 {
                    "All fans on firmware control".to_string()
                } else {
                    format!("Controlling {active} fan{}", if active == 1 { "" } else { "s" })
                };
                (d, status, true)
            }
            Err(e) => {
                if self.tuning.is_some() {
                    self.stop_tuning("⚠ Auto-tune stopped: fan outputs became unavailable");
                }
                (HashMap::new(), e.to_string(), false)
            }
        };
        self.tuning_tick(&sensors, &decisions, &snapshot.sensors);
        let emergency = decisions.values().any(|d| d.emergency);
        let cpu = cpu_temperature(&snapshot.sensors).and_then(|s| s.value);
        let gpu = gpu_temperature(&snapshot.sensors, None).and_then(|s| s.value);

        let mut s = self.state.lock().unwrap();
        if emergency && !self.was_emergency {
            let why: Vec<String> = decisions
                .iter()
                .filter(|(_, d)| d.emergency)
                .map(|(id, d)| format!("{}: {}", self.settings.rule(id).map(|r| r.name.as_str()).unwrap_or(id), d.reason))
                .collect();
            s.notice(format!("⚠ {}", why.join("; ")));
        }
        self.was_emergency = emergency;
        for e in errors.iter().chain(snapshot.errors.iter()) {
            s.notice(format!("⚠ {e}"));
        }
        s.emergency = emergency;
        s.history.push_back((cpu, gpu));
        while s.history.len() > HISTORY {
            s.history.pop_front();
        }
        s.snapshot = snapshot;
        s.decisions = decisions;
        s.cooling_status = status;
        s.cooling_ok = ok;
        s.competing = self.competing;
    }

    fn automation(&mut self) {
        if self.settings.automation.is_empty() {
            return;
        }
        let running = running_processes();
        let matched = self
            .settings
            .automation
            .iter()
            .find(|a| running.contains(a.process.trim().to_lowercase().trim_end_matches(".exe")))
            .map(|a| a.profile.clone());
        let current = self.state.lock().unwrap().auto_profile.clone();
        if matched == current {
            return;
        }
        let target = matched.clone().or_else(|| self.settings.fallback_profile.clone());
        if let Some(name) = target {
            match self.apply_profile(&name) {
                Ok(()) => self.state.lock().unwrap().notice(format!("Automation switched to '{name}'")),
                Err(e) => self.state.lock().unwrap().notice(format!("⚠ Automation: {e}")),
            }
        }
        self.state.lock().unwrap().auto_profile = matched;
    }
}

pub fn channel() -> (Sender<Cmd>, Receiver<Cmd>) {
    std::sync::mpsc::channel()
}
