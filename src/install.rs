//! Self-installation: `stellar.exe` installs itself per user, registers the
//! elevated sign-in task, shortcuts and an Apps & Features entry, and can
//! remove all of it again. No PowerShell scripts or SDKs are needed.

use crate::platform;
use anyhow::{Context, Result, bail};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const NO_WINDOW: u32 = 0x0800_0000;
pub const TASK: &str = "Stellar";
const UNINSTALL_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Stellar";

pub fn install_dir() -> PathBuf {
    crate::config::local_app_data().join("Programs").join("Stellar")
}

fn same_path(a: &Path, b: &Path) -> bool {
    let n = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().to_lowercase();
    n(a) == n(b)
}

/// Whether this process is the installed copy.
pub fn is_installed_copy() -> bool {
    std::env::current_exe().ok().and_then(|e| e.parent().map(|p| same_path(p, &install_dir()))).unwrap_or(false)
}

pub fn is_installed() -> bool {
    install_dir().join("stellar.exe").exists() && task_exists()
}

fn task_exists() -> bool {
    Command::new("schtasks").args(["/Query", "/TN", TASK]).creation_flags(NO_WINDOW).output().is_ok_and(|o| o.status.success())
}

fn run(cmd: &mut Command) -> Result<()> {
    let out = cmd.creation_flags(NO_WINDOW).output().with_context(|| format!("{cmd:?}"))?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim().to_string() + &String::from_utf8_lossy(&out.stdout));
    }
    Ok(())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn current_user() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    match std::env::var("USERDOMAIN") {
        Ok(d) if !d.is_empty() => format!("{d}\\{user}"),
        _ => user,
    }
}

pub fn task_xml(exe: &Path, user: &str) -> String {
    let dir = exe.parent().map(|p| p.display().to_string()).unwrap_or_default();
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Stellar cooling and lighting (starts elevated at sign-in)</Description></RegistrationInfo>
  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId></LogonTrigger></Triggers>
  <Principals><Principal id="Author"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RestartOnFailure><Interval>PT1M</Interval><Count>3</Count></RestartOnFailure>
    <Priority>4</Priority>
  </Settings>
  <Actions Context="Author"><Exec><Command>{}</Command><Arguments>--tray</Arguments><WorkingDirectory>{}</WorkingDirectory></Exec></Actions>
</Task>
"#,
        xml_escape(&exe.display().to_string()),
        xml_escape(&dir),
        user = xml_escape(user),
    )
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let dest = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &dest)?;
        } else {
            std::fs::copy(e.path(), &dest).with_context(|| format!("copying {}", e.path().display()))?;
        }
    }
    Ok(())
}

fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn shortcut_dirs() -> Vec<PathBuf> {
    let mut v = vec![];
    if let Some(p) = std::env::var_os("APPDATA") {
        v.push(PathBuf::from(p).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    if let Ok(o) = Command::new("powershell").args(["-NoProfile", "-Command", "[Environment]::GetFolderPath('Desktop')"]).creation_flags(NO_WINDOW).output() {
        let d = String::from_utf8_lossy(&o.stdout).trim().to_string();
        if !d.is_empty() {
            v.push(PathBuf::from(d));
        }
    }
    v
}

/// Ask a running Stellar to quit (returning fans to firmware) and wait.
pub fn stop_running() -> Result<()> {
    if !platform::running() {
        return Ok(());
    }
    platform::signal(platform::QUIT_EVENT);
    for _ in 0..80 {
        if !platform::running() {
            std::thread::sleep(std::time::Duration::from_millis(500));
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    bail!("Stellar did not quit; close it from its tray icon and try again")
}

/// Install (or update) from the folder this exe runs in. Must be elevated.
pub fn install() -> Result<()> {
    let exe = std::env::current_exe()?;
    let src = exe.parent().context("exe folder")?.to_path_buf();
    let dest = install_dir();
    let hw = crate::hardware::host_path().and_then(|p| p.parent().map(Path::to_path_buf)).context("The hw folder (hardware host) must be next to stellar.exe")?;
    stop_running()?;
    std::fs::create_dir_all(&dest)?;
    if !same_path(&src, &dest) {
        std::fs::copy(&exe, dest.join("stellar.exe")).context("copying stellar.exe")?;
        let hw_dest = dest.join("hw");
        if hw_dest.exists() {
            std::fs::remove_dir_all(&hw_dest).context("replacing the hw folder")?;
        }
        copy_dir(&hw, &hw_dest)?;
    }
    let installed = dest.join("stellar.exe");

    // Elevated sign-in task: no prompts at startup, fans controlled before the desktop appears.
    let xml_path = std::env::temp_dir().join("stellar-task.xml");
    let utf16: Vec<u8> = [0xFFu8, 0xFE].into_iter().chain(task_xml(&installed, &current_user()).encode_utf16().flat_map(|u| u.to_le_bytes())).collect();
    std::fs::write(&xml_path, utf16)?;
    run(Command::new("schtasks").args(["/Create", "/TN", TASK, "/F", "/XML"]).arg(&xml_path)).context("registering the sign-in task")?;
    let _ = std::fs::remove_file(&xml_path);

    for dir in shortcut_dirs() {
        let lnk = dir.join("Stellar.lnk");
        let script = format!(
            "$s=(New-Object -ComObject WScript.Shell).CreateShortcut({});$s.TargetPath={};$s.WorkingDirectory={};$s.IconLocation={};$s.Description='Stellar cooling & lighting';$s.Save()",
            ps_quote(&lnk.display().to_string()),
            ps_quote(&installed.display().to_string()),
            ps_quote(&dest.display().to_string()),
            ps_quote(&format!("{},0", installed.display())),
        );
        let _ = run(Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script]));
    }

    let size_kb = dir_size(&dest) / 1024;
    let uninstall = format!("\"{}\" --uninstall", installed.display());
    for (name, kind, value) in [
        ("DisplayName", "REG_SZ", "Stellar".to_string()),
        ("DisplayVersion", "REG_SZ", env!("CARGO_PKG_VERSION").to_string()),
        ("Publisher", "REG_SZ", "Stellar".to_string()),
        ("DisplayIcon", "REG_SZ", installed.display().to_string()),
        ("InstallLocation", "REG_SZ", dest.display().to_string()),
        ("UninstallString", "REG_SZ", uninstall),
        ("URLInfoAbout", "REG_SZ", "https://github.com/p3n1cillin/stellar".to_string()),
        ("NoModify", "REG_DWORD", "1".to_string()),
        ("NoRepair", "REG_DWORD", "1".to_string()),
        ("EstimatedSize", "REG_DWORD", size_kb.to_string()),
    ] {
        run(Command::new("reg").args(["add", UNINSTALL_KEY, "/v", name, "/t", kind, "/d", &value, "/f"]))?;
    }
    crate::log(&format!("Installed {} to {}", env!("CARGO_PKG_VERSION"), dest.display()));
    Ok(())
}

pub fn start_task() -> bool {
    run(Command::new("schtasks").args(["/Run", "/TN", TASK])).is_ok()
}

fn dir_size(p: &Path) -> u64 {
    std::fs::read_dir(p)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            _ => e.metadata().map(|m| m.len()).unwrap_or(0),
        })
        .sum()
}

