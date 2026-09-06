//! Konfigurationsdatei und Kommandozeile. Argument schlägt Datei, Datei schlägt Vorgabe.

use crate::state::JitterMode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    /// Zufällige Kennung dieser Installation, wird beim ersten Start erzeugt
    pub peer_id: u64,
    pub port: u16,
    pub frame_ms: u8,
    pub jitter: String,
    /// Lautstärke für neue Teilnehmer in Prozent
    pub volume: u32,
    /// Anzeigename bei den anderen
    pub name: String,
    pub hotkey: String,
    pub headset_ms: u32,
    /// Mikrofon-Verstärkung in dB, 0–30
    pub mic_gain_db: f32,
    /// Sprechsperre an/aus und Schwelle 0..1 für die Sprechwahrscheinlichkeit (RNNoise)
    pub gate_on: bool,
    pub gate_vad: f32,
    /// Rauschunterdrückung (RNNoise)
    pub denoise: bool,
    /// Beim Start auf neue Releases prüfen
    pub update_check: bool,
    /// Raum und Passwort, vorausgefüllt für den nächsten Klick auf „Beitreten“
    pub room: String,
    pub room_password: String,
    /// Vermittler „host:port“ für Räume über das Internet; leer = nur LAN
    pub hub: String,
    /// Qualität für Ferne: pcm, opus64, opus32, opus16
    pub codec: String,
    #[serde(rename = "in")]
    pub input: Option<String>,
    #[serde(rename = "out")]
    pub output: Option<String>,
    /// Feste Gegenstellen „ip“ oder „ip:port“, zusätzlich zur Suche
    pub peers: Vec<String>,
    /// Gemerkte Lautstärken je Teilnehmer-Kennung (hex) in Prozent
    pub volumes: HashMap<String, u32>,
    #[serde(skip)]
    pub path: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            peer_id: 0,
            port: 4711,
            frame_ms: 5,
            jitter: "auto".into(),
            volume: 100,
            name: host_name(),
            hotkey: "F9".into(),
            headset_ms: 40,
            mic_gain_db: 12.0,
            gate_on: true,
            gate_vad: 0.5,
            denoise: true,
            update_check: true,
            room: String::new(),
            room_password: String::new(),
            hub: DEFAULT_HUB.into(),
            codec: "opus32".into(),
            input: None,
            output: None,
            peers: Vec::new(),
            volumes: HashMap::new(),
            path: None,
        }
    }
}

/// Vermittler-Vorgabe. Die IP-Vorgabe aus 0.4 bis 1.1 wird beim Laden auf den Namen umgestellt.
pub const DEFAULT_HUB: &str = "holler.app.lupusmalus.dev:4712";
const OLD_HUB_IP: &str = "168.119.111.164:4712";

/// Rechnername als Vorgabe für den Anzeigenamen.
pub fn host_name() -> String {
    if let Ok(n) = std::env::var("COMPUTERNAME") {
        return n;
    }
    if let Ok(n) = std::env::var("HOSTNAME") {
        return n;
    }
    if let Ok(out) = std::process::Command::new("hostname").output() {
        let n = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !n.is_empty() {
            return n;
        }
    }
    "holler".into()
}

/// Konfigurationsdatei: Windows %APPDATA%\holler, macOS ~/Library/Application Support/holler,
/// Linux $XDG_CONFIG_HOME/holler bzw. ~/.config/holler.
pub fn default_path() -> Option<PathBuf> {
    let dir = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library").join("Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    };
    dir.map(|d| d.join("holler").join("config.toml"))
}

impl Config {
    pub fn load(path: Option<PathBuf>) -> Config {
        let path = path.or_else(default_path);
        let mut cfg = match &path {
            Some(p) => match std::fs::read_to_string(p) {
                Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
                    eprintln!("Konfiguration {} unlesbar ({e}), nehme Vorgaben.", p.display());
                    Config::default()
                }),
                Err(_) => Config::default(),
            },
            None => Config::default(),
        };
        cfg.path = path;
        if cfg.hub.trim() == OLD_HUB_IP {
            cfg.hub = DEFAULT_HUB.into();
        }
        if cfg.peer_id == 0 {
            cfg.peer_id = rand::random::<u64>().max(1);
            cfg.save();
        }
        cfg
    }

    pub fn save(&self) {
        let Some(p) = &self.path else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match toml::to_string_pretty(self) {
            Ok(s) => {
                if let Err(e) = std::fs::write(p, s) {
                    eprintln!("Konfiguration konnte nicht geschrieben werden: {e}");
                }
            }
            Err(e) => eprintln!("Konfiguration konnte nicht serialisiert werden: {e}"),
        }
    }

    pub fn jitter_mode(&self) -> JitterMode {
        match self.jitter.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => JitterMode::Auto,
            s => s.parse::<u32>().map(JitterMode::Fixed).unwrap_or(JitterMode::Auto),
        }
    }

    pub fn gate(&self) -> Option<f32> {
        if self.gate_on { Some(self.gate_vad.clamp(0.05, 0.95)) } else { None }
    }

    pub fn frame_samples(&self) -> u32 {
        48 * self.frame_ms.clamp(2, 20) as u32
    }
}
