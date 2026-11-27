//! Small Win32 helpers: single instance, cross-process show/quit signals and
//! window visibility that works while the UI loop is idle.

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND};
use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, SetEvent, WaitForSingleObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IsIconic, SW_HIDE, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindow,
};

pub const SHOW_EVENT: &str = "Local\\Stellar.Show";
pub const QUIT_EVENT: &str = "Local\\Stellar.Quit";
pub const AUTOTUNE_EVENT: &str = "Local\\Stellar.AutoTune";

/// Security for Stellar's named objects. Stellar runs elevated, but the
/// desktop shortcut and `stellar --quit` run at normal integrity and must be
/// able to signal it: allow authenticated users, label medium integrity.
fn shared_attributes() -> windows_sys::Win32::Security::SECURITY_ATTRIBUTES {
    use windows_sys::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
    let mut sd = std::ptr::null_mut();
    let sddl = wide("D:(A;;GA;;;AU)(A;;GA;;;SY)(A;;GA;;;BA)S:(ML;;NW;;;ME)");
    unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, std::ptr::null_mut()) };
    // Leaked deliberately: a handful of process-lifetime objects use it.
    windows_sys::Win32::Security::SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<windows_sys::Win32::Security::SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd,
        bInheritHandle: 0,
    }
}

pub fn is_admin() -> bool {
    unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0 }
}

/// Whether a Stellar instance is running (its show event exists).
pub fn running() -> bool {
    unsafe {
        let h = OpenEventW(EVENT_MODIFY_STATE, 0, wide(SHOW_EVENT).as_ptr());
        if h.is_null() {
            return false;
        }
        CloseHandle(h);
        true
    }
}

/// Start Stellar elevated: through its scheduled task when installed (no
/// prompt), otherwise with a single Windows elevation prompt.
pub fn relaunch_elevated(args: &[String], use_task: bool) -> bool {
    use std::os::windows::process::CommandExt;
    let task = use_task && std::process::Command::new("schtasks")
        .args(["/Run", "/TN", "Stellar"])
        .creation_flags(0x0800_0000)
        .output()
        .is_ok_and(|o| o.status.success());
    if task {
        return true;
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    let params = args.iter().map(|a| format!("\"{a}\"")).collect::<Vec<_>>().join(" ");
    let r = unsafe {
        windows_sys::Win32::UI::Shell::ShellExecuteW(
            std::ptr::null_mut(),
            wide("runas").as_ptr(),
            wide(&exe.to_string_lossy()).as_ptr(),
            wide(&params).as_ptr(),
            std::ptr::null(),
            SW_SHOW,
        )
    };
    r as usize > 32
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub struct Instance(HANDLE);

impl Instance {
    pub fn acquire() -> Option<Self> {
        unsafe {
            let sa = shared_attributes();
            let h = CreateMutexW(&sa, 0, wide("Local\\Stellar.Instance").as_ptr());
            if h.is_null() {
                return None;
            }
            if GetLastError() == ERROR_ALREADY_EXISTS {
                CloseHandle(h);
                return None;
            }
            Some(Self(h))
        }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

pub fn signal(name: &str) {
    unsafe {
        let h = OpenEventW(EVENT_MODIFY_STATE, 0, wide(name).as_ptr());
        if !h.is_null() {
            SetEvent(h);
            CloseHandle(h);
        }
    }
}

/// Run `f` on a background thread every time the named event is signalled.
pub fn watch(name: &str, f: impl Fn() + Send + 'static) {
    let sa = shared_attributes();
    let h = unsafe { CreateEventW(&sa, 0, 0, wide(name).as_ptr()) } as usize;
    if h == 0 {
        return;
    }
    std::thread::spawn(move || loop {
        if unsafe { WaitForSingleObject(h as HANDLE, INFINITE) } != 0 {
            return;
        }
        f();
    });
}

/// A simple Windows message box; returns the button id (IDYES, IDNO, ...).
pub fn message(text: &str, flags: u32) -> i32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_SETFOREGROUND, MessageBoxW};
    unsafe { MessageBoxW(std::ptr::null_mut(), wide(text).as_ptr(), wide("Stellar").as_ptr(), flags | MB_SETFOREGROUND) }
}

pub fn attach_console() {
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

#[derive(Clone, Copy)]
pub struct Window(pub isize);

impl Window {
    fn h(self) -> HWND {
        self.0 as HWND
    }
    pub fn show(self) {
        unsafe {
            if IsIconic(self.h()) != 0 {
                ShowWindow(self.h(), SW_RESTORE);
            } else {
                ShowWindow(self.h(), SW_SHOW);
            }
            SetForegroundWindow(self.h());
        }
    }
    pub fn hide(self) {
        unsafe { ShowWindow(self.h(), SW_HIDE) };
    }
}
