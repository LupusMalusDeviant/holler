//! Gemeinsamer Zustand zwischen Audio-Threads, Netz-Threads und Fenster.
//! Acht feste Teilnehmerplätze aus Atomics; die Audio-Threads sperren nie.

use crate::crypto::Room;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering::Relaxed};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

/// Drahtformat: 48 kHz mono int16, unabhängig von den Geräten.
pub const RATE: u32 = 48_000;
pub const MAX_PEERS: usize = 8;
pub const MIN_TARGET: u32 = 1;
pub const MAX_TARGET: u32 = 8;
/// Annahme für die Anzeige: WASAPI Shared Mode arbeitet mit 10-ms-Perioden.
pub const PERIOD_MS: f32 = 10.0;
/// Nach so vielen ms ohne Audio gilt ein Teilnehmer als „keine Pakete“.
pub const SILENT_MS: u64 = 2000;
/// Nach so vielen ms ohne Lebenszeichen fliegt ein Teilnehmer aus der Liste.
pub const GONE_MS: u64 = 15_000;

pub const PATH_UNKNOWN: u8 = 0;
pub const PATH_LAN: u8 = 1;
pub const PATH_V6: u8 = 2;
pub const PATH_V4: u8 = 3;
pub const PATH_RELAY: u8 = 4;

pub const CODEC_PCM: u8 = 0;
pub const CODEC_OPUS: u8 = 1;

pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

pub fn lin_to_db(v: f32) -> f32 {
    if v <= 1e-6 { -120.0 } else { 20.0 * v.log10() }
}

pub fn path_name(p: u8) -> &'static str {
    match p {
        PATH_LAN => "LAN direkt",
        PATH_V6 => "direkt IPv6",
        PATH_V4 => "direkt IPv4",
        PATH_RELAY => "Relay",
        _ => "…",
    }
}