/// Remove the task, shortcuts, Apps entry and program files. Settings in
/// `Data` are kept unless `purge`. Must be elevated.
pub fn uninstall(purge: bool) -> Result<()> {
    stop_running()?;
    let _ = run(Command::new("schtasks").args(["/Delete", "/TN", TASK, "/F"]));
    for dir in shortcut_dirs() {
        let _ = std::fs::remove_file(dir.join("Stellar.lnk"));
    }
    let _ = run(Command::new("reg").args(["delete", UNINSTALL_KEY, "/f"]));
    let dest = install_dir();
    // This exe may be the installed one, so delete after it exits.
    let targets = if purge {
        format!("rmdir /s /q \"{}\"", dest.display())
    } else {
        format!("del /f /q \"{}\" & rmdir /s /q \"{}\"", dest.join("stellar.exe").display(), dest.join("hw").display())
    };
    Command::new("cmd").raw_arg(format!("/c ping 127.0.0.1 -n 3 >nul & {targets}")).creation_flags(NO_WINDOW).spawn()?;
    Ok(())
}

// ---------------------------------------------------------------- drivers & tools

/// LibreHardwareMonitor needs the PawnIO driver for motherboard fan headers
/// and CPU temperatures.
pub fn pawnio_installed() -> bool {
    Command::new("sc").args(["query", "PawnIO"]).creation_flags(NO_WINDOW).output().is_ok_and(|o| o.status.success())
}

pub fn openrgb_installed() -> bool {
    let pf = |v: &str| std::env::var_os(v).map(|p| Path::new(&p).join("OpenRGB").join("OpenRGB.exe"));
    std::env::var_os("OPENRGB_PATH").map(PathBuf::from).into_iter().chain(pf("ProgramFiles")).chain(pf("ProgramFiles(x86)")).any(|p| p.exists())
}

/// Install a winget package in a visible console so the user sees progress
/// and any license prompts. Returns when the window closes.
pub fn winget_install(id: &str) -> Result<()> {
    let status = Command::new("cmd")
        .raw_arg(format!(
            "/c winget install --id {id} --exact --accept-package-agreements --accept-source-agreements || (echo. & echo Install failed - see above. & pause)"
        ))
        .creation_flags(0x0000_0010) // CREATE_NEW_CONSOLE
        .status()
        .context("winget is not available on this PC")?;
    if !status.success() {
        bail!("winget install {id} failed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_xml_escapes_paths() {
        let xml = task_xml(Path::new(r"C:\Users\A & B\AppData\Local\Programs\Stellar\stellar.exe"), r"PC\A & B");
        assert!(xml.contains(r"<Command>C:\Users\A &amp; B\AppData\Local\Programs\Stellar\stellar.exe</Command>"));
        assert!(xml.contains(r"<UserId>PC\A &amp; B</UserId>"));
        assert!(xml.contains("<RunLevel>HighestAvailable</RunLevel>"));
        assert!(xml.contains("<Arguments>--tray</Arguments>"));
    }

    /// Writes the real task XML for this PC when STELLAR_DUMP_TASK_XML is set,
    /// so it can be checked with Task Scheduler's validate-only mode.
    #[test]
    fn dump_task_xml() {
        if let Some(p) = std::env::var_os("STELLAR_DUMP_TASK_XML") {
            std::fs::write(p, task_xml(&install_dir().join("stellar.exe"), &current_user())).unwrap();
        }
    }

    #[test]
    fn powershell_quotes() {
        assert_eq!(ps_quote("it's"), "'it''s'");
    }
}
