//! Client for `stellar-hw`, the LibreHardwareMonitor host process.
//!
//! The host returns every output it touched to firmware control when this
//! process exits, crashes (stdin closes) or stops talking for 10 seconds.

use crate::cooling::{OutputBus, Sensor};
use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

#[derive(Deserialize, Clone, Copy, Debug)]
pub struct ControlRange {
    pub min: f64,
    pub max: f64,
}

#[derive(Deserialize, Default, Clone, Debug)]
pub struct Snapshot {
    pub admin: bool,
    pub sensors: Vec<Sensor>,
    pub controls: HashMap<String, ControlRange>,
    #[serde(default)]
    pub errors: Vec<String>,
}

pub struct Host {
    child: Option<(Child, ChildStdin, BufReader<ChildStdout>)>,
    controls: HashMap<String, ControlRange>,
}

pub fn host_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("STELLAR_HW").map(PathBuf::from).filter(|p| p.exists()) {
        return Some(p);
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [dir.join("hw").join("stellar-hw.exe"), dir.join("hwhost").join("stellar-hw.exe"), dir.join("../hwhost/stellar-hw.exe")]
        .into_iter()
        .find(|p| p.exists())
}

impl Host {
    pub fn new() -> Self {
        Self { child: None, controls: HashMap::new() }
    }

    fn ensure(&mut self) -> Result<()> {
        if let Some((c, _, _)) = &mut self.child
            && c.try_wait().ok().flatten().is_none()
        {
            return Ok(());
        }
        self.child = None;
        let path = host_path().ok_or_else(|| anyhow!("Hardware host (hw\\stellar-hw.exe) is missing next to Stellar"))?;
        let mut cmd = Command::new(&path);
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd.spawn().with_context(|| format!("Starting {}", path.display()))?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = BufReader::new(child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?);
        self.child = Some((child, stdin, stdout));
        Ok(())
    }

    fn call(&mut self, req: Value) -> Result<Value> {
        self.ensure()?;
        let (_, stdin, stdout) = self.child.as_mut().expect("ensured");
        let r = (|| -> Result<Value> {
            writeln!(stdin, "{req}")?;
            stdin.flush()?;
            let mut line = String::new();
            if stdout.read_line(&mut line)? == 0 {
                bail!("Hardware host exited");
            }
            let v: Value = serde_json::from_str(&line)?;
            if v["ok"] != Value::Bool(true) {
                bail!("{}", v["error"].as_str().unwrap_or("hardware host error"));
            }
            Ok(v)
        })();
        if let Err(e) = &r
            && (e.to_string().contains("exited") || e.downcast_ref::<std::io::Error>().is_some())
        {
            self.kill();
        }
        r
    }

    pub fn read(&mut self) -> Result<Snapshot> {
        let v = self.call(json!({"cmd": "read"}))?;
        let snap: Snapshot = serde_json::from_value(v)?;
        self.controls = snap.controls.clone();
        Ok(snap)
    }

    pub fn shutdown(&mut self) {
        if self.child.is_some() {
            let _ = self.call(json!({"cmd": "quit"}));
        }
        if let Some((mut c, stdin, _)) = self.child.take() {
            drop(stdin);
            for _ in 0..30 {
                if c.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            let _ = c.kill();
        }
    }

    fn kill(&mut self) {
        // Closing stdin makes the host release outputs before exiting.
        if let Some((mut c, stdin, _)) = self.child.take() {
            drop(stdin);
            std::thread::sleep(std::time::Duration::from_millis(500));
            let _ = c.kill();
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl OutputBus for Host {
    fn range(&self, id: &str) -> Option<(f64, f64)> {
        self.controls.get(id).map(|c| (c.min, c.max))
    }
    fn set(&mut self, id: &str, percent: f64) -> Result<()> {
        self.call(json!({"cmd": "set", "id": id, "percent": percent})).map(|_| ())
    }
    fn release(&mut self, id: &str) -> Result<()> {
        self.call(json!({"cmd": "release", "id": id})).map(|_| ())
    }
}

/// Lower-case names of running programs, without `.exe`.
pub fn running_processes() -> std::collections::HashSet<String> {
    let Ok(mut sys) = sysinfo::System::new() else { return Default::default() };
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    sys.processes().values().map(|p| p.name().to_string_lossy().to_lowercase().trim_end_matches(".exe").to_string()).collect()
}

/// Another program that writes fan duty cycles, if one is running.
pub fn competing_writer(procs: &std::collections::HashSet<String>) -> Option<&'static str> {
    let running = |n: &str| procs.contains(&n.to_lowercase());
    if running("FanControl") {
        Some("Fan Control")
    } else if running("CoolingLighting") {
        Some("Cooling & Lighting")
    } else {
        None
    }
}