/// Wie erreicht uns diese Adresse? Privat/Link-lokal/Loopback = LAN.
pub fn classify(ip: IpAddr) -> u8 {
    match ip {
        IpAddr::V4(a) => {
            if a.is_private() || a.is_link_local() || a.is_loopback() {
                PATH_LAN
            } else {
                PATH_V4
            }
        }
        IpAddr::V6(a) => {
            if let Some(m) = a.to_ipv4_mapped() {
                return classify(IpAddr::V4(m));
            }
            let s0 = a.segments()[0];
            if a.is_loopback() || (s0 & 0xffc0) == 0xfe80 || (s0 & 0xfe00) == 0xfc00 {
                PATH_LAN
            } else {
                PATH_V6
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum JitterMode {
    Auto,
    Fixed(u32),
}

/// Ein Teilnehmerplatz. Alles Atomics, damit Mixer und Netz ohne Sperre lesen.
pub struct Peer {
    pub active: AtomicBool,
    pub id: AtomicU64,
    pub name: Mutex<String>,
    pub addr: Mutex<Option<SocketAddr>>,
    pub path: AtomicU8,
    pub codec: AtomicU8,
    /// Bitrate des empfangenen Opus-Stroms in kbit/s (0 bei PCM)
    pub codec_kbps: AtomicU32,
    pub frame_samples: AtomicU32,
    pub volume: AtomicU32,
    pub local_mute: AtomicBool,
    pub remote_muted: AtomicBool,
    pub level: AtomicU32,
    pub last_rx_ms: AtomicU64,
    pub last_seen_ms: AtomicU64,
    pub joined_ms: AtomicU64,
    pub rtt_us: AtomicU32,
    pub jitter_us: AtomicU32,
    pub loss_permille: AtomicU32,
    pub buffered_samples: AtomicU32,
    pub target_frames: AtomicU32,
    pub underruns: AtomicU32,
    pub priming: AtomicBool,
    pub skip_samples: AtomicU32,
    pub dropped: AtomicU32,
    /// Adressen, an denen eine Direktverbindung versucht wird (vom Hub gemeldet).
    pub candidates: Mutex<Vec<SocketAddr>>,
    /// Audio läuft über den Hub, weil (noch) kein Direktweg bestätigt ist.
    pub via_relay: AtomicBool,
    pub last_ack_ms: AtomicU64,
    pub last_probe_ms: AtomicU64,
}

impl Peer {
    fn new() -> Self {
        Peer {
            active: AtomicBool::new(false),
            id: AtomicU64::new(0),
            name: Mutex::new(String::new()),
            addr: Mutex::new(None),
            path: AtomicU8::new(PATH_UNKNOWN),
            codec: AtomicU8::new(CODEC_PCM),
            codec_kbps: AtomicU32::new(0),
            frame_samples: AtomicU32::new(240),
            volume: AtomicU32::new(1.0f32.to_bits()),
            local_mute: AtomicBool::new(false),
            remote_muted: AtomicBool::new(false),
            level: AtomicU32::new(0),
            last_rx_ms: AtomicU64::new(u64::MAX),
            last_seen_ms: AtomicU64::new(0),
            joined_ms: AtomicU64::new(0),
            rtt_us: AtomicU32::new(0),
            jitter_us: AtomicU32::new(0),
            loss_permille: AtomicU32::new(0),
            buffered_samples: AtomicU32::new(0),
            target_frames: AtomicU32::new(2),
            underruns: AtomicU32::new(0),
            priming: AtomicBool::new(true),
            skip_samples: AtomicU32::new(0),
            dropped: AtomicU32::new(0),
            candidates: Mutex::new(Vec::new()),
            via_relay: AtomicBool::new(false),
            last_ack_ms: AtomicU64::new(0),
            last_probe_ms: AtomicU64::new(0),
        }
    }

    /// Platz für einen neuen Teilnehmer herrichten. `active` zuletzt, damit der Mixer nichts Halbes sieht.
    #[allow(clippy::too_many_arguments)]
    pub fn assign(&self, id: u64, name: &str, addr: Option<SocketAddr>, path: u8, now_ms: u64, target: u32, volume: f32) {
        self.id.store(id, Relaxed);
        if let Ok(mut n) = self.name.lock() {
            *n = name.to_string();
        }
        if let Ok(mut a) = self.addr.lock() {
            *a = addr;
        }
        if let Ok(mut c) = self.candidates.lock() {
            c.clear();
        }
        self.via_relay.store(false, Relaxed);
        self.last_ack_ms.store(0, Relaxed);
        self.last_probe_ms.store(0, Relaxed);
        self.path.store(path, Relaxed);
        self.codec.store(CODEC_PCM, Relaxed);
        self.codec_kbps.store(0, Relaxed);
        self.volume.store(volume.to_bits(), Relaxed);
        self.local_mute.store(false, Relaxed);
        self.remote_muted.store(false, Relaxed);
        self.level.store(0, Relaxed);
        self.last_rx_ms.store(u64::MAX, Relaxed);
        self.last_seen_ms.store(now_ms, Relaxed);
        self.joined_ms.store(now_ms, Relaxed);
        self.rtt_us.store(0, Relaxed);
        self.jitter_us.store(0, Relaxed);
        self.loss_permille.store(0, Relaxed);
        self.target_frames.store(target, Relaxed);
        self.underruns.store(0, Relaxed);
        self.priming.store(true, Relaxed);
        self.skip_samples.store(0, Relaxed);
        self.dropped.store(0, Relaxed);
        self.active.store(true, Relaxed);
    }

    pub fn clear(&self) {
        self.active.store(false, Relaxed);
        self.id.store(0, Relaxed);
        self.level.store(0, Relaxed);
        self.priming.store(true, Relaxed);
        if let Ok(mut a) = self.addr.lock() {
            *a = None;
        }
    }

    pub fn addr(&self) -> Option<SocketAddr> {
        self.addr.lock().ok().and_then(|a| *a)
    }

    pub fn name(&self) -> String {
        self.name.lock().map(|n| n.clone()).unwrap_or_default()
    }

    pub fn volume_f(&self) -> f32 {
        f32::from_bits(self.volume.load(Relaxed))
    }

    pub fn level_f(&self) -> f32 {
        f32::from_bits(self.level.load(Relaxed))
    }

    /// Audio in den letzten 2 s.
    pub fn streaming(&self, now_ms: u64) -> bool {
        let l = self.last_rx_ms.load(Relaxed);
        l != u64::MAX && now_ms.saturating_sub(l) < SILENT_MS
    }
}

pub struct Shared {
    pub start: Instant,
    pub peer_id: u64,
    pub name: Mutex<String>,
    pub muted: AtomicBool,
    pub mic_level: AtomicU32,
    pub mic_clip: AtomicBool,
    pub mic_gain: AtomicU32,
    pub gate_threshold: AtomicU32,
    pub gate_open: AtomicBool,
    pub denoise: AtomicBool,
    pub spk_level: AtomicU32,
    pub tx_seq: AtomicU32,
    pub tx_seq_opus: AtomicU32,
    /// Eigene Sendequalität für Ferne: 0 = PCM, sonst Opus-Bitrate in kbit/s
    pub codec_kbps: AtomicU32,
    pub frame_samples: AtomicU32,
    pub default_volume: AtomicU32,
    pub jitter_auto: AtomicBool,
    pub jitter_fixed: AtomicU32,
    pub peers: [Peer; MAX_PEERS],
    pub room: RwLock<Option<Arc<Room>>>,
    pub room_busy: AtomicBool,
    pub room_full: AtomicBool,
    pub bad_auth: AtomicU32,
    pub foreign_room: AtomicU32,
    pub net_error: Mutex<Option<String>>,
    pub ipv6: AtomicBool,
    /// Feste Gegenstellen, die regelmässig ein HELLO bekommen.
    pub manual_peers: Mutex<Vec<SocketAddr>>,
    /// Vermittler: aufgelöste Adressen, Zustand, eigene öffentliche Adresse.
    pub hub_v4: Mutex<Option<SocketAddr>>,
    pub hub_v6: Mutex<Option<SocketAddr>>,
    pub hub_name: Mutex<String>,
    pub hub_error: Mutex<Option<String>>,
    pub hub_last_ms: AtomicU64,
    pub hub_rtt_us: AtomicU32,
    pub public_addr: Mutex<Option<SocketAddr>>,
    /// Testschalter: Direktwege ignorieren, alles über den Hub.
    pub force_relay: AtomicBool,
}

impl Shared {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        peer_id: u64,
        name: String,
        frame_samples: u32,
        default_volume_percent: u32,
        jitter: JitterMode,
        mic_gain_db: f32,
        gate: Option<f32>,
        denoise: bool,
    ) -> Self {
        let (auto, fixed) = match jitter {
            JitterMode::Auto => (true, 0),
            JitterMode::Fixed(n) => (false, n.clamp(MIN_TARGET, MAX_TARGET)),
        };
        Shared {
            start: Instant::now(),
            peer_id,
            name: Mutex::new(name),
            muted: AtomicBool::new(false),
            mic_level: AtomicU32::new(0),
            mic_clip: AtomicBool::new(false),
            mic_gain: AtomicU32::new(db_to_lin(mic_gain_db).to_bits()),
            gate_threshold: AtomicU32::new(gate.unwrap_or(0.0).to_bits()),
            gate_open: AtomicBool::new(true),
            denoise: AtomicBool::new(denoise),
            spk_level: AtomicU32::new(0),
            tx_seq: AtomicU32::new(0),
            tx_seq_opus: AtomicU32::new(0),
            codec_kbps: AtomicU32::new(32),
            frame_samples: AtomicU32::new(frame_samples),
            default_volume: AtomicU32::new((default_volume_percent as f32 / 100.0).to_bits()),
            jitter_auto: AtomicBool::new(auto),
            jitter_fixed: AtomicU32::new(fixed),
            peers: std::array::from_fn(|_| Peer::new()),
            room: RwLock::new(None),
            room_busy: AtomicBool::new(false),
            room_full: AtomicBool::new(false),
            bad_auth: AtomicU32::new(0),
            foreign_room: AtomicU32::new(0),
            net_error: Mutex::new(None),
            ipv6: AtomicBool::new(false),
            manual_peers: Mutex::new(Vec::new()),
            hub_v4: Mutex::new(None),
            hub_v6: Mutex::new(None),
            hub_name: Mutex::new(String::new()),
            hub_error: Mutex::new(None),
            hub_last_ms: AtomicU64::new(u64::MAX),
            hub_rtt_us: AtomicU32::new(0),
            public_addr: Mutex::new(None),
            force_relay: AtomicBool::new(false),
        }
    }

    /// Hub-Adresse auflösen (DNS erlaubt) und merken. Leer = kein Hub.
    pub fn set_hub(&self, spec: &str) {
        use std::net::ToSocketAddrs;
        let spec = spec.trim().to_string();
        let mut v4 = None;
        let mut v6 = None;
        let mut err = None;
        if !spec.is_empty() {
            let with_port = if spec.rsplit(':').next().is_some_and(|p| p.parse::<u16>().is_ok()) && (spec.matches(':').count() == 1 || spec.contains(']')) {
                spec.clone()
            } else {
                format!("{spec}:4712")
            };
            match with_port.to_socket_addrs() {
                Ok(addrs) => {
                    for a in addrs {
                        match a {
                            SocketAddr::V4(_) if v4.is_none() => v4 = Some(a),
                            SocketAddr::V6(_) if v6.is_none() => v6 = Some(a),
                            _ => {}
                        }
                    }
                    if v4.is_none() && v6.is_none() {
                        err = Some(format!("Hub {spec}: keine Adresse gefunden"));
                    }
                }
                Err(e) => err = Some(format!("Hub {spec}: {e}")),
            }
        }
        if let Ok(mut g) = self.hub_v4.lock() {
            *g = v4;
        }
        if let Ok(mut g) = self.hub_v6.lock() {
            *g = v6;
        }
        if let Ok(mut g) = self.hub_name.lock() {
            *g = spec;
        }
        if let Ok(mut g) = self.hub_error.lock() {
            *g = err;
        }
        self.hub_last_ms.store(u64::MAX, Relaxed);
    }

    pub fn hub_addrs(&self) -> (Option<SocketAddr>, Option<SocketAddr>) {
        (self.hub_v4.lock().ok().and_then(|g| *g), self.hub_v6.lock().ok().and_then(|g| *g))
    }

    pub fn hub_configured(&self) -> bool {
        let (a, b) = self.hub_addrs();
        a.is_some() || b.is_some()
    }

    /// Hub hat in den letzten 15 s geantwortet.
    pub fn hub_alive(&self) -> bool {
        let l = self.hub_last_ms.load(Relaxed);
        l != u64::MAX && self.now_ms().saturating_sub(l) < GONE_MS
    }

    pub fn is_hub_addr(&self, a: SocketAddr) -> bool {
        let (v4, v6) = self.hub_addrs();
        Some(a) == v4 || Some(a) == v6
    }

    pub fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    pub fn now_us(&self) -> u32 {
        self.start.elapsed().as_micros() as u32
    }

    pub fn name(&self) -> String {
        self.name.lock().map(|n| n.clone()).unwrap_or_default()
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

    pub fn set_gate(&self, p: Option<f32>) {
        self.gate_threshold.store(p.unwrap_or(0.0).to_bits(), Relaxed);
    }

    pub fn mic_level_f(&self) -> f32 {
        f32::from_bits(self.mic_level.load(Relaxed))
    }

    pub fn spk_level_f(&self) -> f32 {
        f32::from_bits(self.spk_level.load(Relaxed))
    }

    pub fn default_volume_f(&self) -> f32 {
        f32::from_bits(self.default_volume.load(Relaxed))
    }

    /// Zielpuffer für einen neuen Teilnehmer.
    pub fn initial_target(&self) -> u32 {
        if self.jitter_auto.load(Relaxed) { 2 } else { self.jitter_fixed.load(Relaxed).clamp(MIN_TARGET, MAX_TARGET) }
    }

    pub fn room(&self) -> Option<Arc<Room>> {
        self.room.read().ok().and_then(|r| r.clone())
    }

    pub fn room_id(&self) -> [u8; crate::crypto::ROOM_ID_LEN] {
        self.room().map(|r| r.id).unwrap_or([0u8; crate::crypto::ROOM_ID_LEN])
    }

    pub fn set_room(&self, room: Option<Room>) {
        if let Ok(mut r) = self.room.write() {
            *r = room.map(Arc::new);
        }
        self.clear_peers();
        self.bad_auth.store(0, Relaxed);
        self.foreign_room.store(0, Relaxed);
    }

    pub fn find_peer(&self, id: u64) -> Option<usize> {
        self.peers.iter().position(|p| p.active.load(Relaxed) && p.id.load(Relaxed) == id)
    }

    pub fn free_slot(&self) -> Option<usize> {
        self.peers.iter().position(|p| !p.active.load(Relaxed))
    }

    pub fn clear_peers(&self) {
        for p in &self.peers {
            p.clear();
        }
        self.room_full.store(false, Relaxed);
    }

    pub fn peer_count(&self) -> usize {
        self.peers.iter().filter(|p| p.active.load(Relaxed)).count()
    }

    /// Mindestens ein Teilnehmer liefert gerade Audio.
    pub fn connected(&self) -> bool {
        let now = self.now_ms();
        self.peers.iter().any(|p| p.active.load(Relaxed) && p.streaming(now))
    }

    #[allow(dead_code)]
    pub fn frame_ms(&self) -> f32 {
        self.frame_samples.load(Relaxed) as f32 / (RATE as f32 / 1000.0)
    }

    /// Software-Anteil für die Anzeige: Aufnahme + grösster Pufferfüllstand + Wiedergabe.
    pub fn software_latency_ms(&self) -> f32 {
        let buffered = self
            .peers
            .iter()
            .filter(|p| p.active.load(Relaxed))
            .map(|p| p.buffered_samples.load(Relaxed))
            .max()
            .unwrap_or(0) as f32
            / (RATE as f32 / 1000.0);
        PERIOD_MS + buffered + PERIOD_MS
    }

    pub fn status_line(&self, headset_ms: u32) -> String {
        let now = self.now_ms();
        let bars = |v: f32| {
            let n = (v.clamp(0.0, 1.0) * 6.0).round() as usize;
            format!("{}{}", "▮".repeat(n), "▯".repeat(6 - n))
        };
        let mut peers = Vec::new();
        for p in self.peers.iter().filter(|p| p.active.load(Relaxed)) {
            peers.push(format!(
                "{}{}[{} {} rtt{:.0} jit{:.0} buf{}/{:.0}ms und{} drop{} {}]",
                if p.streaming(now) { "●" } else { "○" },
                p.name(),
                path_name(p.path.load(Relaxed)),
                if p.codec.load(Relaxed) == CODEC_OPUS { format!("opus{}", p.codec_kbps.load(Relaxed)) } else { "pcm".to_string() },
                p.rtt_us.load(Relaxed) as f32 / 2000.0,
                p.jitter_us.load(Relaxed) as f32 / 1000.0,
                p.target_frames.load(Relaxed),
                p.buffered_samples.load(Relaxed) as f32 / 48.0,
                p.underruns.load(Relaxed),
                p.dropped.load(Relaxed),
                bars(p.level_f() * 3.0),
            ));
        }
        let room = self.room().map(|r| format!("Raum {} ", r.name)).unwrap_or_else(|| "LAN ".into());
        let hub = if !self.hub_configured() {
            String::new()
        } else if self.hub_alive() {
            format!("hub {:.0}ms{} ", self.hub_rtt_us.load(Relaxed) as f32 / 1000.0, self.public_addr.lock().ok().and_then(|g| *g).map(|a| format!(" pub {a}")).unwrap_or_default())
        } else {
            "hub - ".into()
        };
        format!(
            "{room}{hub}{} Teilnehmer  {}  sw {:.0}ms (+{} Headset)  mic {}  {}",
            self.peer_count(),
            if peers.is_empty() { "suche...".to_string() } else { peers.join(" ") },
            self.software_latency_ms(),
            headset_ms,
            bars(self.mic_level_f() * 3.0),
            if self.muted.load(Relaxed) { "[STUMM]" } else if self.gate_open.load(Relaxed) { "gate:offen" } else { "gate:zu" },
        ) + &if self.bad_auth.load(Relaxed) > 0 { format!("  abgewiesen:{}", self.bad_auth.load(Relaxed)) } else { String::new() }
    }
}
