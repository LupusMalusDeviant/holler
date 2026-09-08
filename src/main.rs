//! Holler — LAN-Funk ohne Umwege.
//! Rohes PCM per UDP, ein Fenster, eine Exe. Siehe docs/INTERFACE.md und docs/INTERFACE-v2.md.

// Fensterprogramm ohne Konsole. Aus einem Terminal gestartet, haengt sich das
// Programm an dessen Konsole, damit --list-devices und --headless Ausgabe zeigen.
#![windows_subsystem = "windows"]

mod audio;
mod codec;
mod config;
mod crypto;
mod desktop;
mod engine;
mod icon;
mod invite;
mod net;
mod single;
mod state;
#[cfg(windows)]
mod tray;
#[cfg(not(windows))]
#[path = "tray_unix.rs"]
mod tray;
mod ui;
mod update;

use clap::Parser;
use config::Config;
use engine::{Engine, EngineError};
use state::Shared;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(name = "holler", version, about = "Holler: LAN-Funk ohne Umwege")]
struct Args {
    /// Einladungslink holler://join?room=…&pw=… (kommt vom Link-Handler)
    #[arg(value_name = "EINLADUNG")]
    url: Option<String>,
    /// Stumm starten
    #[arg(long)]
    mute: bool,
    /// Nach n Sekunden beenden (für Tests)
    #[arg(long = "exit-after", hide = true)]
    exit_after: Option<u64>,
    /// Feste Gegenstelle „ip“ oder „ip:port“ (mehrfach erlaubt), zusätzlich zur Suche
    #[arg(long)]
    peer: Vec<String>,
    /// UDP-Port für Audio und Suche (Vorgabe 4711)
    #[arg(long)]
    port: Option<u16>,
    /// Raum beitreten (Name) — auf der Kommandozeile sofort beim Start
    #[arg(long)]
    room: Option<String>,
    /// Raumpasswort
    #[arg(long = "room-password")]
    room_password: Option<String>,
    /// Vermittler „host:port“ für Räume über das Internet; „off“ = nur LAN
    #[arg(long)]
    hub: Option<String>,
    /// Testschalter: Direktwege ignorieren, alles über den Hub
    #[arg(long = "force-relay")]
    force_relay: bool,
    /// Qualität für Ferne: pcm, opus64, opus32, opus16
    #[arg(long)]
    codec: Option<String>,
    /// Desktop-Audio senden: on oder off
    #[arg(long)]
    desktop: Option<String>,
    /// Desktop-Quelle: all, pid:<nr>, dev:<Gerätename>
    #[arg(long = "desktop-source")]
    desktop_source: Option<String>,
    /// Desktop-Qualität: spiel (Opus 96) oder musik (Opus 160)
    #[arg(long = "desktop-quality")]
    desktop_quality: Option<String>,
    /// Anzeigename bei den anderen
    #[arg(long)]
    name: Option<String>,
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
    /// Lautstärke neuer Teilnehmer 0–300 Prozent
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
    /// Stumm-Hotkey, z. B. F9 oder Ctrl+M
    #[arg(long)]
    hotkey: Option<String>,
    /// Schätzwert Headset-Funk in ms für die Anzeige (Vorgabe 40)
    #[arg(long = "headset-ms")]
    headset_ms: Option<u32>,
    /// Andere Konfigurationsdatei (z. B. für eine zweite Instanz)
    #[arg(long)]
    config: Option<PathBuf>,
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
    /// Programme mit Tonausgabe (Desktop-Quellen) ausgeben und beenden
    #[arg(long = "list-desktop-sources")]
    list_desktop_sources: bool,
}

