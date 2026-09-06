//! Konfigurationsdatei und Kommandozeile. Argument schlägt Datei, Datei schlägt Vorgabe.

use crate::state::JitterMode;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    pub port: u16,
    pub frame_ms: u8,
    pub jitter: String,
    pub volume: u32,
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
    #[serde(rename = "in")]
    pub input: Option<String>,
    #[serde(rename = "out")]
    pub output: Option<String>,
    pub peer: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            port: 4711,
            frame_ms: 5,
            jitter: "auto".into(),
            volume: 100,
            name: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "holler".into()),
            hotkey: "F9".into(),
            headset_ms: 40,
            mic_gain_db: 12.0,
            gate_on: true,
            gate_vad: 0.5,
            denoise: true,
            input: None,
            output: None,
            peer: None,
        }
    }
}

pub fn path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("holler").join("config.toml"))
}

impl Config {
    pub fn load() -> Config {
        let Some(p) = path() else { return Config::default() };
        match std::fs::read_to_string(&p) {
            Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
                eprintln!("Konfiguration {} unlesbar ({e}), nehme Vorgaben.", p.display());
                Config::default()
            }),
            Err(_) => Config::default(),
        }
    }

    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match toml::to_string_pretty(self) {
            Ok(s) => {
                if let Err(e) = std::fs::write(&p, s) {
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
