//! Fan policy and output ownership, ported from the verified C# engine.
//!
//! Safety rules (unchanged from the previous app):
//! * missing, stale or implausible temperature/RPM feedback -> 100 %
//! * the hottest raw input at or above the critical limit -> 100 %
//! * a paired fan under 100 RPM for 15 s while commanded -> 100 % (stall)
//! * increases are immediate; decreases use hysteresis and a slew limit
//! * any failed write drives every owned output to its maximum

use crate::config::{CalibrationPoint, CurveDef, FanMode, FanRule};
use crate::curves::Evaluator;
use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub const STALE_MS: i64 = 5_000;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Sensor {
    pub id: String,
    pub hardware: String,
    #[serde(default)]
    pub hardware_type: String,
    pub name: String,
    pub kind: String,
    pub value: Option<f64>,
    pub updated_ms: i64,
    #[serde(default)]
    pub can_control: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Decision {
    pub percent: Option<f64>,
    pub reason: String,
    pub emergency: bool,
    pub temperature: Option<f64>,
}

fn fresh(s: &Sensor, kind: &str, lo: f64, hi: f64, now: i64) -> Option<f64> {
    let v = s.value?;
    (s.kind == kind && v.is_finite() && v >= lo && v <= hi && now - s.updated_ms <= STALE_MS && s.updated_ms <= now + 1000)
        .then_some(v)
}

#[derive(Default)]
pub struct Policy {
    last: HashMap<String, (f64, f64, i64)>, // output percent, anchor temperature, time
    low_rpm_since: HashMap<String, i64>,
    pub curves: Evaluator,
}

impl Policy {
    pub fn decide(&mut self, rule: &FanRule, curves: &[CurveDef], sensors: &HashMap<String, Sensor>, now: i64) -> Result<Decision> {
        rule.validate_with(curves)?;
        if rule.mode == FanMode::Firmware {
            self.forget(&rule.control_id);
            return Ok(Decision { percent: None, reason: "Firmware control".into(), emergency: false, temperature: None });
        }
        // Every sensor the curve (or the failsafe list) depends on must be fresh.
        let mut temps = HashMap::new();
        for id in rule.watched(curves) {
            match sensors.get(&id).and_then(|s| fresh(s, "Temperature", -10.0, 130.0, now)) {
                Some(v) => {
                    temps.insert(id, v);
                }
                None => return Ok(self.remember(rule, 100.0, f64::NAN, now, "Temperature unavailable — full cooling", true, None)),
            }
        }
        let hottest = temps.values().copied().fold(f64::MIN, f64::max);
        if hottest >= rule.critical_temp {
            return Ok(self.remember(rule, 100.0, hottest, now, "Temperature limit — full cooling", true, Some(hottest)));
        }
        let demand = match rule.mode {
            FanMode::Manual => rule.manual_percent,
            _ => match rule.curve.as_deref().and_then(|c| self.curves.eval_at(curves, c, &temps, now)) {
                Some(v) => v,
                None => return Ok(self.remember(rule, 100.0, f64::NAN, now, "Curve unavailable — full cooling", true, Some(hottest))),
            },
        };
        let demand = (demand + rule.offset_percent).clamp(0.0, 100.0);
        let mut target = if rule.allow_stop && demand < rule.minimum_percent { 0.0 } else { demand.max(rule.minimum_percent) };

        if let Some(rpm_id) = rule.rpm_sensor_id.as_deref().filter(|s| !s.trim().is_empty())
            && target > 0.0
        {
            match sensors.get(rpm_id).and_then(|s| fresh(s, "Fan", 0.0, 30000.0, now)) {
                None => {
                    return Ok(self.remember(rule, 100.0, f64::NAN, now, "RPM feedback unavailable — full cooling", true, Some(hottest)));
                }
                Some(rpm) if rpm < 100.0 => {
                    let since = *self.low_rpm_since.entry(rule.control_id.clone()).or_insert(now);
                    if now - since >= 15_000 {
                        return Ok(self.remember(rule, 100.0, f64::NAN, now, "Fan may be stalled — full cooling", true, Some(hottest)));
                    }
                }
                Some(_) => {
                    self.low_rpm_since.remove(&rule.control_id);
                }
            }
        } else {
            self.low_rpm_since.remove(&rule.control_id);
        }

        match self.last.get(&rule.control_id) {
            Some(&(prev, prev_temp, prev_time)) => {
                let dt = ((now - prev_time) as f64 / 1000.0).clamp(0.0, 5.0);
                if target > prev {
                    // From standstill, kick the fan with the start speed.
                    if prev <= 0.0 && target > 0.0 {
                        target = target.max(rule.start_percent);
                    } else if prev_temp.is_finite() {
                        target = target.min(prev + rule.step_up_per_sec * dt.max(1.0));
                    }
                } else if target < prev {
                    if prev_temp.is_finite() && hottest > prev_temp - rule.hysteresis {
                        target = prev;
                    } else {
                        let floor = prev - rule.ramp_down_per_sec * dt;
                        target = if target == 0.0 && floor <= rule.minimum_percent { 0.0 } else { target.max(floor) };
                    }
                }
            }
            None => {
                if target > 0.0 {
                    target = target.max(rule.start_percent);
                }
            }
        }
        // Keep a stable temperature anchor while holding for hysteresis.
        let anchor = match self.last.get(&rule.control_id) {
            Some(&(p, t, _)) if target == p && hottest < t => t,
            _ => hottest,
        };
        let reason = if rule.mode == FanMode::Manual { "Manual with thermal protection" } else { "Following curve" };
        Ok(self.remember(rule, target.clamp(0.0, 100.0), anchor, now, reason, false, Some(hottest)))
    }

    #[allow(clippy::too_many_arguments)]
    fn remember(&mut self, rule: &FanRule, pct: f64, anchor: f64, now: i64, reason: &str, emergency: bool, temperature: Option<f64>) -> Decision {
        self.last.insert(rule.control_id.clone(), (pct, anchor, now));
        Decision { percent: Some(pct), reason: reason.into(), emergency, temperature }
    }

    pub fn reset(&mut self) {
        self.last.clear();
        self.low_rpm_since.clear();
        self.curves.reset();
    }
    pub fn forget(&mut self, id: &str) {
        self.last.remove(id);
        self.low_rpm_since.remove(id);
    }
    pub fn record_emergency(&mut self, id: &str, pct: f64, now: i64) {
        self.last.insert(id.into(), (pct, f64::NAN, now));
    }
}

// ---------------------------------------------------------------- tuning

/// What a calibration sweep says about a fan (or a hub of fans).
#[derive(Clone, Debug, PartialEq)]
pub struct Tune {
    /// Lowest duty that keeps every fan on the channel turning.
    pub minimum: f64,
    /// Duty that reliably starts the fans from standstill.
    pub start: f64,
    pub max_rpm: f64,
    /// Duty above which extra speed buys little airflow.
    pub efficient: f64,
    pub summary: String,
}

/// Derives safe, quiet limits from a duty→RPM sweep. A fan hub reports one
/// fan's tachometer, so a margin covers the unmonitored fans on the hub.
pub fn tune(cal: &[CalibrationPoint], measured_start: Option<f64>) -> Option<Tune> {
    let mut c = cal.to_vec();
    c.sort_by(|a, b| a.percent.total_cmp(&b.percent));
    let max_rpm = c.iter().map(|p| p.rpm).fold(0.0, f64::max);
    if max_rpm < 150.0 {
        return None; // nothing connected, or no tachometer signal
    }
    let threshold = (max_rpm * 0.08).max(120.0);
    let spinning: Vec<&CalibrationPoint> = c.iter().filter(|p| p.rpm >= threshold).collect();
    let stop = spinning.first()?.percent;
    let floor_rpm = spinning[0].rpm;
    // Below some duty many fans and hubs ignore PWM; going lower only adds noise risk.
    let flat_end = spinning.iter().take_while(|p| p.rpm <= floor_rpm * 1.06).map(|p| p.percent).last().unwrap_or(stop);
    const HUB_MARGIN: f64 = 8.0;
    let minimum = if stop > 0.0 { (stop + HUB_MARGIN).max(flat_end) } else { flat_end }.clamp(0.0, 60.0).round();
    let start = match measured_start {
        Some(s) => (s + 10.0).max(minimum),
        None if stop > 0.0 => (stop + 15.0).max(minimum),
        None => minimum,
    }
    .clamp(0.0, 100.0)
    .round();
    let efficient = c.iter().find(|p| p.rpm >= max_rpm * 0.92).map(|p| p.percent).unwrap_or(100.0).max(minimum + 10.0).min(100.0);
    let stops = if stop > 0.0 { format!("stops below {stop:.0}%") } else { "never stops (PWM floor)".into() };
    let summary = format!(
        "{stops}; flat until {flat_end:.0}%; max {max_rpm:.0} RPM; 92% of max at {efficient:.0}%. Minimum {minimum:.0}%, start {start:.0}%."
    );
    Some(Tune { minimum, start, max_rpm, efficient, summary })
}

/// Hardware write boundary. The real implementation is the LibreHardwareMonitor
/// host; tests use a fake and never touch real fans.
pub trait OutputBus {
    fn range(&self, id: &str) -> Option<(f64, f64)>;
    fn set(&mut self, id: &str, percent: f64) -> Result<()>;
    fn release(&mut self, id: &str) -> Result<()>;
}

/// Owns fan outputs exclusively and applies policy decisions to them.
#[derive(Default)]
pub struct Outputs {
    owned: HashSet<String>,
    pub policy: Policy,
    /// Outputs pinned by a calibration run: (percent).
    pub overrides: HashMap<String, f64>,
}

impl Outputs {
    #[allow(clippy::too_many_arguments)]
    pub fn apply(
        &mut self,
        rules: &[FanRule],
        curves: &[CurveDef],
        sensors: &HashMap<String, Sensor>,
        bus: &mut dyn OutputBus,
        admin: bool,
        competing: Option<&str>,
        now: i64,
        errors: &mut Vec<String>,
    ) -> Result<HashMap<String, Decision>> {
        if competing.is_some() && !self.owned.is_empty() {
            self.release_all(bus, errors);
        }
        let active = rules.iter().any(|r| r.mode != FanMode::Firmware) || !self.overrides.is_empty();
        if active {
            if let Some(name) = competing {
                bail!("{name} is running. Stellar leaves fans in firmware mode to prevent competing fan writers.");
            }
            if !admin {
                bail!("Administrator access is required for fan outputs");
            }
        }
        let mut decisions = HashMap::new();
        let result = (|| -> Result<()> {
            // Validate the whole update before the first write.
            crate::config::validate_curves(curves)?;
            for r in rules {
                r.validate_with(curves)?;
            }
            let ids: HashSet<&String> = rules.iter().map(|r| &r.control_id).collect();
            if ids.len() != rules.len() {
                bail!("Duplicate fan outputs");
            }
            let stale: Vec<String> =
                self.owned.iter().filter(|id| !ids.contains(id) && !self.overrides.contains_key(*id)).cloned().collect();
            for id in stale {
                self.release(bus, &id)?;
            }
            for rule in rules {
                let mut d = self.policy.decide(rule, curves, sensors, now)?;
                if let Some(&pin) = self.overrides.get(&rule.control_id)
                    && !d.emergency
                {
                    d = Decision { percent: Some(pin), reason: "Calibrating".into(), emergency: false, temperature: d.temperature };
                }
                let pct = d.percent;
                decisions.insert(rule.control_id.clone(), d);
                let Some(pct) = pct else {
                    self.release(bus, &rule.control_id)?;
                    continue;
                };
                self.write(bus, &rule.control_id, &rule.name, pct)?;
            }
            for (id, &pct) in &self.overrides.clone() {
                if !ids.contains(id) {
                    self.write(bus, id, id, pct)?;
                    decisions.insert(id.clone(), Decision { percent: Some(pct), reason: "Calibrating".into(), emergency: false, temperature: None });
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            for id in self.owned.clone() {
                let max = bus.range(&id).map(|r| r.1).unwrap_or(100.0);
                match bus.set(&id, max) {
                    Ok(()) => self.policy.record_emergency(&id, max, now),
                    Err(x) => errors.push(format!("Emergency cooling {id}: {x}")),
                }
            }
            return Err(e);
        }
        Ok(decisions)
    }

    fn write(&mut self, bus: &mut dyn OutputBus, id: &str, name: &str, pct: f64) -> Result<()> {
        let (min, max) = bus.range(id).ok_or_else(|| anyhow!("Fan output disappeared: {name}"))?;
        if !min.is_finite() || !max.is_finite() || min < 0.0 || max > 100.0 || min > max {
            bail!("Invalid speed range: {name}");
        }
        // Claim before writing: even a failed write must get a release attempt.
        self.owned.insert(id.to_string());
        bus.set(id, pct.clamp(min, max))
    }

    fn release(&mut self, bus: &mut dyn OutputBus, id: &str) -> Result<()> {
        if !self.owned.contains(id) {
            return Ok(());
        }
        bus.release(id)?;
        self.owned.remove(id);
        self.policy.forget(id);
        Ok(())
    }

    pub fn release_all(&mut self, bus: &mut dyn OutputBus, errors: &mut Vec<String>) {
        for id in self.owned.clone() {
            if let Err(e) = self.release(bus, &id) {
                errors.push(format!("Release {id}: {e}"));
            }
        }
    }

}

// ---------------------------------------------------------------- defaults

/// Default temperature for a CPU: never an unlabelled motherboard probe.
pub fn cpu_temperature<'a>(sensors: impl IntoIterator<Item = &'a Sensor>) -> Option<&'a Sensor> {
    let rank = |s: &Sensor| {
        if s.name.eq_ignore_ascii_case("CPU Package") {
            0
        } else if s.name.to_lowercase().contains("tctl/tdie") {
            1
        } else {
            2
        }
    };
    let mut c: Vec<&Sensor> = sensors
        .into_iter()
        .filter(|s| s.kind == "Temperature")
        .filter(|s| {
            let id = s.id.to_lowercase();
            id.starts_with("/amdcpu/") || id.starts_with("/intelcpu/")
        })
        .filter(|s| {
            let n = s.name.to_lowercase();
            n.contains("package") || n.contains("tctl")
        })
        .collect();
    c.sort_by_key(|s| rank(s));
    c.into_iter().next()
}

pub fn gpu_temperature<'a>(sensors: impl IntoIterator<Item = &'a Sensor>, hardware: Option<&str>) -> Option<&'a Sensor> {
    sensors.into_iter().find(|s| {
        s.kind == "Temperature"
            && s.id.to_lowercase().starts_with("/gpu-")
            && hardware.is_none_or(|h| s.hardware == h)
            && s.name.eq_ignore_ascii_case("GPU Core")
    })
}

