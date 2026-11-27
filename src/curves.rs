//! Reusable fan curves, modelled on Fan Control: several fans can follow one
//! curve, and Mix curves combine others (e.g. case fans follow the hotter of
//! the CPU and GPU curves).

use crate::config::{CurvePoint, Mix};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MixFn {
    #[default]
    Max,
    Min,
    Average,
    Sum,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "type")]
pub enum CurveKind {
    /// Temperature → speed graph over one or more sensors.
    Graph { sensors: Vec<String>, mix: Mix, offset: f64, points: Vec<CurvePoint> },
    /// Straight line between two temperatures.
    Linear { sensor: String, low_temp: f64, high_temp: f64, low_percent: f64, high_percent: f64 },
    /// Constant speed.
    Flat { percent: f64 },
    /// Idle speed until `load_temp`, load speed until it falls back to `idle_temp`.
    Trigger { sensor: String, idle_temp: f64, load_temp: f64, idle_percent: f64, load_percent: f64 },
    /// Combination of other curves.
    Mix { curves: Vec<String>, function: MixFn },
}

impl CurveKind {
    pub fn label(&self) -> &'static str {
        match self {
            CurveKind::Graph { .. } => "Graph",
            CurveKind::Linear { .. } => "Linear",
            CurveKind::Flat { .. } => "Flat",
            CurveKind::Trigger { .. } => "Trigger",
            CurveKind::Mix { .. } => "Mix",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct CurveDef {
    pub id: String,
    pub name: String,
    pub kind: CurveKind,
    /// Response time in seconds: the input temperature is smoothed so fans
    /// follow the trend, not spikes (ideal for AIO radiators, where coolant
    /// lags the CPU). Never applied to the critical-temperature failsafe.
    #[serde(default)]
    pub response_s: f64,
}

fn finite_in(v: f64, lo: f64, hi: f64) -> bool {
    v.is_finite() && v >= lo && v <= hi
}

pub fn validate_points(points: &[CurvePoint]) -> Result<()> {
    if points.len() < 2 || points.iter().any(|p| !finite_in(p.percent, 0.0, 100.0) || !finite_in(p.temp, 0.0, 110.0)) {
        bail!("Invalid curve points");
    }
    for w in points.windows(2) {
        if w[1].temp <= w[0].temp || w[1].percent < w[0].percent {
            bail!("Curve temperatures must increase and fan speeds cannot decrease");
        }
    }
    Ok(())
}

impl CurveDef {
    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() || self.name.trim().is_empty() {
            bail!("Curves need a name");
        }
        if !finite_in(self.response_s, 0.0, 60.0) {
            bail!("Response time must be 0–60 s");
        }
        let pct = |v: f64| finite_in(v, 0.0, 100.0);
        let temp = |v: f64| finite_in(v, 0.0, 110.0);
        match &self.kind {
            CurveKind::Graph { sensors, offset, points, .. } => {
                if sensors.is_empty() || sensors.iter().any(|s| s.trim().is_empty()) {
                    bail!("Choose a temperature sensor");
                }
                if !finite_in(*offset, -20.0, 20.0) {
                    bail!("Temperature offset must be within ±20°C");
                }
                validate_points(points)?;
            }
            CurveKind::Linear { sensor, low_temp, high_temp, low_percent, high_percent } => {
                if sensor.trim().is_empty() || !temp(*low_temp) || !temp(*high_temp) || high_temp <= low_temp
                    || !pct(*low_percent) || !pct(*high_percent) || high_percent < low_percent
                {
                    bail!("Linear curves need a sensor, low < high temperature and a rising speed");
                }
            }
            CurveKind::Flat { percent } => {
                if !pct(*percent) {
                    bail!("Speeds must be 0–100%");
                }
            }
            CurveKind::Trigger { sensor, idle_temp, load_temp, idle_percent, load_percent } => {
                if sensor.trim().is_empty() || !temp(*idle_temp) || !temp(*load_temp) || load_temp <= idle_temp
                    || !pct(*idle_percent) || !pct(*load_percent) || load_percent < idle_percent
                {
                    bail!("Trigger curves need a sensor, idle < load temperature and load ≥ idle speed");
                }
            }
            CurveKind::Mix { curves, .. } => {
                if curves.is_empty() {
                    bail!("A mix curve needs at least one curve");
                }
            }
        }
        Ok(())
    }
}

pub fn validate_curves(curves: &[CurveDef]) -> Result<()> {
    let mut ids = std::collections::HashSet::new();
    for c in curves {
        c.validate().with_context(|| format!("Curve '{}'", c.name))?;
        if !ids.insert(c.id.as_str()) {
            bail!("Duplicate curve ids");
        }
    }
    for c in curves {
        if let CurveKind::Mix { curves: refs, .. } = &c.kind
            && refs.iter().any(|r| !ids.contains(r.as_str()))
        {
            bail!("Curve '{}' mixes a curve that no longer exists", c.name);
        }
    }
    fn cyclic(curves: &[CurveDef], id: &str, stack: &mut Vec<String>) -> bool {
        if stack.iter().any(|s| s == id) {
            return true;
        }
        let Some(c) = curves.iter().find(|c| c.id == id) else { return false };
        if let CurveKind::Mix { curves: refs, .. } = &c.kind {
            stack.push(id.to_string());
            let r = refs.iter().any(|r| cyclic(curves, r, stack));
            stack.pop();
            return r;
        }
        false
    }
    for c in curves {
        if cyclic(curves, &c.id, &mut vec![]) {
            bail!("Curve '{}' mixes itself", c.name);
        }
    }
    Ok(())
}

/// Every temperature sensor a curve depends on (through mixes too).
pub fn curve_sensors(curves: &[CurveDef], id: &str) -> Vec<String> {
    fn walk(curves: &[CurveDef], id: &str, depth: usize, out: &mut Vec<String>) {
        let Some(c) = curves.iter().find(|c| c.id == id) else { return };
        if depth > 16 {
            return;
        }
        match &c.kind {
            CurveKind::Graph { sensors, .. } => out.extend(sensors.iter().cloned()),
            CurveKind::Linear { sensor, .. } | CurveKind::Trigger { sensor, .. } => out.push(sensor.clone()),
            CurveKind::Flat { .. } => {}
            CurveKind::Mix { curves: ids, .. } => ids.iter().for_each(|i| walk(curves, i, depth + 1, out)),
        }
    }
    let mut out = vec![];
    walk(curves, id, 0, &mut out);
    out.sort();
    out.dedup();
    out
}

pub fn new_id() -> String {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("c{:x}", n & 0xffff_ffff_ffff)
}

pub fn interpolate(points: &[CurvePoint], t: f64) -> f64 {
    if t <= points[0].temp {
        return points[0].percent;
    }
    for w in points.windows(2) {
        if t <= w[1].temp {
            return w[0].percent + (w[1].percent - w[0].percent) * (t - w[0].temp) / (w[1].temp - w[0].temp);
        }
    }
    points[points.len() - 1].percent
}

/// Evaluates curves. Trigger curves keep their idle/load state between calls.
#[derive(Default)]
pub struct Evaluator {
    triggered: HashMap<String, bool>,
    /// Smoothed input per curve: (value, time in ms).
    smoothed: HashMap<String, (f64, i64)>,
    now: Option<i64>,
}

impl Evaluator {
    /// `temps` holds validated, fresh readings. Returns None if a curve or
    /// reading is missing, which callers treat as a failsafe condition.
    pub fn eval(&mut self, curves: &[CurveDef], id: &str, temps: &HashMap<String, f64>) -> Option<f64> {
        self.now = None;
        self.eval_depth(curves, id, temps, 0)
    }

