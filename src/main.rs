#![cfg_attr(not(test), windows_subsystem = "windows")]
//! Stellar — one place for all PC cooling and lighting.

mod config;
mod cooling;
mod curves;
mod engine;
mod hardware;
mod install;
mod lighting;
mod openrgb;
mod platform;
mod tray;
mod tuning;
mod ui;

use std::io::Write;
use std::sync::{Arc, Mutex};

pub fn log(msg: &str) {
    let dir = config::Store::default_location().dir;
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("stellar.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1 << 20) {
        let _ = std::fs::rename(&path, dir.join("stellar.log.old"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} {msg}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    }
}

const HELP: &str = "Stellar — cooling & lighting control centre

  stellar            open the dashboard (or raise the running instance)
  stellar --tray     start quietly in the system tray
  stellar --quit     ask the running instance to return fans to firmware and exit
  stellar --autotune measure every fan and apply tuned curves (running instance)
  stellar --install  install or update from this folder (asks for elevation)
  stellar --uninstall remove Stellar (settings are kept)
  stellar --portable run this copy without installing
  stellar status     print a one-off hardware reading as JSON
  stellar rgb        list OpenRGB devices
  stellar --help";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |a: &str| args.iter().any(|x| x == a);
    if has("--help") || has("-h") || has("status") || has("rgb") {
        platform::attach_console();
    }
    if has("--help") || has("-h") {
        println!("{HELP}");
        return;
    }
    if has("--quit") {
        platform::signal(platform::QUIT_EVENT);
        return;
    }
    if has("--autotune") {
        if platform::running() {
            platform::signal(platform::AUTOTUNE_EVENT);
            return;
        }
        platform::attach_console();
        eprintln!("Stellar is not running");
        std::process::exit(1);
    }
    if has("status") {
        std::process::exit(match cli_status() {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{e:#}");
                1
            }
        });
    }
    if has("rgb") {
        std::process::exit(match cli_rgb() {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{e:#}");
                1
            }
        });
    }

    if has("--install") || has("--uninstall") {
        if !platform::is_admin() {
            platform::relaunch_elevated(&args, false);
            return;
        }
        if has("--install") {
            run_install();
        } else {
            run_uninstall(has("--purge"));
        }
        return;
    }

    // Stellar always runs elevated: fan headers and CPU sensors need it. A
    // normal launch (desktop shortcut) raises the running instance or starts
    // the elevated one, never a limited dashboard.
    if !platform::is_admin() && std::env::var_os("STELLAR_SNAPSHOT").is_none() && !has("--no-elevate") {
        if platform::running() {
            platform::signal(platform::SHOW_EVENT);
            return;
        }
        let mut args = args.clone();
        let mut use_task = install::is_installed_copy();
        if !use_task && !has("--portable") {
            use windows_sys::Win32::UI::WindowsAndMessaging::{IDNO, IDYES, MB_ICONQUESTION, MB_YESNOCANCEL};
            let text = if install::is_installed() {
                "Stellar is already installed.

Yes: update the installed copy with this version
No: just run this copy
Cancel: do nothing"
            } else {
                "Install Stellar?

It installs for your account, starts with Windows (elevated, without a prompt each time) and appears in Apps & Features.

Yes: install
No: just run this copy without installing
Cancel: do nothing"
            };
            match platform::message(text, MB_YESNOCANCEL | MB_ICONQUESTION) {
                IDYES => {
                    platform::relaunch_elevated(&["--install".to_string()], false);
                    return;
                }
                IDNO => args.push("--portable".into()),
                _ => return,
            }
        }
        if has("--portable") {
            use_task = false;
        }
        if platform::relaunch_elevated(&args, use_task) && !has("--tray") {
            for _ in 0..40 {
                std::thread::sleep(std::time::Duration::from_millis(250));
                if platform::running() {
                    platform::signal(platform::SHOW_EVENT);
                    break;
                }
            }
        }
        return;
    }
    let Some(_instance) = platform::Instance::acquire() else {
        platform::signal(platform::SHOW_EVENT);
        return;
    };
    log(&format!("Launch {args:?}"));
    let state: engine::State = Arc::new(Mutex::new(engine::Shared::default()));
    let (tx, rx) = engine::channel();
    let repaint: Arc<Mutex<Option<eframe::egui::Context>>> = Arc::new(Mutex::new(None));
    let r = repaint.clone();
    let engine = engine::spawn(state.clone(), rx, Box::new(move || {
        if let Some(c) = r.lock().unwrap().as_ref() {
            c.request_repaint();
        }
    }));
    {
        let tx = tx.clone();
        platform::watch(platform::AUTOTUNE_EVENT, move || {
            let _ = tx.send(engine::Cmd::AutoTune(None));
        });
    }
    let result = ui::run(state.clone(), tx.clone(), repaint, has("--tray"));
    if let Err(e) = result {
        log(&format!("UI error: {e}"));
    }
    let _ = tx.send(engine::Cmd::Quit);
    let _ = engine.join();
}