pub fn temperature_for_output<'a>(sensors: &'a [Sensor], output: &Sensor) -> Option<&'a Sensor> {
    if output.id.to_lowercase().starts_with("/gpu-") {
        gpu_temperature(sensors, Some(&output.hardware))
    } else {
        cpu_temperature(sensors)
    }
}

/// The RPM sensor that shares an output's channel index, e.g.
/// `/lpc/chip/0/control/3` -> `/lpc/chip/0/fan/3`.
pub fn paired_rpm(output_id: &str) -> String {
    output_id.replacen("/control/", "/fan/", 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CurvePoint, Mix};
    use crate::curves::CurveKind;

    const T: &str = "/amdcpu/0/temperature/2";
    const R: &str = "/lpc/c/0/fan/0";
    const C: &str = "/lpc/c/0/control/0";

    fn s(id: &str, kind: &str, v: Option<f64>, t: i64) -> Sensor {
        Sensor { id: id.into(), hardware: "H".into(), hardware_type: String::new(), name: id.into(), kind: kind.into(), value: v, updated_ms: t, can_control: false }
    }
    fn map(v: Vec<Sensor>) -> HashMap<String, Sensor> {
        v.into_iter().map(|s| (s.id.clone(), s)).collect()
    }
    /// 35°C → 40%, 55 → 55, 70 → 80, 85 → 100
    fn curves() -> Vec<CurveDef> {
        vec![CurveDef { response_s: 0.0,
            id: "cpu".into(),
            name: "CPU".into(),
            kind: CurveKind::Graph { sensors: vec![T.into()], mix: Mix::Max, offset: 0.0, points: crate::config::preset(crate::config::CurvePreset::Balanced) },
        }]
    }
    fn rule() -> FanRule {
        FanRule {
            control_id: C.into(),
            name: "Fan".into(),
            mode: FanMode::Curve,
            curve: Some("cpu".into()),
            step_up_per_sec: 100.0,
            critical_temp: 85.0,
            ..Default::default()
        }
    }
    fn temp(v: f64, t: i64) -> HashMap<String, Sensor> {
        map(vec![s(T, "Temperature", Some(v), t)])
    }
    fn decide(p: &mut Policy, r: &FanRule, m: &HashMap<String, Sensor>, now: i64) -> Decision {
        p.decide(r, &curves(), m, now).unwrap()
    }

    #[test]
    fn firmware_returns_none() {
        let mut p = Policy::default();
        assert_eq!(decide(&mut p, &FanRule { mode: FanMode::Firmware, ..rule() }, &HashMap::new(), 0).percent, None);
    }

    #[test]
    fn failsafes() {
        let mut p = Policy::default();
        assert_eq!(decide(&mut p, &rule(), &HashMap::new(), 0).percent, Some(100.0), "missing sensor");
        assert!(decide(&mut p, &rule(), &temp(40.0, 0), 10_000).emergency, "stale sensor");
        assert!(decide(&mut p, &rule(), &temp(40.0, 5_000), 0).emergency, "future timestamp");
        assert!(decide(&mut p, &rule(), &temp(f64::NAN, 0), 0).emergency, "NaN");
        assert!(decide(&mut p, &rule(), &temp(150.0, 0), 0).emergency, "implausible");
        let d = decide(&mut p, &rule(), &temp(90.0, 0), 0);
        assert!(d.emergency && d.percent == Some(100.0), "critical");
        let mut wrong = temp(40.0, 0);
        wrong.get_mut(T).unwrap().kind = "Load".into();
        assert!(decide(&mut p, &rule(), &wrong, 0).emergency, "wrong sensor kind");
        // A manual fan's extra failsafe sensor is watched too.
        let extra = "/gpu/temp";
        let m = FanRule { mode: FanMode::Manual, sensors: vec![extra.into()], ..rule() };
        assert!(decide(&mut p, &m, &temp(40.0, 0), 0).emergency, "extra failsafe sensor missing");
        assert!(p.decide(&rule(), &[], &temp(40.0, 0), 0).is_err(), "rule pointing at a missing curve is invalid");
    }

    #[test]
    fn manual_minimum_offset_and_stop() {
        let mut p = Policy::default();
        let m = FanRule { mode: FanMode::Manual, manual_percent: 20.0, minimum_percent: 30.0, start_percent: 0.0, sensors: vec![T.into()], ..rule() };
        assert_eq!(decide(&mut p, &m, &temp(40.0, 0), 0).percent, Some(30.0));
        assert!(decide(&mut p, &m, &temp(95.0, 0), 1000).emergency, "manual keeps thermal protection");
        let mut p = Policy::default();
        let o = FanRule { offset_percent: 10.0, start_percent: 0.0, ..rule() };
        assert_eq!(decide(&mut p, &o, &temp(35.0, 0), 0).percent, Some(50.0));
        let mut p = Policy::default();
        let stop = FanRule { allow_stop: true, minimum_percent: 45.0, start_percent: 60.0, ..rule() };
        assert_eq!(decide(&mut p, &stop, &temp(35.0, 0), 0).percent, Some(0.0), "below minimum stops the fan");
        assert_eq!(decide(&mut p, &stop, &temp(56.0, 1000), 1000).percent, Some(60.0), "restarts with the start speed");
    }

    #[test]
    fn start_boost_steps_and_smoothing() {
        let mut p = Policy::default();
        let r = FanRule { start_percent: 60.0, step_up_per_sec: 10.0, ..rule() };
        assert_eq!(decide(&mut p, &r, &temp(35.0, 0), 0).percent, Some(60.0), "start boost");
        assert_eq!(decide(&mut p, &r, &temp(34.0, 1000), 1000).percent, Some(60.0), "hold within hysteresis");
        assert_eq!(decide(&mut p, &r, &temp(30.0, 2000), 2000).percent, Some(55.0), "ramp down 5%/s");
        assert_eq!(decide(&mut p, &r, &temp(84.0, 3000), 3000).percent, Some(65.0), "step up 10%/s");
        let d = decide(&mut p, &r, &temp(86.0, 4000), 4000);
        assert_eq!(d.percent, Some(100.0), "emergencies are never rate limited");
    }

    #[test]
    fn rpm_feedback_and_stall() {
        let mut p = Policy::default();
        let r = FanRule { rpm_sensor_id: Some(R.into()), ..rule() };
        let mut m = temp(40.0, 0);
        assert!(decide(&mut p, &r, &m, 0).emergency, "missing rpm");
        m.insert(R.into(), s(R, "Fan", Some(50.0), 0));
        assert!(!decide(&mut p, &r, &m, 0).emergency);
        for t in [5_000, 10_000] {
            let mut m2 = m.clone();
            m2.values_mut().for_each(|x| x.updated_ms = t);
            assert!(!decide(&mut p, &r, &m2, t).emergency);
        }
        let mut m3 = m.clone();
        m3.values_mut().for_each(|x| x.updated_ms = 15_000);
        assert_eq!(decide(&mut p, &r, &m3, 15_000).reason, "Fan may be stalled — full cooling");
        m3.get_mut(R).unwrap().value = Some(900.0);
        assert!(!decide(&mut p, &r, &m3, 15_000).emergency, "recovered");
    }

    #[test]
    fn tuning_from_sweeps() {
        let cal = |v: &[(f64, f64)]| v.iter().map(|&(percent, rpm)| CalibrationPoint { percent, rpm }).collect::<Vec<_>>();
        // Fan #1 from this PC: stops below 20%.
        let cpu = cal(&[(10.0, 0.0), (20.0, 410.0), (30.0, 604.0), (40.0, 766.0), (50.0, 889.0), (60.0, 1011.0), (70.0, 1116.0), (80.0, 1216.0), (90.0, 1311.0), (100.0, 1378.0)]);
        let t = tune(&cpu, Some(25.0)).unwrap();
        assert_eq!((t.minimum, t.start), (28.0, 35.0));
        assert_eq!(t.efficient, 90.0);
        // Fan #2 (hub): never stops, ignores PWM below ~40%.
        let hub = cal(&[(0.0, 755.0), (10.0, 759.0), (20.0, 766.0), (30.0, 772.0), (40.0, 775.0), (50.0, 948.0), (60.0, 1105.0), (70.0, 1236.0), (80.0, 1369.0), (90.0, 1501.0), (100.0, 1619.0)]);
        let t = tune(&hub, None).unwrap();
        assert_eq!((t.minimum, t.start), (40.0, 40.0), "floor is where PWM starts to matter");
        assert!(tune(&cal(&[(0.0, 0.0), (100.0, 0.0)]), None).is_none(), "empty header");
    }

    // ---- ownership

    #[derive(Default)]
    struct Fake {
        ranges: HashMap<String, (f64, f64)>,
        speeds: HashMap<String, f64>,
        released: Vec<String>,
        fail_set: bool,
    }
    impl OutputBus for Fake {
        fn range(&self, id: &str) -> Option<(f64, f64)> {
            self.ranges.get(id).copied()
        }
        fn set(&mut self, id: &str, p: f64) -> Result<()> {
            if self.fail_set && p < 100.0 {
                bail!("write failed");
            }
            self.speeds.insert(id.into(), p);
            Ok(())
        }
        fn release(&mut self, id: &str) -> Result<()> {
            self.speeds.remove(id);
            self.released.push(id.into());
            Ok(())
        }
    }
    fn fake() -> Fake {
        Fake { ranges: [(C.to_string(), (0.0, 100.0))].into(), ..Default::default() }
    }
    fn apply(o: &mut Outputs, rules: &[FanRule], m: &HashMap<String, Sensor>, bus: &mut Fake, admin: bool, comp: Option<&str>) -> Result<HashMap<String, Decision>> {
        o.apply(rules, &curves(), m, bus, admin, comp, 0, &mut vec![])
    }

    #[test]
    fn outputs_write_and_release() {
        let mut o = Outputs::default();
        let mut bus = fake();
        apply(&mut o, &[rule()], &temp(40.0, 0), &mut bus, true, None).unwrap();
        assert!(bus.speeds.contains_key(C));
        apply(&mut o, &[FanRule { mode: FanMode::Firmware, ..rule() }], &temp(40.0, 0), &mut bus, true, None).unwrap();
        assert_eq!(bus.released, vec![C.to_string()]);
        apply(&mut o, &[rule()], &temp(40.0, 0), &mut bus, true, None).unwrap();
        apply(&mut o, &[], &temp(40.0, 0), &mut bus, true, None).unwrap();
        assert_eq!(bus.released.len(), 2, "removed rule releases");
    }

    #[test]
    fn outputs_refuse_competitors_and_non_admin() {
        let mut o = Outputs::default();
        let mut bus = fake();
        assert!(apply(&mut o, &[rule()], &temp(40.0, 0), &mut bus, false, None).is_err());
        apply(&mut o, &[rule()], &temp(40.0, 0), &mut bus, true, None).unwrap();
        assert!(apply(&mut o, &[rule()], &temp(40.0, 0), &mut bus, true, Some("Fan Control")).is_err());
        assert!(bus.speeds.is_empty(), "competitor appearing releases owned outputs");
        apply(&mut o, &[FanRule { mode: FanMode::Firmware, ..rule() }], &HashMap::new(), &mut bus, false, Some("X")).unwrap();
    }

    #[test]
    fn outputs_failure_drives_full_speed() {
        let mut o = Outputs::default();
        let mut bus = fake();
        bus.fail_set = true;
        assert!(apply(&mut o, &[rule()], &temp(40.0, 0), &mut bus, true, None).is_err());
        assert_eq!(bus.speeds.get(C), Some(&100.0), "claimed output forced to max after failed write");
        let mut bus = Fake::default();
        assert!(apply(&mut o, &[rule()], &temp(40.0, 0), &mut bus, true, None).is_err(), "missing output");
        let mut bus = fake();
        assert!(apply(&mut o, &[rule(), rule()], &temp(40.0, 0), &mut bus, true, None).is_err());
        assert!(!bus.speeds.contains_key(C) || bus.speeds[C] == 100.0, "no partial write before validation");
    }

    #[test]
    fn outputs_calibration_override() {
        let mut o = Outputs::default();
        let mut bus = fake();
        o.overrides.insert(C.into(), 20.0);
        apply(&mut o, &[FanRule { mode: FanMode::Firmware, ..rule() }], &temp(40.0, 0), &mut bus, true, None).unwrap();
        assert_eq!(bus.speeds.get(C), Some(&20.0), "calibration pins firmware fans too");
        o.overrides.clear();
        apply(&mut o, &[], &temp(40.0, 0), &mut bus, true, None).unwrap();
        assert!(bus.speeds.is_empty());
    }

    #[test]
    fn default_temperatures() {
        let mk = |id: &str, name: &str, hw: &str| Sensor { hardware: hw.into(), name: name.into(), ..s(id, "Temperature", Some(50.0), 0) };
        let probe = mk("/lpc/chip/temperature/0", "Temperature #1", "MB");
        let amd = mk("/amdcpu/0/temperature/0", "Core (Tctl/Tdie)", "CPU");
        let intel = mk("/intelcpu/0/temperature/0", "CPU Package", "CPU");
        let gpu = mk("/gpu-nvidia/0/temperature/0", "GPU Core", "GPU");
        let board = Sensor { kind: "Control".into(), ..mk("/lpc/chip/control/0", "Fan #1", "MB") };
        let gpu_out = Sensor { kind: "Control".into(), ..mk("/gpu-nvidia/0/control/0", "Fan", "GPU") };
        assert_eq!(cpu_temperature([&probe, &amd]), Some(&amd));
        assert_eq!(cpu_temperature([&probe, &intel]), Some(&intel));
        let all = vec![probe.clone(), amd.clone(), gpu.clone()];
        assert_eq!(temperature_for_output(&all, &board), Some(&amd));
        assert_eq!(temperature_for_output(&all, &gpu_out), Some(&gpu));
        assert_eq!(temperature_for_output(&[probe.clone(), gpu.clone()], &board), None, "no unrelated fallback");
        let other = Sensor { hardware: "Other".into(), ..gpu.clone() };
        assert_eq!(temperature_for_output(&[amd, other], &gpu_out), None, "no cross-GPU fallback");
        assert_eq!(gpu_temperature([&Sensor { name: "GPU Hot Spot".into(), ..gpu }], None), None);
        assert_eq!(paired_rpm("/lpc/c/0/control/3"), "/lpc/c/0/fan/3");
        let _ = CurvePoint { temp: 0.0, percent: 0.0 };
    }
}