    /// Like `eval`, applying each curve's response time (smoothing) at `now_ms`.
    pub fn eval_at(&mut self, curves: &[CurveDef], id: &str, temps: &HashMap<String, f64>, now_ms: i64) -> Option<f64> {
        self.now = Some(now_ms);
        self.eval_depth(curves, id, temps, 0)
    }

    /// Exponential smoothing with time constant `response_s`. Evaluating a
    /// curve several times in one tick (shared curves, mixes) updates it once.
    fn smooth(&mut self, c: &CurveDef, raw: f64) -> f64 {
        let Some(now) = self.now else { return raw };
        if c.response_s <= 0.0 {
            return raw;
        }
        let e = self.smoothed.entry(c.id.clone()).or_insert((raw, now));
        let dt = (now - e.1) as f64 / 1000.0;
        if dt > 0.0 {
            // Rises are followed faster than falls: never lag far behind heat.
            let tau = if raw > e.0 { c.response_s / 2.0 } else { c.response_s };
            let k = if dt > 30.0 { 1.0 } else { 1.0 - (-dt / tau).exp() };
            e.0 += (raw - e.0) * k;
            e.1 = now;
        }
        e.0
    }

    fn eval_depth(&mut self, curves: &[CurveDef], id: &str, temps: &HashMap<String, f64>, depth: usize) -> Option<f64> {
        if depth > 16 {
            return None;
        }
        let c = curves.iter().find(|c| c.id == id)?;
        let v = match &c.kind {
            CurveKind::Graph { sensors, mix, offset, points } => {
                let vals: Option<Vec<f64>> = sensors.iter().map(|s| temps.get(s).copied()).collect();
                let vals = vals?;
                let t = match mix {
                    Mix::Max => vals.iter().copied().fold(f64::MIN, f64::max),
                    Mix::Average => vals.iter().sum::<f64>() / vals.len() as f64,
                };
                let t = self.smooth(c, t);
                interpolate(points, t + offset)
            }
            CurveKind::Linear { sensor, low_temp, high_temp, low_percent, high_percent } => {
                let t = self.smooth(c, *temps.get(sensor)?);
                let k = ((t - low_temp) / (high_temp - low_temp)).clamp(0.0, 1.0);
                low_percent + (high_percent - low_percent) * k
            }
            CurveKind::Flat { percent } => *percent,
            CurveKind::Trigger { sensor, idle_temp, load_temp, idle_percent, load_percent } => {
                let t = self.smooth(c, *temps.get(sensor)?);
                let on = self.triggered.entry(c.id.clone()).or_insert(false);
                if t >= *load_temp {
                    *on = true;
                } else if t <= *idle_temp {
                    *on = false;
                }
                if *on { *load_percent } else { *idle_percent }
            }
            CurveKind::Mix { curves: refs, function } => {
                let vals: Option<Vec<f64>> = refs.iter().map(|r| self.eval_depth(curves, r, temps, depth + 1)).collect();
                let vals = vals?;
                match function {
                    MixFn::Max => vals.iter().copied().fold(f64::MIN, f64::max),
                    MixFn::Min => vals.iter().copied().fold(f64::MAX, f64::min),
                    MixFn::Average => vals.iter().sum::<f64>() / vals.len() as f64,
                    MixFn::Sum => vals.iter().sum(),
                }
            }
        };
        Some(v.clamp(0.0, 100.0))
    }

