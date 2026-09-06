//! Holler — LAN-Funk ohne Umwege.
//! Rohes PCM per UDP, ein Fenster, eine Exe. Siehe docs/INTERFACE.md.

// Fensterprogramm ohne Konsole. Aus einem Terminal gestartet, haengt sich das
// Programm an dessen Konsole, damit --list-devices und --headless Ausgabe zeigen.
#![windows_subsystem = "windows"]

mod audio;
mod config;
mod engine;
mod icon;
mod net;
mod state;
mod tray;
mod ui;
mod update;

use clap::Parser;
use config::Config;
use engine::{Engine, EngineError};
use state::Shared;
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(name = "holler", version, about = "Holler: LAN-Funk ohne Umwege")]
struct Args {
    /// Peer fest vorgeben statt Broadcast-Suche (im WLAN oft nötig)
    #[arg(long)]
    peer: Option<String>,
    /// UDP-Port für Audio und Suche (Vorgabe 4711)
    #[arg(long)]
    port: Option<u16>,
    /// Aufnahmegerät, Teilstring oder Index
    #[arg(long = "in")]
    input: Option<String>,
    /// Wiedergabegerät, Teilstring oder Index
    #[arg(long = "out")]
    output: Option<String>,
    /// Rahmenlänge in ms, 5 oder 10 (Vorgabe 5)
    #[arg(long)]
    frame: Option<u8>,
    /// Jitter-Puffer: auto oder Rahmenzahl 1–8
    #[arg(long)]
    jitter: Option<String>,
    /// Partner-Lautstärke 0–300 Prozent
    #[arg(long)]
    volume: Option<u32>,
    /// Mikrofon-Verstärkung in dB, 0–30 (Vorgabe 12)
    #[arg(long = "mic-gain")]
    mic_gain: Option<f32>,
    /// Sprechsperre: Schwelle 0.05–0.95 für die Sprechwahrscheinlichkeit (Vorgabe 0.5) oder off
    #[arg(long)]
    gate: Option<String>,
    /// Rauschunterdrückung (RNNoise): on oder off (Vorgabe on)
    #[arg(long)]
    denoise: Option<String>,
    /// Anzeigename für den Partner
    #[arg(long)]
    name: Option<String>,
    /// Stumm-Hotkey, z. B. F9 oder Ctrl+M
    #[arg(long)]
    hotkey: Option<String>,
    /// Schätzwert Headset-Funk in ms für die Anzeige (Vorgabe 40)
    #[arg(long = "headset-ms")]
    headset_ms: Option<u32>,
    /// Versteckt starten, nur Tray-Symbol
    #[arg(long)]
    hidden: bool,
    /// Keine Update-Prüfung bei GitHub
    #[arg(long = "no-update-check")]
    no_update_check: bool,
    /// Kein Fenster, Statuszeile in der Konsole
    #[arg(long)]
    headless: bool,
    /// Geräte mit Index ausgeben und beenden
    #[arg(long = "list-devices")]
    list_devices: bool,
}

fn main() {
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS);
    }
    let args = Args::parse();
    let mut cfg = Config::load();
    if let Some(v) = args.peer { cfg.peer = if v.trim().is_empty() { None } else { Some(v) }; }
    if let Some(v) = args.port { cfg.port = v; }
    if let Some(v) = args.input { cfg.input = Some(v); }
    if let Some(v) = args.output { cfg.output = Some(v); }
    if let Some(v) = args.frame { cfg.frame_ms = v; }
    if let Some(v) = args.jitter { cfg.jitter = v; }
    if let Some(v) = args.volume { cfg.volume = v.min(300); }
    if let Some(v) = args.mic_gain { cfg.mic_gain_db = v.clamp(0.0, 30.0); }
    if let Some(v) = args.gate {
        cfg.gate_on = v.trim().to_ascii_lowercase() != "off";
        if let Ok(p) = v.trim().parse::<f32>() { cfg.gate_vad = p.clamp(0.05, 0.95); }
    }
    if let Some(v) = args.denoise { cfg.denoise = v.trim().to_ascii_lowercase() != "off"; }
    if let Some(v) = args.name { cfg.name = v; }
    if let Some(v) = args.hotkey { cfg.hotkey = v; }
    if let Some(v) = args.headset_ms { cfg.headset_ms = v; }
    if args.no_update_check { cfg.update_check = false; }

    if args.list_devices {
        let host = cpal::default_host();
        let d = audio::enumerate(&host);
        println!("Aufnahmegeräte:");
        for (i, (n, _)) in d.inputs.iter().enumerate() {
            let def = if d.default_input.as_deref() == Some(n) { "  (Standard)" } else { "" };
            println!("  {i:2}  {n}{def}");
        }
        println!("Wiedergabegeräte:");
        for (i, (n, _)) in d.outputs.iter().enumerate() {
            let def = if d.default_output.as_deref() == Some(n) { "  (Standard)" } else { "" };
            println!("  {i:2}  {n}{def}");
        }
        return;
    }

    let shared = Arc::new(Shared::new(cfg.frame_samples(), cfg.volume.min(300), cfg.jitter_mode(), cfg.mic_gain_db, cfg.gate(), cfg.denoise));
    let engine = match Engine::new(&cfg, shared.clone()) {
        Ok(e) => e,
        Err(EngineError::Port(e)) => {
            eprintln!("{e}");
            std::process::exit(3);
        }
        Err(EngineError::Device(e)) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    if let Some(e) = &engine.in_error {
        eprintln!("Mikrofon konnte nicht geöffnet werden: {e}");
    }
    if let Some(e) = &engine.out_error {
        eprintln!("Ausgabe konnte nicht geöffnet werden: {e}");
    }
    eprintln!("Holler {} · Port {} · Name {} · Rahmen {} ms", env!("CARGO_PKG_VERSION"), cfg.port, cfg.name, cfg.frame_ms);

    if args.headless {
        let _engine = engine;
        loop {
            std::thread::sleep(Duration::from_millis(500));
            eprint!("\r\x1b[2K{}", shared.status_line(cfg.headset_ms));
        }
    }

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([640.0, 900.0])
            .with_min_inner_size([560.0, 640.0])
            .with_visible(!args.hidden)
            .with_icon(Arc::new(eframe::egui::IconData { rgba: icon::render(64, icon::GREEN), width: 64, height: 64 }))
            .with_title("Holler"),
        ..Default::default()
    };
    let start_hidden = args.hidden;
    let updater = update::Updater::new(cfg.update_check);
    updater.spawn_check();
    if let Err(e) = eframe::run_native(
        "holler",
        options,
        Box::new(move |cc| Ok(Box::new(ui::App::new(shared, engine, cfg, cc, start_hidden, updater)))),
    ) {
        eprintln!("Fenster konnte nicht gestartet werden: {e}");
        std::process::exit(1);
    }
    // Sicherheitsnetz: nichts darf den Prozess nach dem Fenster am Leben halten.
    std::process::exit(0);
}
