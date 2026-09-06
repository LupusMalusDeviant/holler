//! Tray-Symbol, globaler Hotkey, Fenster zeigen und verstecken.
//!
//! Alles hier läuft auf dem Hauptthread, aufgerufen aus der
//! Windows-Nachrichtenschleife. Das ist wichtig: sobald das Fenster versteckt
//! ist, ruft egui `update()` nicht mehr auf. Hotkey, Menü und Symbolzustand
//! müssen deshalb ohne das Fenster auskommen. Zeigen und Verstecken laufen über
//! `ShowWindow`, ein Thread-Timer frischt das Symbol jede Sekunde auf.

use crate::icon;
use crate::state::Shared;
use eframe::egui;
use global_hotkey::{hotkey::HotKey, GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, OnceLock};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{SetForegroundWindow, SetTimer, ShowWindow, SW_HIDE, SW_RESTORE, SW_SHOW};


struct Ctl {
    shared: Arc<Shared>,
    ctx: egui::Context,
    hwnd: isize,
    show_id: MenuId,
    mute_id: MenuId,
    quit_id: MenuId,
}

static CTL: OnceLock<Ctl> = OnceLock::new();
static QUIT: AtomicBool = AtomicBool::new(false);
static HIDDEN: AtomicBool = AtomicBool::new(false);

struct TrayState {
    icon: Option<TrayIcon>,
    mute_item: MenuItem,
    _hotkeys: Option<GlobalHotKeyManager>,
    last: (u8, bool),
}

thread_local! {
    static TRAY: RefCell<Option<TrayState>> = const { RefCell::new(None) };
}

pub struct InitReport {
    pub tray_error: Option<String>,
    pub hotkey_error: Option<String>,
}

pub fn init(shared: Arc<Shared>, ctx: egui::Context, hwnd: isize, hotkey: &str, start_hidden: bool) -> InitReport {
    let show = MenuItem::new("Fenster anzeigen", true, None);
    let mute = MenuItem::new("Stumm", true, None);
    let quit_item = MenuItem::new("Beenden", true, None);

    let build = || -> Result<TrayIcon, String> {
        let menu = Menu::new();
        menu.append(&show).map_err(|e| e.to_string())?;
        menu.append(&mute).map_err(|e| e.to_string())?;
        menu.append(&PredefinedMenuItem::separator()).map_err(|e| e.to_string())?;
        menu.append(&quit_item).map_err(|e| e.to_string())?;
        TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Holler")
            .with_icon(make_icon(icon::GREY))
            .build()
            .map_err(|e| e.to_string())
    };
    let (icon, tray_error) = match build() {
        Ok(t) => (Some(t), None),
        Err(e) => (None, Some(format!("Tray-Symbol: {e}"))),
    };

    let (manager, hotkey_error) = match hotkey.parse::<HotKey>() {
        Ok(hk) => match GlobalHotKeyManager::new() {
            Ok(m) => match m.register(hk) {
                Ok(()) => (Some(m), None),
                Err(e) => (None, Some(format!("Hotkey {hotkey} belegt: {e}"))),
            },
            Err(e) => (None, Some(format!("Hotkeys nicht verfügbar: {e}"))),
        },
        Err(e) => (None, Some(format!("Hotkey {hotkey} unlesbar: {e}"))),
    };

    let _ = CTL.set(Ctl {
        shared,
        ctx,
        hwnd,
        show_id: show.id().clone(),
        mute_id: mute.id().clone(),
        quit_id: quit_item.id().clone(),
    });
    HIDDEN.store(start_hidden, Relaxed);
    TRAY.with(|t| {
        *t.borrow_mut() = Some(TrayState { icon, mute_item: mute, _hotkeys: manager, last: (255, false) });
    });

    MenuEvent::set_event_handler(Some(|e: MenuEvent| {
        let Some(c) = CTL.get() else { return };
        if e.id() == &c.show_id {
            show_window();
        } else if e.id() == &c.mute_id {
            toggle_mute();
        } else if e.id() == &c.quit_id {
            quit();
        }
    }));
    TrayIconEvent::set_event_handler(Some(|e: TrayIconEvent| match e {
        TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. }
        | TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } => show_window(),
        _ => {}
    }));
    GlobalHotKeyEvent::set_event_handler(Some(|e: GlobalHotKeyEvent| {
        if e.state == HotKeyState::Pressed {
            toggle_mute();
        }
    }));

    // Thread-Timer: WM_TIMER landet in der Schleife von winit und ruft timer_proc,
    // auch wenn das Fenster versteckt ist.
    unsafe {
        SetTimer(std::ptr::null_mut(), 0, 1000, Some(timer_proc));
    }
    refresh();
    InitReport { tray_error, hotkey_error }
}

unsafe extern "system" fn timer_proc(_: HWND, _: u32, _: usize, _: u32) {
    refresh();
}

pub fn quit_requested() -> bool {
    QUIT.load(Relaxed)
}

pub fn show_window() {
    let Some(c) = CTL.get() else { return };
    if c.hwnd != 0 {
        unsafe {
            ShowWindow(c.hwnd as HWND, SW_SHOW);
            ShowWindow(c.hwnd as HWND, SW_RESTORE);
            SetForegroundWindow(c.hwnd as HWND);
        }
    }
    HIDDEN.store(false, Relaxed);
    c.ctx.request_repaint();
}

pub fn hide_window() {
    let Some(c) = CTL.get() else { return };
    if c.hwnd != 0 {
        unsafe {
            ShowWindow(c.hwnd as HWND, SW_HIDE);
        }
        HIDDEN.store(true, Relaxed);
    }
}

pub fn toggle_mute() {
    let Some(c) = CTL.get() else { return };
    let now = !c.shared.muted.load(Relaxed);
    c.shared.muted.store(now, Relaxed);
    refresh();
    c.ctx.request_repaint();
}

pub fn quit() {
    let Some(c) = CTL.get() else { return };
    QUIT.store(true, Relaxed);
    show_window();
    c.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    c.ctx.request_repaint();
}

/// Symbolfarbe und Tooltip an den Zustand anpassen; nur bei Änderung.
pub fn refresh() {
    let Some(c) = CTL.get() else { return };
    let muted = c.shared.muted.load(Relaxed);
    let conn: u8 = if c.shared.connected() {
        2
    } else if c.shared.peer_addr().is_some() {
        1
    } else {
        0
    };
    TRAY.with(|t| {
        let mut t = t.borrow_mut();
        let Some(t) = t.as_mut() else { return };
        if t.last == (conn, muted) {
            return;
        }
        t.last = (conn, muted);
        let color = if muted {
            icon::RED
        } else {
            match conn {
                2 => icon::GREEN,
                1 => icon::AMBER,
                _ => icon::GREY,
            }
        };
        let state = match conn {
            2 => "verbunden",
            1 => "warte auf Antwort",
            _ => "suche",
        };
        let tip = format!("Holler · {state}{}", if muted { " · stumm" } else { "" });
        if let Some(i) = &t.icon {
            let _ = i.set_icon(Some(make_icon(color)));
            let _ = i.set_tooltip(Some(tip));
        }
        t.mute_item.set_text(if muted { "Stumm aufheben" } else { "Stumm" });
    });
}

/// Tray-Symbol in Zustandsfarbe, dieselbe Zeichnung wie das Programmicon.
fn make_icon(rgb: [u8; 3]) -> Icon {
    Icon::from_rgba(icon::render(32, rgb), 32, 32).expect("Icon")
}
