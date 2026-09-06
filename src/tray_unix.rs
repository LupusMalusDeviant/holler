//! Linux/macOS: kein Tray, kein Verstecken. Schliessen beendet das Programm.
//! Der globale Hotkey läuft über global-hotkey (macOS, Linux mit X11), sonst gibt es
//! nur den Knopf im Fenster. Gleiche Schnittstelle wie das Windows-Modul.

use crate::state::Shared;
use eframe::egui;
use global_hotkey::{hotkey::HotKey, GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock};

struct Ctl {
    shared: Arc<Shared>,
    ctx: egui::Context,
}

static CTL: OnceLock<Ctl> = OnceLock::new();
static QUIT: AtomicBool = AtomicBool::new(false);
static HOTKEYS: Mutex<Option<GlobalHotKeyManager>> = Mutex::new(None);

pub struct InitReport {
    pub tray_error: Option<String>,
    pub hotkey_error: Option<String>,
}

pub fn init(shared: Arc<Shared>, ctx: egui::Context, _hwnd: isize, hotkey: &str, _start_hidden: bool) -> InitReport {
    let hotkey_error = match hotkey.parse::<HotKey>() {
        Ok(hk) => match GlobalHotKeyManager::new() {
            Ok(m) => match m.register(hk) {
                Ok(()) => {
                    if let Ok(mut g) = HOTKEYS.lock() {
                        *g = Some(m);
                    }
                    None
                }
                Err(e) => Some(format!("Hotkey {hotkey} belegt: {e}")),
            },
            Err(e) => Some(format!("Hotkeys nicht verfügbar: {e}")),
        },
        Err(e) => Some(format!("Hotkey {hotkey} unlesbar: {e}")),
    };
    let _ = CTL.set(Ctl { shared, ctx });
    GlobalHotKeyEvent::set_event_handler(Some(|e: GlobalHotKeyEvent| {
        if e.state == HotKeyState::Pressed {
            toggle_mute();
        }
    }));
    InitReport { tray_error: None, hotkey_error }
}

pub fn quit_requested() -> bool {
    QUIT.load(Relaxed)
}

pub fn show_window() {
    if let Some(c) = CTL.get() {
        c.ctx.request_repaint();
    }
}

/// Ohne Tray gibt es kein Verstecken; das Fenster bleibt.
pub fn hide_window() {}

pub fn toggle_mute() {
    let Some(c) = CTL.get() else { return };
    let now = !c.shared.muted.load(Relaxed);
    c.shared.muted.store(now, Relaxed);
    c.ctx.request_repaint();
}

pub fn quit() {
    let Some(c) = CTL.get() else { return };
    QUIT.store(true, Relaxed);
    c.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    c.ctx.request_repaint();
}

pub fn refresh() {}
