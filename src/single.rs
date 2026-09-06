//! Nur eine Instanz je Port. Ein zweiter Start holt das Fenster der ersten nach
//! vorn und übergibt ihr, falls vorhanden, einen Einladungslink über eine Datei.

use std::path::PathBuf;
use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// true = wir sind die erste Instanz. Der Mutex bleibt bis zum Prozessende bestehen.
pub fn acquire(port: u16) -> bool {
    let name = wide(&format!("Local\\Holler-{port}"));
    let h = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if h.is_null() {
        return true;
    }
    let exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    // Handle absichtlich nicht schliessen: es markiert die laufende Instanz.
    !exists
}

/// Fenster der laufenden Instanz nach vorn holen.
pub fn show_existing() {
    for title in ["Holler", "Holler — STUMM"] {
        let t = wide(title);
        let hwnd = unsafe { FindWindowW(std::ptr::null(), t.as_ptr()) };
        if !hwnd.is_null() {
            unsafe {
                ShowWindow(hwnd, SW_SHOW);
                ShowWindow(hwnd, SW_RESTORE);
                SetForegroundWindow(hwnd);
            }
            return;
        }
    }
}

/// Übergabedatei für Einladungslinks an die laufende Instanz.
pub fn join_file() -> Option<PathBuf> {
    crate::config::default_path().map(|p| p.with_file_name("join.txt"))
}

pub fn hand_over_invite(url: &str) {
    if let Some(p) = join_file() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, url);
    }
}

/// Von der laufenden Instanz gepollt: Link abholen und Datei entfernen.
pub fn take_invite() -> Option<String> {
    let p = join_file()?;
    let s = std::fs::read_to_string(&p).ok()?;
    let _ = std::fs::remove_file(&p);
    let s = s.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}