fn run_install() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IDYES, MB_ICONERROR, MB_ICONQUESTION, MB_YESNO};
    if !install::pawnio_installed()
        && platform::message(
            "Stellar needs the free PawnIO driver to read CPU temperatures and control motherboard fan headers.

Install PawnIO now with winget? (A console window shows progress.)",
            MB_YESNO | MB_ICONQUESTION,
        ) == IDYES
        && let Err(e) = install::winget_install("namazso.PawnIO")
    {
        platform::message(&format!("PawnIO could not be installed automatically ({e}).

Get it from https://pawnio.eu and restart Stellar. Stellar will still install."), MB_ICONERROR);
    }
    match install::install() {
        Ok(()) => {
            install::start_task();
            for _ in 0..40 {
                std::thread::sleep(std::time::Duration::from_millis(250));
                if platform::running() {
                    platform::signal(platform::SHOW_EVENT);
                    break;
                }
            }
        }
        Err(e) => {
            log(&format!("Install failed: {e:#}"));
            platform::message(&format!("Stellar could not be installed:

{e:#}"), MB_ICONERROR);
        }
    }
}

fn run_uninstall(purge: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IDOK, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONQUESTION, MB_OKCANCEL};
    let text = "Uninstall Stellar?

Fans return to motherboard/graphics card control and lighting stops updating. Your settings are kept in %LOCALAPPDATA%\\Programs\\Stellar\\Data.";
    if platform::message(text, MB_OKCANCEL | MB_ICONQUESTION) != IDOK {
        return;
    }
    match install::uninstall(purge) {
        Ok(()) => {
            platform::message("Stellar has been removed.", MB_ICONINFORMATION);
        }
        Err(e) => {
            platform::message(&format!("Stellar could not be removed:

{e:#}"), MB_ICONERROR);
        }
    }
}

fn cli_status() -> anyhow::Result<()> {
    let mut host = hardware::Host::new();
    host.read()?;
    std::thread::sleep(std::time::Duration::from_millis(1200));
    let s = host.read()?;
    let sensors: Vec<_> = s.sensors.iter().map(|x| serde_json::json!({"id": x.id, "hardware": x.hardware, "name": x.name, "kind": x.kind, "value": x.value})).collect();
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({"admin": s.admin, "controls": s.controls.keys().collect::<Vec<_>>(), "errors": s.errors, "sensors": sensors}))?);
    host.shutdown();
    Ok(())
}

fn cli_rgb() -> anyhow::Result<()> {
    let mut c = openrgb::Client::connect("127.0.0.1:6742", "Stellar probe")?;
    for d in c.devices()? {
        println!("{} ({}) — {} LEDs", d.name, d.vendor, d.led_count);
        for z in d.zones {
            println!("    {}: {} LEDs (range {}–{})", z.name, z.leds_count, z.leds_min, z.leds_max);
        }
    }
    Ok(())
}
