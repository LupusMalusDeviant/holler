//! Gemeinsamer Zustand zwischen Audio-Threads, Netz-Thread und Fenster.
//! Nur Atomics und zwei selten benutzte Mutexe. Die Audio-Threads sperren nie.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::Mutex;
use std::time::Instant;

/// Drahtformat: immer 48 kHz mono int16, unabhängig von den Geräten.
pub const RATE: u32 = 48_000;
pub const MIN_TARGET: u32 = 1;
pub const MAX_TARGET: u32 = 8;
/// Annahme für die Anzeige: WASAPI Shared Mode arbeitet mit 10-ms-Perioden.
pub const PERIOD_MS: f32 = 10.0;
/// Nach so vielen ms ohne Paket gilt der Peer als weg.
pub const TIMEOUT_MS: u64 = 2000;

pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

pub fn lin_to_db(v: f32) -> f32 {
    if v <= 1e-6 { -120.0 } else { 20.0 * v.log10() }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum JitterMode {
    Auto,
    Fixed(u32),
}

pub struct Shared {
    pub start: Instant,
    pub muted: AtomicBool,
    pub volume: AtomicU32,
    pub mic_level: AtomicU32,
    pub mic_clip: AtomicBool,
    /// Lineare Mikrofon-Verstärkung (1.0 = 0 dB)
    pub mic_gain: AtomicU32,
    /// Sprechsperre: Schwelle für die RNNoise-Sprechwahrscheinlichkeit, 0 = aus
    pub gate_threshold: AtomicU32,
    pub gate_open: AtomicBool,
    /// Rauschunterdrückung (RNNoise) an
    pub denoise: AtomicBool,
    pub spk_level: AtomicU32,
    pub tx_seq: AtomicU16,
    pub frame_samples: AtomicU32,
    /// 0 = kein Peer, sonst (IPv4 als u32) << 16 | Port
    peer: AtomicU64,
    pub peer_name: Mutex<String>,
    pub second_peer: Mutex<Option<String>>,
    pub last_rx_ms: AtomicU64,
    pub peer_muted: AtomicBool,
    pub rtt_us: AtomicU32,
    pub jitter_us: AtomicU32,
    pub loss_permille: AtomicU32,
    pub buffered_samples: AtomicU32,
    pub target_frames: AtomicU32,
    pub jitter_auto: AtomicBool,
    pub underruns: AtomicU32,
    pub priming: AtomicBool,
    pub skip_samples: AtomicU32,
    pub dropped: AtomicU32,
    pub net_error: Mutex<Option<String>>,
}

impl Shared {
    pub fn new(frame_samples: u32, volume_percent: u32, jitter: JitterMode, mic_gain_db: f32, gate: Option<f32>, denoise: bool) -> Self {
        let (auto, target) = match jitter {
            JitterMode::Auto => (true, 2),
            JitterMode::Fixed(n) => (false, n.clamp(MIN_TARGET, MAX_TARGET)),
        };
        Shared {
            start: Instant::now(),
            muted: AtomicBool::new(false),
            volume: AtomicU32::new((volume_percent as f32 / 100.0).to_bits()),
            mic_level: AtomicU32::new(0),
            mic_clip: AtomicBool::new(false),
            mic_gain: AtomicU32::new(db_to_lin(mic_gain_db).to_bits()),
            gate_threshold: AtomicU32::new(gate.unwrap_or(0.0).to_bits()),
            gate_open: AtomicBool::new(true),
            denoise: AtomicBool::new(denoise),
            spk_level: AtomicU32::new(0),
            tx_seq: AtomicU16::new(0),
            frame_samples: AtomicU32::new(frame_samples),
            peer: AtomicU64::new(0),
            peer_name: Mutex::new(String::new()),
            second_peer: Mutex::new(None),
            last_rx_ms: AtomicU64::new(u64::MAX),
            peer_muted: AtomicBool::new(false),
            rtt_us: AtomicU32::new(0),
            jitter_us: AtomicU32::new(0),
            loss_permille: AtomicU32::new(0),
            buffered_samples: AtomicU32::new(0),
            target_frames: AtomicU32::new(target),
            jitter_auto: AtomicBool::new(auto),
            underruns: AtomicU32::new(0),
            priming: AtomicBool::new(true),
            skip_samples: AtomicU32::new(0),
            dropped: AtomicU32::new(0),
            net_error: Mutex::new(None),
        }
    }

    pub fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    pub fn now_us(&self) -> u32 {
        self.start.elapsed().as_micros() as u32
    }

    pub fn volume_f(&self) -> f32 {
        f32::from_bits(self.volume.load(Relaxed))
    }

    pub fn set_volume_f(&self, v: f32) {
        self.volume.store(v.to_bits(), Relaxed);
    }

    pub fn mic_gain_f(&self) -> f32 {
        f32::from_bits(self.mic_gain.load(Relaxed))
    }

    pub fn set_mic_gain_db(&self, db: f32) {
        self.mic_gain.store(db_to_lin(db).to_bits(), Relaxed);
    }

    pub fn gate_threshold_f(&self) -> f32 {
        f32::from_bits(self.gate_threshold.load(Relaxed))
    }

    /// None = Sprechsperre aus, sonst Schwelle 0..1 für die Sprechwahrscheinlichkeit
    pub fn set_gate(&self, p: Option<f32>) {
        self.gate_threshold.store(p.unwrap_or(0.0).to_bits(), Relaxed);
    }

    pub fn mic_level_f(&self) -> f32 {
        f32::from_bits(self.mic_level.load(Relaxed))
    }

    pub fn spk_level_f(&self) -> f32 {
        f32::from_bits(self.spk_level.load(Relaxed))
    }

    pub fn peer_addr(&self) -> Option<SocketAddr> {
        let v = self.peer.load(Relaxed);
        if v == 0 {
            return None;
        }
        let ip = Ipv4Addr::from((v >> 16) as u32);
        let port = (v & 0xffff) as u16;
        Some(SocketAddr::V4(SocketAddrV4::new(ip, port)))
    }

    pub fn set_peer(&self, addr: SocketAddr) {
        if let SocketAddr::V4(a) = addr {
            let v = ((u32::from(*a.ip()) as u64) << 16) | a.port() as u64;
            self.peer.store(v, Relaxed);
        }
    }

    pub fn clear_peer(&self) {
        self.peer.store(0, Relaxed);
        self.last_rx_ms.store(u64::MAX, Relaxed);
        if let Ok(mut n) = self.peer_name.lock() {
            n.clear();
        }
    }

    /// Verbunden = in den letzten 2 s kam ein Audio-Paket (auch ein stummes).
    pub fn connected(&self) -> bool {
        let last = self.last_rx_ms.load(Relaxed);
        last != u64::MAX && self.now_ms().saturating_sub(last) < TIMEOUT_MS
    }

    pub fn ever_connected(&self) -> bool {
        self.last_rx_ms.load(Relaxed) != u64::MAX
    }

    pub fn frame_ms(&self) -> f32 {
        self.frame_samples.load(Relaxed) as f32 / (RATE as f32 / 1000.0)
    }

    /// Software-Anteil: Aufnahmeperiode + Pufferfüllstand + Wiedergabeperiode.
    pub fn software_latency_ms(&self) -> f32 {
        let buffered = self.buffered_samples.load(Relaxed) as f32 / (RATE as f32 / 1000.0);
        PERIOD_MS + buffered + PERIOD_MS
    }

    pub fn status_line(&self, headset_ms: u32) -> String {
        let (dot, who) = match (self.peer_addr(), self.connected()) {
            (Some(a), true) => ("●", format!("{} {}", self.peer_name.lock().map(|n| n.clone()).unwrap_or_default(), a.ip())),
            (Some(a), false) => ("○", format!("keine Pakete {}", a.ip())),
            (None, _) => ("○", "suche...".to_string()),
        };
        let bars = |v: f32| {
            let n = (v.clamp(0.0, 1.0) * 6.0).round() as usize;
            format!("{}{}", "▮".repeat(n), "▯".repeat(6 - n))
        };
        format!(
            "{dot} {who}  rtt {:.0}ms  jit {:.0}ms  loss {:.1}%  buf {}/{:.0}ms  sw {:.0}ms (+{} Headset)  mic {}  spk {}  {}",
            self.rtt_us.load(Relaxed) as f32 / 1000.0,
            self.jitter_us.load(Relaxed) as f32 / 1000.0,
            self.loss_permille.load(Relaxed) as f32 / 10.0,
            self.target_frames.load(Relaxed),
            self.buffered_samples.load(Relaxed) as f32 / 48.0,
            self.software_latency_ms(),
            headset_ms,
            bars(self.mic_level_f() * 3.0),
            bars(self.spk_level_f() * 3.0),
            if self.muted.load(Relaxed) { "[STUMM]" } else if self.gate_open.load(Relaxed) { "gate:offen" } else { "gate:zu" },
        )
    }
}