fn main() {
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS);
    }
    let args = Args::parse();
    let mut cfg = Config::load(args.config.clone());
    if !args.peer.is_empty() { cfg.peers = args.peer.clone(); }
    if let Some(v) = args.port { cfg.port = v; }
    let invite = args.url.as_deref().and_then(invite::Invite::parse);
    if args.url.is_some() && invite.is_none() {
        eprintln!("Einladungslink unlesbar: {}", args.url.clone().unwrap_or_default());
    }
    if let Some(inv) = &invite {
        cfg.room = inv.room.clone();
        cfg.room_password = inv.password.clone();
        if let Some(h) = &inv.hub { cfg.hub = if h.trim().eq_ignore_ascii_case("off") { String::new() } else { h.clone() }; }
    }
    if let Some(v) = &args.room { cfg.room = v.clone(); }
    if let Some(v) = &args.room_password { cfg.room_password = v.clone(); }
    if let Some(v) = &args.hub { cfg.hub = if v.trim().eq_ignore_ascii_case("off") { String::new() } else { v.clone() }; }
    if let Some(v) = &args.codec { cfg.codec = codec::choice_key(codec::parse_choice(v)).to_string(); }
    if let Some(v) = &args.desktop { cfg.desktop_on = v.trim().eq_ignore_ascii_case("on"); }
    if let Some(v) = &args.desktop_source { cfg.desktop_source = v.clone(); }
    if let Some(v) = &args.desktop_quality { cfg.desktop_quality = v.clone(); }
    if let Some(v) = args.name { cfg.name = v; }
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
    if let Some(v) = args.hotkey { cfg.hotkey = v; }
    if let Some(v) = args.headset_ms { cfg.headset_ms = v; }
    if args.no_update_check { cfg.update_check = false; }

    if args.list_desktop_sources {
        println!("Desktop-Quellen:");
        if cfg!(windows) {
            println!("  all                     Alles ausser Holler");
            for (pid, name) in desktop::list_processes() {
                println!("  pid:{pid:<8}            Nur {name}");
            }
        }
        let host = cpal::default_host();
        for (n, _) in audio::enumerate(&host).inputs {
            println!("  dev:{n}");
        }
        return;
    }

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

    // Nur eine Instanz je Port: die zweite reicht den Link weiter und holt das Fenster nach vorn.
    if !args.headless && !single::acquire(cfg.port) {
        if let Some(u) = &args.url {
            single::hand_over_invite(u);
        }
        single::show_existing();
        eprintln!("Holler läuft bereits, Fenster geholt.");
        return;
    }
    if let Some(secs) = args.exit_after {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(secs));
            std::process::exit(0);
        });
    }

    let shared = Arc::new(Shared::new(
        cfg.peer_id,
        cfg.name.clone(),
        cfg.frame_samples(),
        cfg.volume.min(300),
        cfg.jitter_mode(),
        cfg.mic_gain_db,
        cfg.gate(),
        cfg.denoise,
    ));
    let mut engine = match Engine::new(&cfg, shared.clone()) {
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
    eprintln!(
        "Holler {} · Port {} · Name {} · Kennung {:016x} · Rahmen {} ms · IPv6 {}",
        env!("CARGO_PKG_VERSION"),
        cfg.port,
        cfg.name,
        cfg.peer_id,
        cfg.frame_ms,
        if engine.sockets.v6.is_some() { "ja" } else { "nein" }
    );

    shared.set_hub(&cfg.hub);
    if args.mute {
        shared.muted.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    if let Ok(mut m) = shared.volumes.lock() {
        for (k, v) in &cfg.volumes {
            if let Ok(id) = u64::from_str_radix(k, 16) {
                m.insert(id, *v as f32 / 100.0);
            }
        }
    }
    shared.codec_kbps.store(codec::parse_choice(&cfg.codec), std::sync::atomic::Ordering::Relaxed);
    {
        use std::sync::atomic::Ordering::Relaxed;
        let music = cfg.desktop_quality.trim().eq_ignore_ascii_case("musik");
        shared.desktop_music.store(music, Relaxed);
        shared.desktop_kbps.store(if music { 160 } else { 96 }, Relaxed);
        shared.desktop_gain.store((cfg.desktop_gain.min(300) as f32 / 100.0).to_bits(), Relaxed);
        if let Ok(mut g) = shared.desktop_source.lock() {
            *g = cfg.desktop_source.clone();
        }
    }
    shared.force_relay.store(args.force_relay, std::sync::atomic::Ordering::Relaxed);
    if let Some(e) = shared.hub_error.lock().ok().and_then(|g| g.clone()) {
        eprintln!("{e}");
    }

    // Raum von der Kommandozeile: sofort beitreten. Aus der Konfiguration: nur vorausfüllen.
    if (args.room.is_some() || invite.is_some()) && !cfg.room.trim().is_empty() {
        eprintln!("Berechne Raumschlüssel für „{}“ …", cfg.room.trim());
        shared.set_room(Some(crypto::derive(&cfg.room, &cfg.room_password)));
        eprintln!("Raum „{}“ beigetreten, Pakete verschlüsselt.", cfg.room.trim());
    }

    if cfg.desktop_on {
        engine.set_desktop(true);
    }

    if args.headless {
        let _engine = engine;
        loop {
            std::thread::sleep(Duration::from_millis(500));
            eprint!("\r\x1b[2K{}", shared.status_line(cfg.headset_ms));
        }
    }

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([660.0, 940.0])
            .with_min_inner_size([580.0, 640.0])
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
