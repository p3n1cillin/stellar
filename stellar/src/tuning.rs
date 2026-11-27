//! Auto-tune: sweeps fan headers, measures how each fan (or fan hub) really
//! responds, and builds quiet, safe settings from the measurements.

use crate::config::{CalibrationPoint, CpuCooler, CurvePoint, FanMode, FanRule, Mix, Settings};
use crate::cooling::{Sensor, cpu_temperature, gpu_temperature, paired_rpm, tune};
use crate::curves::{CurveDef, CurveKind, MixFn};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// Duty steps, high to low: start fast and safe, finer near where fans stop.
pub const DOWN: [f64; 12] = [100.0, 80.0, 60.0, 50.0, 40.0, 30.0, 25.0, 20.0, 15.0, 10.0, 5.0, 0.0];
/// Above this any CPU/GPU temperature aborts tuning.
pub const ABORT_TEMP: f64 = 85.0;

#[derive(Clone, Debug)]
pub struct Target {
    pub control: String,
    pub name: String,
    pub rpm: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Measured {
    pub control: String,
    pub name: String,
    pub rpm: String,
    pub calibration: Vec<CalibrationPoint>,
    pub start: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct Progress {
    pub control: String,
    pub name: String,
    pub phase: &'static str,
    pub percent: f64,
    pub index: usize,
    pub total: usize,
}

struct Sweep {
    target: Target,
    up: bool,
    steps: Vec<f64>,
    idx: usize,
    since: Instant,
    results: Vec<CalibrationPoint>,
    start: Option<f64>,
}

pub enum Step {
    Hold(String, f64),
    Done(Vec<Measured>),
}

pub struct AutoTune {
    queue: VecDeque<Target>,
    current: Option<Sweep>,
    done: Vec<Measured>,
    total: usize,
    pub apply_curves: bool,
}

fn rpm_of(sensors: &HashMap<String, Sensor>, id: &str) -> f64 {
    sensors.get(id).and_then(|s| s.value).unwrap_or(0.0)
}

impl AutoTune {
    pub fn new(targets: Vec<Target>, apply_curves: bool) -> Self {
        Self { total: targets.len(), queue: targets.into(), current: None, done: vec![], apply_curves }
    }

    pub fn progress(&self) -> Option<Progress> {
        let s = self.current.as_ref()?;
        Some(Progress {
            control: s.target.control.clone(),
            name: s.target.name.clone(),
            phase: if s.up { "finding start speed" } else { "sweeping" },
            percent: s.steps[s.idx.min(s.steps.len() - 1)],
            index: self.done.len() + 1,
            total: self.total,
        })
    }

    pub fn current_control(&self) -> Option<&str> {
        self.current.as_ref().map(|s| s.target.control.as_str())
    }

    /// Advance with the latest readings (called about once a second).
    pub fn tick(&mut self, sensors: &HashMap<String, Sensor>) -> Step {
        loop {
            let Some(s) = &mut self.current else {
                match self.queue.pop_front() {
                    Some(t) => {
                        self.current = Some(Sweep { target: t, up: false, steps: DOWN.to_vec(), idx: 0, since: Instant::now(), results: vec![], start: None });
                        continue;
                    }
                    None => return Step::Done(std::mem::take(&mut self.done)),
                }
            };
            let pct = s.steps[s.idx];
            // Let the fan settle: first step and full stop take longest.
            let hold = if s.idx == 0 || pct == 0.0 { 8 } else if s.up { 4 } else { 5 };
            if s.since.elapsed() < Duration::from_secs(hold) {
                return Step::Hold(s.target.control.clone(), pct);
            }
            let rpm = rpm_of(sensors, &s.target.rpm);
            if !s.up {
                s.results.push(CalibrationPoint { percent: pct, rpm: rpm.round() });
                if s.idx == 0 && rpm < 150.0 {
                    // Nothing spinning at full duty: empty header or no tach wire.
                    self.finish();
                    continue;
                }
                s.idx += 1;
                if s.idx < s.steps.len() {
                    s.since = Instant::now();
                    continue;
                }
                // Swept down. If the fan stopped, measure what it takes to restart.
                let max = s.results.iter().map(|p| p.rpm).fold(0.0, f64::max);
                let threshold = (max * 0.08).max(120.0);
                let stop = s.results.iter().filter(|p| p.rpm >= threshold).map(|p| p.percent).fold(f64::MAX, f64::min);
                if s.results.iter().any(|p| p.rpm < threshold) && stop < f64::MAX {
                    let lo = (stop - 5.0).max(5.0);
                    s.steps = (0..).map(|i| lo + 5.0 * i as f64).take_while(|&v| v <= (stop + 30.0).min(70.0)).collect();
                    s.up = true;
                    s.idx = 0;
                    s.since = Instant::now();
                    continue;
                }
                self.finish();
                continue;
            }
            let max = s.results.iter().map(|p| p.rpm).fold(0.0, f64::max);
            if rpm >= (max * 0.08).max(120.0) {
                s.start = Some(pct);
                self.finish();
                continue;
            }
            s.idx += 1;
            if s.idx >= s.steps.len() {
                self.finish();
                continue;
            }
            s.since = Instant::now();
        }
    }

    fn finish(&mut self) {
        if let Some(s) = self.current.take() {
            let mut cal = s.results;
            cal.sort_by(|a, b| a.percent.total_cmp(&b.percent));
            self.done.push(Measured { control: s.target.control, name: s.target.name, rpm: s.target.rpm, calibration: cal, start: s.start });
        }
    }
}

/// Every controllable output that has a matching RPM sensor.
pub fn targets(sensors: &[Sensor], settings: &Settings, only: Option<&str>) -> Vec<Target> {
    sensors
        .iter()
        .filter(|s| s.kind == "Control" && s.can_control && only.is_none_or(|o| o == s.id))
        .filter_map(|s| {
            let rule = settings.rule(&s.id);
            let rpm = rule.and_then(|r| r.rpm_sensor_id.clone()).unwrap_or_else(|| paired_rpm(&s.id));
            sensors.iter().any(|x| x.id == rpm && x.kind == "Fan").then(|| Target {
                control: s.id.clone(),
                name: rule.map(|r| r.name.clone()).unwrap_or_else(|| s.name.clone()),
                rpm,
            })
        })
        .collect()
}

/// Auto-tune owns three curves by id; re-tuning updates them in place.
fn upsert_curve(curves: &mut Vec<CurveDef>, id: &str, name: &str, kind: CurveKind, response_s: f64) -> String {
    match curves.iter_mut().find(|c| c.id == id) {
        Some(c) => {
            c.name = name.into();
            c.kind = kind;
            c.response_s = response_s;
        }
        None => curves.push(CurveDef { response_s, id: id.into(), name: name.into(), kind }),
    }
    id.into()
}

/// Measurements stored on fan rules, so curves can be rebuilt without a sweep.
pub fn stored(settings: &Settings) -> Vec<Measured> {
    settings
        .fans
        .iter()
        .filter(|r| !r.calibration.is_empty())
        .map(|r| Measured {
            control: r.control_id.clone(),
            name: r.name.clone(),
            rpm: r.rpm_sensor_id.clone().unwrap_or_else(|| paired_rpm(&r.control_id)),
            calibration: r.calibration.clone(),
            start: r.measured_start,
        })
        .collect()
}

fn graph(sensor: &str, pts: &[(f64, f64)]) -> CurveKind {
    CurveKind::Graph {
        sensors: vec![sensor.into()],
        mix: Mix::Max,
        offset: 0.0,
        points: pts.iter().map(|&(temp, percent)| CurvePoint { temp, percent }).collect(),
    }
}

/// Applies measurements to the settings. Motherboard headers get tuned limits
/// and, with `apply_curves`, Stellar curves: the CPU header follows the CPU,
/// every other header (case fans, hubs) follows the hotter of CPU and GPU.
/// GPU fans keep their mode; only their calibration and limits are stored.
pub fn build(settings: &Settings, measured: &[Measured], sensors: &[Sensor], apply_curves: bool) -> (Settings, Vec<String>) {
    let mut new = settings.clone();
    let mut notes = vec![];
    let cpu = cpu_temperature(sensors).map(|s| s.id.clone());
    let gpu = gpu_temperature(sensors, None).map(|s| s.id.clone());
    let amd = cpu.as_deref().is_some_and(|id| id.starts_with("/amdcpu/"));
    let aio = settings.cpu_cooler == CpuCooler::Aio;
    let mut cpu_curve = None;
    let mut case_curve = None;
    if apply_curves {
        if let Some(c) = &cpu {
            // Ryzen 7000 runs up to 95 °C by design; Intel package targets are lower.
            // An AIO's coolant lags the CPU: a gentler curve on a smoothed input
            // keeps radiator fans from chasing momentary Tctl spikes.
            let pts: &[(f64, f64)] = match (amd, aio) {
                (true, true) => &[(50.0, 0.0), (70.0, 35.0), (82.0, 65.0), (90.0, 100.0)],
                (true, false) => &[(50.0, 0.0), (65.0, 35.0), (80.0, 70.0), (90.0, 100.0)],
                (false, true) => &[(45.0, 0.0), (65.0, 35.0), (78.0, 65.0), (88.0, 100.0)],
                (false, false) => &[(45.0, 0.0), (60.0, 35.0), (75.0, 70.0), (88.0, 100.0)],
            };
            let name = if aio { "CPU (AIO radiator)" } else { "CPU" };
            cpu_curve = Some(upsert_curve(&mut new.curves, "auto-cpu", name, graph(c, pts), if aio { 10.0 } else { 3.0 }));
        }
        let gpu_curve = gpu.as_ref().map(|g| upsert_curve(&mut new.curves, "auto-gpu", "GPU", graph(g, &[(45.0, 0.0), (60.0, 40.0), (70.0, 70.0), (80.0, 100.0)]), 3.0));
        case_curve = match (&cpu_curve, &gpu_curve) {
            (Some(a), Some(b)) => Some(upsert_curve(&mut new.curves, "auto-case", "Case", CurveKind::Mix { curves: vec![a.clone(), b.clone()], function: MixFn::Max }, 0.0)),
            (a, b) => a.clone().or(b.clone()),
        };
    }
    let first_board = measured.iter().find(|m| m.control.starts_with("/lpc/")).map(|m| m.control.clone());
    for m in measured {
        let Some(t) = tune(&m.calibration, m.start) else {
            notes.push(format!("{}: no fan detected — left on firmware control", m.name));
            continue;
        };
        let idx = match new.fans.iter().position(|r| r.control_id == m.control) {
            Some(i) => i,
            None => {
                new.fans.push(FanRule { control_id: m.control.clone(), name: m.name.clone(), ..Default::default() });
                new.fans.len() - 1
            }
        };
        let r = &mut new.fans[idx];
        r.calibration = m.calibration.clone();
        r.rpm_sensor_id = Some(m.rpm.clone());
        r.measured_start = m.start;
        r.minimum_percent = t.minimum;
        r.start_percent = t.start;
        r.allow_stop = false;
        notes.push(format!("{}: {}", r.name, t.summary));
        if apply_curves && m.control.starts_with("/lpc/") {
            let curve = if Some(&m.control) == first_board.as_ref() { cpu_curve.clone() } else { case_curve.clone() };
            if let Some(c) = curve {
                r.mode = FanMode::Curve;
                r.curve = Some(c);
                r.critical_temp = if amd { 95.0 } else { 90.0 };
                r.hysteresis = if aio { 4.0 } else { 3.0 };
                r.step_up_per_sec = if aio { 10.0 } else { 20.0 };
                r.ramp_down_per_sec = 4.0;
                r.offset_percent = 0.0;
            }
        }
    }
    (new, notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sensor(id: &str, kind: &str, name: &str, v: f64) -> Sensor {
        Sensor { id: id.into(), hardware: "H".into(), hardware_type: String::new(), name: name.into(), kind: kind.into(), value: Some(v), updated_ms: 0, can_control: kind == "Control" }
    }

    #[test]
    fn sweep_finds_stop_and_start() {
        let t = Target { control: "/lpc/c/0/control/0".into(), name: "Fan".into(), rpm: "/lpc/c/0/fan/0".into() };
        let mut a = AutoTune::new(vec![t], true);
        // Fake fan: spins at duty*15 RPM from 20% up, stops below; needs 25% to start.
        let mut running = true;
        let mut steps = 0;
        let mut last = 100.0;
        let measured = loop {
            let rpm = if (running && last >= 20.0) || last >= 25.0 { running = true; last * 15.0 } else { running = false; 0.0 };
            let m: HashMap<String, Sensor> = [("/lpc/c/0/fan/0".to_string(), sensor("/lpc/c/0/fan/0", "Fan", "Fan", rpm))].into();
            // Pretend the hold time has passed.
            if let Some(s) = &mut a.current {
                s.since = Instant::now() - Duration::from_secs(10);
            }
            match a.tick(&m) {
                Step::Hold(_, p) => last = p,
                Step::Done(d) => break d,
            }
            steps += 1;
            assert!(steps < 100);
        };
        assert_eq!(measured.len(), 1);
        assert_eq!(measured[0].start, Some(25.0));
        let tuned = tune(&measured[0].calibration, measured[0].start).unwrap();
        assert_eq!(tuned.minimum, 28.0, "stops below 20% plus hub margin");
        assert_eq!(tuned.start, 35.0);
    }

    #[test]
    fn empty_header_is_skipped_quickly() {
        let t = Target { control: "/lpc/c/0/control/4".into(), name: "Fan #5".into(), rpm: "/lpc/c/0/fan/4".into() };
        let mut a = AutoTune::new(vec![t], true);
        let m: HashMap<String, Sensor> = [("/lpc/c/0/fan/4".to_string(), sensor("/lpc/c/0/fan/4", "Fan", "Fan #5", 0.0))].into();
        assert!(matches!(a.tick(&m), Step::Hold(_, 100.0)));
        a.current.as_mut().unwrap().since = Instant::now() - Duration::from_secs(10);
        let Step::Done(d) = a.tick(&m) else { panic!("should finish") };
        assert_eq!(d[0].calibration.len(), 1);
        let (s, notes) = build(&Settings::default(), &d, &[], true);
        assert!(s.fans.is_empty() && notes[0].contains("no fan detected"));
    }

    #[test]
    fn build_assigns_cpu_and_case_curves() {
        let sensors = vec![
            sensor("/amdcpu/0/temperature/2", "Temperature", "Core (Tctl/Tdie)", 50.0),
            sensor("/gpu-nvidia/0/temperature/0", "Temperature", "GPU Core", 45.0),
        ];
        let cal = |k: f64| (0..=10).map(|i| CalibrationPoint { percent: i as f64 * 10.0, rpm: if i >= 2 { i as f64 * k } else { 0.0 } }).collect::<Vec<_>>();
        let m = |c: &str, n: &str| Measured { control: c.into(), name: n.into(), rpm: c.replace("control", "fan"), calibration: cal(150.0), start: Some(20.0) };
        let measured = vec![m("/lpc/n/0/control/0", "Fan #1"), m("/lpc/n/0/control/1", "Fan #2"), m("/gpu-nvidia/0/control/1", "GPU")];
        let (s, notes) = build(&Settings::default(), &measured, &sensors, true);
        s.validate().unwrap();
        assert_eq!(notes.len(), 3);
        let names = |id: &str| s.curves.iter().find(|c| c.id == id).unwrap().name.clone();
        assert_eq!(names(s.fans[0].curve.as_ref().unwrap()), "CPU");
        assert_eq!(names(s.fans[1].curve.as_ref().unwrap()), "Case");
        assert_eq!(s.fans[1].watched(&s.curves).len(), 2, "case fans watch CPU and GPU");
        assert_eq!(s.fans[2].mode, FanMode::Firmware, "GPU keeps its own control");
        assert_eq!(s.fans[0].critical_temp, 95.0);
        // Running it again updates the same curves instead of duplicating them.
        let (again, _) = build(&s, &measured, &sensors, true);
        assert_eq!(again.curves.len(), 3);
        // Switching to an AIO rebuilds from stored measurements, no sweep needed.
        let aio_settings = Settings { cpu_cooler: CpuCooler::Aio, ..again };
        let (aio, _) = build(&aio_settings, &stored(&aio_settings), &sensors, true);
        aio.validate().unwrap();
        assert_eq!(aio.curves.len(), 3);
        let cpu = aio.curves.iter().find(|c| c.id == "auto-cpu").unwrap();
        assert_eq!((cpu.name.as_str(), cpu.response_s), ("CPU (AIO radiator)", 10.0));
        assert_eq!(aio.fans[0].hysteresis, 4.0);
        assert_eq!(aio.fans[0].start_percent, s.fans[0].start_percent, "measured start survives a rebuild");
    }
}