    pub fn reset(&mut self) {
        self.triggered.clear();
        self.smoothed.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(id: &str, sensor: &str) -> CurveDef {
        CurveDef { response_s: 0.0,
            id: id.into(),
            name: id.into(),
            kind: CurveKind::Graph {
                sensors: vec![sensor.into()],
                mix: Mix::Max,
                offset: 0.0,
                points: vec![CurvePoint { temp: 40.0, percent: 20.0 }, CurvePoint { temp: 80.0, percent: 100.0 }],
            },
        }
    }

    #[test]
    fn kinds_evaluate() {
        let mut e = Evaluator::default();
        let temps: HashMap<String, f64> = [("cpu".to_string(), 60.0), ("gpu".to_string(), 70.0)].into();
        let cs = vec![
            graph("a", "cpu"),
            graph("b", "gpu"),
            CurveDef { response_s: 0.0, id: "m".into(), name: "m".into(), kind: CurveKind::Mix { curves: vec!["a".into(), "b".into()], function: MixFn::Max } },
            CurveDef { response_s: 0.0, id: "f".into(), name: "f".into(), kind: CurveKind::Flat { percent: 33.0 } },
            CurveDef { response_s: 0.0,
                id: "l".into(),
                name: "l".into(),
                kind: CurveKind::Linear { sensor: "cpu".into(), low_temp: 50.0, high_temp: 70.0, low_percent: 0.0, high_percent: 100.0 },
            },
        ];
        validate_curves(&cs).unwrap();
        assert_eq!(e.eval(&cs, "a", &temps), Some(60.0));
        assert_eq!(e.eval(&cs, "m", &temps), Some(80.0));
        assert_eq!(e.eval(&cs, "f", &temps), Some(33.0));
        assert_eq!(e.eval(&cs, "l", &temps), Some(50.0));
        assert_eq!(e.eval(&cs, "missing", &temps), None);
        assert_eq!(e.eval(&cs, "a", &HashMap::new()), None, "missing reading");
        assert_eq!(curve_sensors(&cs, "m"), vec!["cpu", "gpu"]);
    }

    #[test]
    fn trigger_has_hysteresis() {
        let mut e = Evaluator::default();
        let cs = vec![CurveDef { response_s: 0.0,
            id: "t".into(),
            name: "t".into(),
            kind: CurveKind::Trigger { sensor: "s".into(), idle_temp: 50.0, load_temp: 70.0, idle_percent: 30.0, load_percent: 80.0 },
        }];
        let at = |v: f64| -> HashMap<String, f64> { [("s".to_string(), v)].into() };
        assert_eq!(e.eval(&cs, "t", &at(60.0)), Some(30.0));
        assert_eq!(e.eval(&cs, "t", &at(71.0)), Some(80.0));
        assert_eq!(e.eval(&cs, "t", &at(60.0)), Some(80.0), "stays loaded until idle temperature");
        assert_eq!(e.eval(&cs, "t", &at(50.0)), Some(30.0));
    }

    #[test]
    fn response_time_smooths_spikes() {
        let mut c = graph("a", "cpu");
        c.response_s = 10.0;
        let cs = vec![c];
        let at = |v: f64| -> HashMap<String, f64> { [("cpu".to_string(), v)].into() };
        let mut e = Evaluator::default();
        assert_eq!(e.eval_at(&cs, "a", &at(40.0), 0), Some(20.0));
        let spike = e.eval_at(&cs, "a", &at(80.0), 1000).unwrap();
        assert!(spike > 20.0 && spike < 40.0, "a 1 s spike moves the fan only a little: {spike}");
        assert_eq!(e.eval_at(&cs, "a", &at(80.0), 1000).unwrap(), spike, "same tick evaluates once");
        let held = e.eval_at(&cs, "a", &at(80.0), 21_000).unwrap();
        assert!(held > 97.0, "sustained heat is followed fully: {held}");
        assert_eq!(Evaluator::default().eval(&cs, "a", &at(80.0)), Some(100.0), "display evaluation is unsmoothed");
    }

    #[test]
    fn rejects_cycles_and_dangling_mixes() {
        let mix = |id: &str, r: &str| CurveDef { response_s: 0.0, id: id.into(), name: id.into(), kind: CurveKind::Mix { curves: vec![r.into()], function: MixFn::Max } };
        assert!(validate_curves(&[mix("a", "b"), mix("b", "a")]).is_err());
        assert!(validate_curves(&[mix("a", "zzz")]).is_err());
        assert!(validate_curves(&[graph("a", "x"), graph("a", "y")]).is_err());
    }
}
