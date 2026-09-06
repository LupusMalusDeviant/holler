//! Drahtprotokoll v2: UDP, symmetrisch, IPv4 und IPv6, bis 8 Teilnehmer, Hub.
//!
//! Kopf (Klartext, 20 Byte): "H2", Typ, Flags, Absender-Kennung u64, Sequenz u32, Zeit µs u32.
//! Nutzlast: bei Raum verschlüsselt (ChaCha20-Poly1305, Nonce = Kennung+Sequenz, Kopf als AAD).
//!   HELLO   1  [version=2][port u16][raum-id 32][name utf8]        Suche per Broadcast, Klartext
//!   AUDIO   2  [codec][rahmen ms][pcm i16 * n]                       ein Rahmen pro Paket
//!   PONG    3  leer; das Zeitfeld trägt den Zeitstempel des Fragenden Laufzeit (Peer oder Hub)
//!   PROBE   4  [raum-id 32]                                          Direktweg suchen/halten
//!   PROBE-ACK 5 [raum-id 32]                                         Antwort, bestätigt den Weg
//!   LEAVE   6  leer
//!   JOIN   10  an den Hub: [version][raum-id 32][namelen][name][n][fam,ip16,port]*n
//!   MEMBERS 11 vom Hub: [count]([id][namelen][name][n][kind,fam,ip16,port]*n)*count
//!   RELAY  12  an den Hub: [ziel-id u64][inneres Paket]
//!   PING   13  an den Hub, kommt als PONG zurück
//! Flags: Bit 0 Sender stumm (dann keine Samples), Bit 2 Nutzlast verschlüsselt.

use crate::codec;
use crate::crypto::ROOM_ID_LEN;
use crate::state::{classify, Shared, CODEC_OPUS, CODEC_PCM, MAX_PEERS, MAX_TARGET, MIN_TARGET, PATH_LAN, PATH_RELAY, PATH_V6, RATE};
use std::collections::VecDeque;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6, UdpSocket};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const VERSION: u8 = 2;
pub const HEADER: usize = 20;
const MAGIC: [u8; 2] = *b"H2";
const T_HELLO: u8 = 1;
const T_AUDIO: u8 = 2;
const T_PONG: u8 = 3;
const T_PROBE: u8 = 4;
const T_PROBE_ACK: u8 = 5;
const T_LEAVE: u8 = 6;
const T_JOIN: u8 = 10;
const T_MEMBERS: u8 = 11;
const T_RELAY: u8 = 12;
const T_PING: u8 = 13;
const F_MUTED: u8 = 1;
const F_ENC: u8 = 4;
const MAX_GAP_FILL: u32 = 8;
/// Nach so vielen ms ohne bestätigten Direktweg geht Audio über den Hub.
const RELAY_AFTER_MS: u64 = 2000;
/// Direktweg gilt als tot, wenn so lange kein PROBE-ACK kam.
const DIRECT_DEAD_MS: u64 = 15_000;
/// Opus-Pakete nutzen den oberen Sequenzraum, damit die Nonce je Schlüssel eindeutig bleibt.
const SEQ_OPUS: u32 = 0x8000_0000;

#[derive(Clone, Copy)]
struct Header {
    typ: u8,
    flags: u8,
    sender: u64,
    seq: u32,
    ts: u32,
}

fn write_header(buf: &mut Vec<u8>, h: &Header) {
    buf.extend_from_slice(&MAGIC);
    buf.push(h.typ);
    buf.push(h.flags);
    buf.extend_from_slice(&h.sender.to_le_bytes());
    buf.extend_from_slice(&h.seq.to_le_bytes());
    buf.extend_from_slice(&h.ts.to_le_bytes());
}

fn parse_header(p: &[u8]) -> Option<Header> {
    if p.len() < HEADER || p[0..2] != MAGIC {
        return None;
    }
    Some(Header {
        typ: p[2],
        flags: p[3],
        sender: u64::from_le_bytes(p[4..12].try_into().ok()?),
        seq: u32::from_le_bytes(p[12..16].try_into().ok()?),
        ts: u32::from_le_bytes(p[16..20].try_into().ok()?),
    })
}

fn nonce(sender: u64, seq: u32) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[..8].copy_from_slice(&sender.to_le_bytes());
    n[8..].copy_from_slice(&seq.to_le_bytes());
    n
}

fn push_addr(buf: &mut Vec<u8>, a: SocketAddr) {
    match a.ip() {
        IpAddr::V4(ip) => {
            buf.push(4);
            buf.extend_from_slice(&ip.to_ipv6_mapped().octets());
        }
        IpAddr::V6(ip) => {
            buf.push(6);
            buf.extend_from_slice(&ip.octets());
        }
    }
    buf.extend_from_slice(&a.port().to_le_bytes());
}

fn read_addr(p: &[u8]) -> Option<(SocketAddr, usize)> {
    if p.len() < 19 {
        return None;
    }
    let mut oct = [0u8; 16];
    oct.copy_from_slice(&p[1..17]);
    let port = u16::from_le_bytes([p[17], p[18]]);
    let ip = match p[0] {
        4 => IpAddr::V4(Ipv6Addr::from(oct).to_ipv4_mapped().unwrap_or(Ipv4Addr::UNSPECIFIED)),
        _ => IpAddr::V6(Ipv6Addr::from(oct)),
    };
    Some((SocketAddr::new(ip, port), 19))
}

/// Rang eines Direktwegs: kleiner ist besser.
fn prio(a: SocketAddr) -> u8 {
    match classify(a.ip()) {
        PATH_LAN => 0,
        PATH_V6 => 1,
        _ => 2,
    }
}

/// Ein IPv4- und ein optionaler IPv6-Socket auf demselben Port.
pub struct Sockets {
    pub v4: Arc<UdpSocket>,
    pub v6: Option<Arc<UdpSocket>>,
    pub port: u16,
}

impl Sockets {
    pub fn bind(port: u16) -> Result<Arc<Sockets>, String> {
        let v4 = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port))
            .map_err(|e| format!("UDP-Port {port} (IPv4) belegt oder nicht bindbar: {e}"))?;
        v4.set_broadcast(true).map_err(|e| e.to_string())?;
        v4.set_read_timeout(Some(Duration::from_millis(100))).map_err(|e| e.to_string())?;
        // Windows bindet [::] standardmässig nur IPv6 (V6ONLY), daher kein Konflikt mit 0.0.0.0.
        let v6 = match UdpSocket::bind(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, port, 0, 0)) {
            Ok(s) => {
                let _ = s.set_read_timeout(Some(Duration::from_millis(200)));
                Some(Arc::new(s))
            }
            Err(e) => {
                eprintln!("IPv6 nicht verfügbar ({e}), nur IPv4.");
                None
            }
        };
        Ok(Arc::new(Sockets { v4: Arc::new(v4), v6, port }))
    }

    pub fn send_to(&self, buf: &[u8], addr: SocketAddr) {
        match addr {
            SocketAddr::V4(_) => {
                let _ = self.v4.send_to(buf, addr);
            }
            SocketAddr::V6(a) => {
                if let Some(m) = a.ip().to_ipv4_mapped() {
                    let _ = self.v4.send_to(buf, SocketAddr::V4(SocketAddrV4::new(m, a.port())));
                } else if let Some(s) = &self.v6 {
                    let _ = s.send_to(buf, addr);
                }
            }
        }
    }

    /// Eigene Adressen auf dem Weg nach draussen (ohne zu senden): Quelle des Standardwegs.
    pub fn local_addrs(&self) -> Vec<SocketAddr> {
        let mut out = Vec::new();
        if let Ok(s) = UdpSocket::bind("0.0.0.0:0") {
            if s.connect("8.8.8.8:53").is_ok() {
                if let Ok(a) = s.local_addr() {
                    out.push(SocketAddr::new(a.ip(), self.port));
                }
            }
        }
        if self.v6.is_some() {
            if let Ok(s) = UdpSocket::bind("[::]:0") {
                if s.connect("[2001:4860:4860::8888]:53").is_ok() {
                    if let Ok(a) = s.local_addr() {
                        if classify(a.ip()) == PATH_V6 {
                            out.push(SocketAddr::new(a.ip(), self.port));
                        }
                    }
                }
            }
        }
        out
    }
}

/// Wird im Aufnahme-Callback benutzt: baut ein Audio-Paket und schickt es an alle Teilnehmer,
/// direkt oder als RELAY über den Hub.
pub struct Wire {
    sockets: Arc<Sockets>,
    shared: Arc<Shared>,
    buf: Vec<u8>,
    relay: Vec<u8>,
    encoder: Option<codec::Encoder>,
    opus_acc: Vec<i16>,
    opus_pkt: Vec<u8>,
}

impl Wire {
    pub fn new(sockets: Arc<Sockets>, shared: Arc<Shared>) -> Self {
        Wire {
            sockets,
            shared,
            buf: Vec::with_capacity(HEADER + 2 + 2 * 1024 + 16),
            relay: Vec::with_capacity(HEADER + 8 + 2 * 1024 + 40),
            encoder: None,
            opus_acc: Vec::with_capacity(codec::FRAME * 2),
            opus_pkt: Vec::with_capacity(codec::MAX_PACKET),
        }
    }

    /// Baut ein AUDIO-Paket in `self.buf` und verschickt es an die Ziele (direkt und per Relay).
    #[allow(clippy::too_many_arguments)]
    fn ship(&mut self, seq: u32, muted: bool, payload_head: &[u8], data: Option<&[u8]>, direct: &[Option<SocketAddr>], relayed: &[Option<u64>]) {
        let room = self.shared.room.try_read().ok().and_then(|r| r.clone());
        let mut flags = if muted { F_MUTED } else { 0 };
        if room.is_some() {
            flags |= F_ENC;
        }
        let h = Header { typ: T_AUDIO, flags, sender: self.shared.peer_id, seq, ts: self.shared.now_us() };
        self.buf.clear();
        write_header(&mut self.buf, &h);
        let mut payload = Vec::with_capacity(payload_head.len() + data.map(|d| d.len()).unwrap_or(0) + 16);
        payload.extend_from_slice(payload_head);
        if let Some(d) = data {
            payload.extend_from_slice(d);
        }
        if let Some(r) = &room {
            if !r.seal(&self.buf[..HEADER], &nonce(h.sender, h.seq), &mut payload) {
                return;
            }
        }
        self.buf.extend_from_slice(&payload);
        for t in direct.iter().flatten() {
            self.sockets.send_to(&self.buf, *t);
        }
        let (hub4, hub6) = self.shared.hub_addrs();
        if let Some(hub) = hub4.or(hub6) {
            for dest in relayed.iter().flatten() {
                self.relay.clear();
                write_header(&mut self.relay, &Header { typ: T_RELAY, flags: 0, sender: self.shared.peer_id, seq, ts: h.ts });
                self.relay.extend_from_slice(&dest.to_le_bytes());
                self.relay.extend_from_slice(&self.buf);
                self.sockets.send_to(&self.relay, hub);
            }
        }
    }

    /// Ein 5-ms-Rahmen vom Mikrofon. LAN-Peers bekommen ihn sofort als PCM;
    /// für Ferne werden zwei Rahmen zu 10 ms Opus gebündelt (oder PCM, wenn so gewählt).
    pub fn send_frame(&mut self, samples: &[i16]) {
        let kbps = self.shared.codec_kbps.load(Relaxed);
        let mut pcm_direct: [Option<SocketAddr>; MAX_PEERS] = [None; MAX_PEERS];
        let mut pcm_relay: [Option<u64>; MAX_PEERS] = [None; MAX_PEERS];
        let mut opus_direct: [Option<SocketAddr>; MAX_PEERS] = [None; MAX_PEERS];
        let mut opus_relay: [Option<u64>; MAX_PEERS] = [None; MAX_PEERS];
        let mut any_pcm = false;
        let mut any_opus = false;
        for (i, p) in self.shared.peers.iter().enumerate() {
            if !p.active.load(Relaxed) {
                continue;
            }
            let lan = p.path.load(Relaxed) == PATH_LAN;
            let use_pcm = lan || kbps == 0;
            if p.via_relay.load(Relaxed) {
                let id = p.id.load(Relaxed);
                if use_pcm {
                    pcm_relay[i] = Some(id);
                    any_pcm = true;
                } else {
                    opus_relay[i] = Some(id);
                    any_opus = true;
                }
            } else if let Ok(a) = p.addr.try_lock() {
                if let Some(a) = *a {
                    if use_pcm {
                        pcm_direct[i] = Some(a);
                        any_pcm = true;
                    } else {
                        opus_direct[i] = Some(a);
                        any_opus = true;
                    }
                }
            }
        }
        let muted = self.shared.muted.load(Relaxed);
        if any_pcm {
            let seq = self.shared.tx_seq.fetch_add(1, Relaxed) & !SEQ_OPUS;
            let frame_ms = (samples.len() as u32 * 1000 / RATE) as u8;
            let mut data = Vec::with_capacity(2 * samples.len());
            if !muted {
                for s in samples {
                    data.extend_from_slice(&s.to_le_bytes());
                }
            }
            self.ship(seq, muted, &[CODEC_PCM, frame_ms], Some(&data), &pcm_direct, &pcm_relay);
        }
        if !any_opus {
            self.opus_acc.clear();
            return;
        }
        self.opus_acc.extend_from_slice(samples);
        if self.opus_acc.len() < codec::FRAME {
            return;
        }
        if self.encoder.is_none() {
            self.encoder = codec::Encoder::new(kbps);
        }
        let Some(enc) = self.encoder.as_mut() else { return };
        enc.set_kbps(kbps);
        let ok = if muted { false } else { enc.encode(&self.opus_acc[..codec::FRAME], &mut self.opus_pkt) };
        self.opus_acc.drain(..codec::FRAME);
        let seq = SEQ_OPUS | (self.shared.tx_seq_opus.fetch_add(1, Relaxed) & !SEQ_OPUS);
        let head = [CODEC_OPUS, 10, kbps.min(255) as u8];
        let pkt = std::mem::take(&mut self.opus_pkt);
        self.ship(seq, muted || !ok, &head, if ok { Some(&pkt) } else { None }, &opus_direct, &opus_relay);
        self.opus_pkt = pkt;
    }
}

pub struct NetCfg {
    pub peers: Vec<SocketAddr>,
}

struct Rx {
    last_seq: Option<u32>,
    last_arrival_us: u32,
    last_ns: usize,
    last_frame: Vec<i16>,
    jitter: VecDeque<(u64, u32)>,
    gaps: VecDeque<(u64, u32)>,
    received: VecDeque<u64>,
    prev_underruns: u32,
    last_underrun_ms: u64,
    last_shrink_ms: u64,
    /// Kleinster Füllstand seit dem letzten Regeltakt, für die Driftregelung.
    min_fill: usize,
    pads: u32,
}

impl Rx {
    fn new(frame: usize) -> Self {
        Rx {
            last_seq: None,
            last_arrival_us: 0,
            last_ns: frame,
            last_frame: Vec::new(),
            jitter: VecDeque::new(),
            gaps: VecDeque::new(),
            received: VecDeque::new(),
            prev_underruns: 0,
            last_underrun_ms: 0,
            last_shrink_ms: 0,
            min_fill: usize::MAX,
            pads: 0,
        }
    }

    fn reset(&mut self, frame: usize, now: u64) {
        *self = Rx::new(frame);
        self.last_underrun_ms = now;
    }
}

struct Slot {
    producer: rtrb::Producer<i16>,
    capacity: usize,
    rx: Rx,
    decoder: Option<codec::Decoder>,
    codec: u8,
}

/// Empfangsseite aller Plätze; nur die Netz-Threads sperren hier.
pub struct Table {
    slots: Vec<Slot>,
}

impl Table {
    pub fn new(producers: Vec<rtrb::Producer<i16>>, frame: usize) -> Arc<Mutex<Table>> {
        let slots = producers
            .into_iter()
            .map(|p| {
                let capacity = p.buffer().capacity();
                Slot { producer: p, capacity, rx: Rx::new(frame), decoder: None, codec: CODEC_PCM }
            })
            .collect();
        Arc::new(Mutex::new(Table { slots }))
    }
}

pub fn spawn(sockets: Arc<Sockets>, shared: Arc<Shared>, table: Arc<Mutex<Table>>, cfg: NetCfg) {
    if let Ok(mut m) = shared.manual_peers.lock() {
        *m = cfg.peers.clone();
    }
    let cfg = Arc::new(cfg);
    {
        let (s, sh, t, c) = (sockets.clone(), shared.clone(), table.clone(), cfg.clone());
        std::thread::Builder::new()
            .name("holler-net4".into())
            .spawn(move || run(s.v4.clone(), s, sh, t, c, true))
            .expect("Netz-Thread");
    }
    if let Some(v6) = sockets.v6.clone() {
        shared.ipv6.store(true, Relaxed);
        std::thread::Builder::new()
            .name("holler-net6".into())
            .spawn(move || run(v6, sockets, shared, table, cfg, false))
            .expect("Netz-Thread v6");
    }
}

fn run(socket: Arc<UdpSocket>, sockets: Arc<Sockets>, shared: Arc<Shared>, table: Arc<Mutex<Table>>, cfg: Arc<NetCfg>, ticker: bool) {
    let mut buf = [0u8; 4096];
    let mut last_hello: u64 = 0;
    let mut last_stats: u64 = 0;
    let mut last_join: u64 = 0;
    let mut last_probe: u64 = 0;
    loop {
        match socket.recv_from(&mut buf) {
            Ok((n, src)) => {
                let mut t = match table.lock() {
                    Ok(t) => t,
                    Err(p) => p.into_inner(),
                };
                handle(&buf[..n], src, &sockets, &shared, &mut t);
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::ConnectionReset) => {}
            Err(e) => {
                if let Ok(mut ne) = shared.net_error.lock() {
                    *ne = Some(format!("Netzfehler: {e}"));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        if !ticker {
            continue;
        }
        let now = shared.now_ms();
        let interval = if shared.connected() { 5000 } else { 1000 };
        if now.saturating_sub(last_hello) >= interval {
            last_hello = now;
            send_hello(&sockets, &shared, &cfg);
        }
        // Hub: JOIN alle 5 s (2 s bis zur ersten Antwort), zusammen mit PING für die Laufzeit.
        if shared.room().is_some() && shared.hub_configured() {
            let join_interval = if shared.hub_alive() { 5000 } else { 2000 };
            if now.saturating_sub(last_join) >= join_interval {
                last_join = now;
                send_join(&sockets, &shared);
            }
        }
        if now.saturating_sub(last_probe) >= 500 {
            last_probe = now;
            probe_tick(&sockets, &shared, now);
        }
        if now.saturating_sub(last_stats) >= 500 {
            last_stats = now;
            let mut t = match table.lock() {
                Ok(t) => t,
                Err(p) => p.into_inner(),
            };
            stats_and_control(&shared, &mut t, now);
        }
    }
}

fn hello_packet(shared: &Shared, port: u16) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(HEADER + 3 + ROOM_ID_LEN + 32);
    write_header(&mut pkt, &Header { typ: T_HELLO, flags: 0, sender: shared.peer_id, seq: 0, ts: shared.now_us() });
    pkt.push(VERSION);
    pkt.extend_from_slice(&port.to_le_bytes());
    pkt.extend_from_slice(&shared.room_id());
    let name = shared.name();
    let name = name.as_bytes();
    pkt.extend_from_slice(&name[..name.len().min(32)]);
    pkt
}

fn send_hello(sockets: &Sockets, shared: &Shared, _cfg: &NetCfg) {
    let pkt = hello_packet(shared, sockets.port);
    let _ = sockets.v4.send_to(&pkt, SocketAddrV4::new(Ipv4Addr::BROADCAST, sockets.port));
    let manual: Vec<SocketAddr> = shared.manual_peers.lock().map(|m| m.clone()).unwrap_or_default();
    for a in manual {
        sockets.send_to(&pkt, a);
    }
    for p in &shared.peers {
        if p.active.load(Relaxed) && !p.via_relay.load(Relaxed) {
            if let Some(a) = p.addr() {
                sockets.send_to(&pkt, a);
            }
        }
    }
}

fn send_join(sockets: &Sockets, shared: &Shared) {
    let mut pkt = Vec::with_capacity(HEADER + 40 + 64);
    write_header(&mut pkt, &Header { typ: T_JOIN, flags: 0, sender: shared.peer_id, seq: 0, ts: shared.now_us() });
    pkt.push(VERSION);
    pkt.extend_from_slice(&shared.room_id());
    let name = shared.name();
    let name = name.as_bytes();
    let n = name.len().min(32);
    pkt.push(n as u8);
    pkt.extend_from_slice(&name[..n]);
    let locals = sockets.local_addrs();
    pkt.push(locals.len() as u8);
    for a in &locals {
        push_addr(&mut pkt, *a);
    }
    let mut ping = Vec::with_capacity(HEADER);
    write_header(&mut ping, &Header { typ: T_PING, flags: 0, sender: shared.peer_id, seq: 0, ts: shared.now_us() });
    let (h4, h6) = shared.hub_addrs();
    if let Some(a) = h4 {
        sockets.send_to(&pkt, a);
        sockets.send_to(&ping, a);
    }
    if let Some(a) = h6 {
        sockets.send_to(&pkt, a);
        if h4.is_none() {
            sockets.send_to(&ping, a);
        }
    }
}

fn probe_packet(shared: &Shared, typ: u8) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(HEADER + ROOM_ID_LEN);
    write_header(&mut pkt, &Header { typ, flags: 0, sender: shared.peer_id, seq: 0, ts: shared.now_us() });
    pkt.extend_from_slice(&shared.room_id());
    pkt
}

/// Alle 500 ms: Direktwege suchen, halten, aufgeben.
fn probe_tick(sockets: &Sockets, shared: &Shared, now: u64) {
    let probe = probe_packet(shared, T_PROBE);
    let hub_ok = shared.hub_configured() && shared.hub_alive();
    let force_relay = shared.force_relay.load(Relaxed);
    for p in &shared.peers {
        if !p.active.load(Relaxed) {
            continue;
        }
        let candidates: Vec<SocketAddr> = p.candidates.lock().map(|c| c.clone()).unwrap_or_default();
        if candidates.is_empty() {
            continue; // reiner LAN-Teilnehmer (per HELLO gefunden), nichts zu tun
        }
        let joined = p.joined_ms.load(Relaxed);
        let last_ack = p.last_ack_ms.load(Relaxed);
        let has_direct = p.addr().is_some() && !p.via_relay.load(Relaxed);
        let age = now.saturating_sub(joined);
        if force_relay {
            if !p.via_relay.load(Relaxed) && hub_ok {
                p.via_relay.store(true, Relaxed);
                p.path.store(PATH_RELAY, Relaxed);
            }
            continue;
        }
        // Direktweg tot? Zurück auf Relay und neu suchen.
        if has_direct && last_ack > 0 && now.saturating_sub(last_ack) > DIRECT_DEAD_MS {
            eprintln!("Direktweg zu {} verloren, Relay", p.name());
            if let Ok(mut a) = p.addr.lock() {
                *a = None;
            }
            if hub_ok {
                p.via_relay.store(true, Relaxed);
                p.path.store(PATH_RELAY, Relaxed);
            }
        }
        // Noch kein Direktweg nach 2 s: Relay, weiter suchen.
        if !has_direct && !p.via_relay.load(Relaxed) && age > RELAY_AFTER_MS && hub_ok {
            p.via_relay.store(true, Relaxed);
            p.path.store(PATH_RELAY, Relaxed);
        }
        // Suchen: erste 2 s alle 500 ms an alle Kandidaten, danach alle 5 s; bestehender Weg alle 5 s.
        let last_probe = p.last_probe_ms.load(Relaxed);
        let due = if has_direct || age > RELAY_AFTER_MS { now.saturating_sub(last_probe) >= 5000 } else { true };
        if !due {
            continue;
        }
        p.last_probe_ms.store(now, Relaxed);
        let current = p.addr();
        for c in &candidates {
            // Bestehenden Weg halten; bessere Kandidaten (LAN vor v6 vor v4) weiter versuchen.
            if let Some(cur) = current {
                if *c != cur && prio(*c) >= prio(cur) {
                    continue;
                }
            }
            sockets.send_to(&probe, *c);
        }
    }
}

pub fn send_leave(sockets: &Sockets, shared: &Shared) {
    let mut pkt = Vec::with_capacity(HEADER);
    write_header(&mut pkt, &Header { typ: T_LEAVE, flags: 0, sender: shared.peer_id, seq: 0, ts: shared.now_us() });
    for p in &shared.peers {
        if p.active.load(Relaxed) {
            if let Some(a) = p.addr() {
                sockets.send_to(&pkt, a);
            }
        }
    }
    let (h4, h6) = shared.hub_addrs();
    for a in [h4, h6].into_iter().flatten() {
        sockets.send_to(&pkt, a);
    }
}

/// Teilnehmer finden oder anlegen. None = Liste voll.
fn peer_slot(shared: &Shared, table: &mut Table, id: u64, name: &str, addr: Option<SocketAddr>, now: u64) -> Option<usize> {
    if let Some(i) = shared.find_peer(id) {
        return Some(i);
    }
    let i = shared.free_slot()?;
    let path = addr.map(|a| classify(a.ip())).unwrap_or(crate::state::PATH_UNKNOWN);
    shared.peers[i].assign(id, name, addr, path, now, shared.initial_target(), shared.default_volume_f());
    table.slots[i].rx.reset(shared.frame_samples.load(Relaxed) as usize, now);
    table.slots[i].decoder = None;
    table.slots[i].codec = CODEC_PCM;
    shared.room_full.store(false, Relaxed);
    eprintln!(
        "Teilnehmer {name} auf Platz {i}, {}",
        addr.map(|a| format!("{a}, Weg {}", crate::state::path_name(path))).unwrap_or_else(|| "Adresse über Hub".into())
    );
    Some(i)
}

/// Bestätigten Direktweg übernehmen, wenn er besser ist als der aktuelle.
fn adopt_direct(shared: &Shared, i: usize, src: SocketAddr, now: u64) {
    let p = &shared.peers[i];
    let cur = p.addr();
    let better = match cur {
        None => true,
        Some(c) => c == src || prio(src) < prio(c) || p.via_relay.load(Relaxed),
    };
    if !better {
        return;
    }
    if cur != Some(src) || p.via_relay.load(Relaxed) {
        eprintln!("Direktweg zu {}: {src} ({})", p.name(), crate::state::path_name(classify(src.ip())));
    }
    if let Ok(mut a) = p.addr.lock() {
        *a = Some(src);
    }
    p.via_relay.store(false, Relaxed);
    p.path.store(classify(src.ip()), Relaxed);
    p.last_ack_ms.store(now, Relaxed);
}

fn handle(pkt: &[u8], src: SocketAddr, sockets: &Sockets, shared: &Shared, table: &mut Table) {
    let Some(h) = parse_header(pkt) else { return };
    if h.sender == shared.peer_id {
        return;
    }
    let body = &pkt[HEADER..];
    let now_ms = shared.now_ms();
    let from_hub = shared.is_hub_addr(src);
    match h.typ {
        T_HELLO => {
            if body.len() < 3 + ROOM_ID_LEN || body[0] != VERSION {
                return;
            }
            let port = u16::from_le_bytes([body[1], body[2]]);
            let room_id: [u8; ROOM_ID_LEN] = body[3..3 + ROOM_ID_LEN].try_into().unwrap_or([0; ROOM_ID_LEN]);
            if room_id != shared.room_id() {
                shared.foreign_room.fetch_add(1, Relaxed);
                return;
            }
            let name = String::from_utf8_lossy(&body[3 + ROOM_ID_LEN..]).trim().to_string();
            let addr = SocketAddr::new(src.ip(), port);
            match peer_slot(shared, table, h.sender, &name, Some(addr), now_ms) {
                Some(i) => {
                    let p = &shared.peers[i];
                    p.last_seen_ms.store(now_ms, Relaxed);
                    if let Ok(mut n) = p.name.lock() {
                        if !name.is_empty() {
                            *n = name;
                        }
                    }
                    // HELLO kommt nur aus dem LAN: bester Weg, sofort nehmen.
                    adopt_direct(shared, i, addr, now_ms);
                }
                None => shared.room_full.store(true, Relaxed),
            }
        }
        T_PROBE | T_PROBE_ACK => {
            if body.len() < ROOM_ID_LEN || body[..ROOM_ID_LEN] != shared.room_id() {
                return;
            }
            let Some(i) = shared.find_peer(h.sender) else { return };
            let p = &shared.peers[i];
            p.last_seen_ms.store(now_ms, Relaxed);
            if h.typ == T_PROBE {
                let ack = probe_packet(shared, T_PROBE_ACK);
                sockets.send_to(&ack, src);
                // Wenn uns ein Probe erreicht, erreicht unseres wohl auch: gleich mitprobieren.
                if p.addr().is_none() || p.via_relay.load(Relaxed) {
                    let probe = probe_packet(shared, T_PROBE);
                    sockets.send_to(&probe, src);
                }
            } else {
                adopt_direct(shared, i, src, now_ms);
            }
        }
        T_MEMBERS => {
            if !from_hub || body.is_empty() {
                return;
            }
            shared.hub_last_ms.store(now_ms, Relaxed);
            let mine = sockets.local_addrs();
            let count = body[0] as usize;
            let mut p = 1;
            for _ in 0..count {
                if body.len() < p + 8 + 1 {
                    return;
                }
                let id = u64::from_le_bytes(body[p..p + 8].try_into().unwrap_or([0; 8]));
                p += 8;
                let nl = body[p] as usize;
                p += 1;
                if body.len() < p + nl + 1 {
                    return;
                }
                let name = String::from_utf8_lossy(&body[p..p + nl]).trim().to_string();
                p += nl;
                let na = body[p] as usize;
                p += 1;
                let mut public = Vec::new();
                let mut locals = Vec::new();
                for _ in 0..na {
                    if body.len() < p + 1 {
                        return;
                    }
                    let kind = body[p];
                    p += 1;
                    let Some((a, used)) = read_addr(&body[p..]) else { return };
                    p += used;
                    if kind == 1 {
                        public.push(a);
                    } else {
                        locals.push(a);
                    }
                }
                if id == shared.peer_id {
                    if let Ok(mut g) = shared.public_addr.lock() {
                        *g = public.first().copied();
                    }
                    continue;
                }
                let Some(i) = peer_slot(shared, table, id, &name, None, now_ms) else {
                    shared.room_full.store(true, Relaxed);
                    continue;
                };
                let peer = &shared.peers[i];
                peer.last_seen_ms.store(now_ms, Relaxed);
                if let Ok(mut n) = peer.name.lock() {
                    if !name.is_empty() {
                        *n = name;
                    }
                }
                if let Ok(mut c) = peer.candidates.lock() {
                    // Reihenfolge: lokale Adressen zuerst (LAN), dann öffentliche.
                    let mut list: Vec<SocketAddr> = locals.iter().chain(public.iter()).copied().collect();
                    list.dedup();
                    // Eigene Adressen sind keine Kandidaten (Hub im selben Netz meldet uns unsere eigenen).
                    list.retain(|a| !mine.contains(a));
                    if list != *c {
                        *c = list;
                        peer.last_probe_ms.store(0, Relaxed);
                    }
                }
            }
        }
        T_AUDIO => {
            if body.len() < 2 {
                return;
            }
            let room = shared.room();
            let mut payload = body.to_vec();
            match (&room, h.flags & F_ENC != 0) {
                (Some(r), true) => {
                    if !r.open(&pkt[..HEADER], &nonce(h.sender, h.seq), &mut payload) {
                        shared.bad_auth.fetch_add(1, Relaxed);
                        return;
                    }
                }
                (None, false) => {}
                _ => {
                    shared.bad_auth.fetch_add(1, Relaxed);
                    return;
                }
            }
            if payload.len() < 2 || (payload[0] != CODEC_PCM && payload[0] != CODEC_OPUS) {
                return;
            }
            let in_codec = payload[0];
            let Some(i) = peer_slot(shared, table, h.sender, "", if from_hub { None } else { Some(src) }, now_ms) else {
                shared.room_full.store(true, Relaxed);
                return;
            };
            let p = &shared.peers[i];
            let muted = h.flags & F_MUTED != 0;
            let now_us = shared.now_us();
            p.remote_muted.store(muted, Relaxed);
            p.last_rx_ms.store(now_ms, Relaxed);
            p.last_seen_ms.store(now_ms, Relaxed);
            if from_hub {
                if p.addr().is_none() {
                    p.path.store(PATH_RELAY, Relaxed);
                }
            } else if p.addr().is_none() && !p.via_relay.load(Relaxed) {
                // Direktes Audio ohne vorherigen Handschlag (offenes LAN): Adresse übernehmen.
                adopt_direct(shared, i, src, now_ms);
            }
            if h.seq % 200 == 0 {
                let mut pong = Vec::with_capacity(HEADER);
                write_header(&mut pong, &Header { typ: T_PONG, flags: 0, sender: shared.peer_id, seq: 0, ts: h.ts });
                if from_hub {
                    // Antwort auf demselben Weg zurück: über den Hub.
                    let mut wrap = Vec::with_capacity(HEADER + 8 + HEADER);
                    write_header(&mut wrap, &Header { typ: T_RELAY, flags: 0, sender: shared.peer_id, seq: 0, ts: h.ts });
                    wrap.extend_from_slice(&h.sender.to_le_bytes());
                    wrap.extend_from_slice(&pong);
                    sockets.send_to(&wrap, src);
                } else {
                    sockets.send_to(&pong, src);
                }
            }

            let slot = &mut table.slots[i];
            if slot.codec != in_codec {
                // Codec-Wechsel: eigener Sequenzraum, daher Zähler neu ansetzen.
                slot.codec = in_codec;
                slot.rx.last_seq = None;
                slot.rx.last_frame.clear();
                p.codec.store(in_codec, Relaxed);
            }
            let mut decoded: Vec<i16> = Vec::new();
            let samples: &[i16] = if in_codec == CODEC_OPUS {
                if payload.len() < 3 {
                    return;
                }
                p.codec_kbps.store(payload[2] as u32, Relaxed);
                if slot.decoder.is_none() {
                    slot.decoder = codec::Decoder::new();
                }
                let Some(dec) = slot.decoder.as_mut() else { return };
                if !muted && payload.len() > 3 {
                    if !dec.decode(&payload[3..], &mut decoded) {
                        return;
                    }
                }
                &decoded
            } else {
                p.codec_kbps.store(0, Relaxed);
                &[]
            };
            let rx = &mut slot.rx;
            let raw = &payload[2..];
            let ns = if muted {
                if in_codec == CODEC_OPUS { codec::FRAME } else { rx.last_ns }
            } else if in_codec == CODEC_OPUS {
                samples.len()
            } else {
                raw.len() / 2
            };
            if ns == 0 {
                return;
            }
            if !muted {
                rx.last_ns = ns;
                p.frame_samples.store(ns as u32, Relaxed);
            }
            if let Some(ls) = rx.last_seq {
                let d = h.seq.wrapping_sub(ls) as i32;
                if d > 0 {
                    let expected = d as u64 * ns as u64 * 1_000_000 / RATE as u64;
                    let actual = now_us.wrapping_sub(rx.last_arrival_us) as u64;
                    rx.jitter.push_back((now_ms, actual.abs_diff(expected).min(1_000_000) as u32));
                }
            }
            rx.last_arrival_us = now_us;
            if let Some(ls) = rx.last_seq {
                let d = h.seq.wrapping_sub(ls) as i32;
                if d <= 0 {
                    return;
                }
                if d > 1 {
                    let gap = (d - 1) as u32;
                    rx.gaps.push_back((now_ms, gap));
                    if gap <= MAX_GAP_FILL {
                        let mut plc: Vec<i16> = Vec::new();
                        for g in 0..gap {
                            if in_codec == CODEC_OPUS && g < 2 && slot.decoder.as_mut().is_some_and(|d| d.conceal(&mut plc)) {
                                for &s in &plc {
                                    let _ = slot.producer.push(s);
                                }
                            } else if g == 0 && !rx.last_frame.is_empty() {
                                for &s in &rx.last_frame {
                                    let _ = slot.producer.push(s / 2);
                                }
                            } else {
                                for _ in 0..ns {
                                    let _ = slot.producer.push(0);
                                }
                            }
                        }
                    }
                }
            }
            rx.last_seq = Some(h.seq);
            rx.received.push_back(now_ms);

            let target = p.target_frames.load(Relaxed) as usize;
            let fill = slot.capacity - slot.producer.slots();
            rx.min_fill = rx.min_fill.min(fill);
            // Überlauf erst deutlich über dem Ziel: Jitter-Schübe sollen den Puffer
            // wachsen lassen, nicht Pakete kosten. Zurückgeregelt wird per Drift-Skip.
            if fill + ns > (target + 4) * ns {
                p.dropped.fetch_add(1, Relaxed);
                return;
            }
            rx.last_frame.clear();
            if muted {
                for _ in 0..ns {
                    let _ = slot.producer.push(0);
                }
            } else if in_codec == CODEC_OPUS {
                for &s in samples {
                    rx.last_frame.push(s);
                    let _ = slot.producer.push(s);
                }
            } else {
                for c in raw.chunks_exact(2) {
                    let s = i16::from_le_bytes([c[0], c[1]]);
                    rx.last_frame.push(s);
                    let _ = slot.producer.push(s);
                }
            }
            let fill = slot.capacity - slot.producer.slots();
            if p.priming.load(Relaxed) && fill >= target * ns {
                p.priming.store(false, Relaxed);
            }
        }
        T_PONG => {
            let rtt = shared.now_us().wrapping_sub(h.ts).min(5_000_000);
            if from_hub && h.sender == 0 {
                shared.hub_rtt_us.store(rtt, Relaxed);
                shared.hub_last_ms.store(now_ms, Relaxed);
            } else if let Some(i) = shared.find_peer(h.sender) {
                shared.peers[i].rtt_us.store(rtt, Relaxed);
            }
        }
        T_LEAVE => {
            if let Some(i) = shared.find_peer(h.sender) {
                eprintln!("Teilnehmer {} hat den Raum verlassen", shared.peers[i].name());
                shared.peers[i].clear();
            }
        }
        _ => {}
    }
}

fn stats_and_control(shared: &Shared, table: &mut Table, now: u64) {
    let auto = shared.jitter_auto.load(Relaxed);
    let fixed = shared.jitter_fixed.load(Relaxed);
    for (i, p) in shared.peers.iter().enumerate() {
        if !p.active.load(Relaxed) {
            continue;
        }
        if now.saturating_sub(p.last_seen_ms.load(Relaxed)) > crate::state::GONE_MS {
            eprintln!("Teilnehmer {} weg (keine Lebenszeichen)", p.name());
            p.clear();
            continue;
        }
        let slot = &mut table.slots[i];
        let rx = &mut slot.rx;
        let horizon = now.saturating_sub(5000);
        while rx.jitter.front().is_some_and(|(t, _)| *t < horizon) {
            rx.jitter.pop_front();
        }
        while rx.gaps.front().is_some_and(|(t, _)| *t < horizon) {
            rx.gaps.pop_front();
        }
        while rx.received.front().is_some_and(|t| *t < horizon) {
            rx.received.pop_front();
        }
        let p95 = if rx.jitter.is_empty() {
            0
        } else {
            let mut v: Vec<u32> = rx.jitter.iter().map(|(_, d)| *d).collect();
            v.sort_unstable();
            v[(v.len() * 95 / 100).min(v.len() - 1)]
        };
        p.jitter_us.store(p95, Relaxed);
        let lost: u64 = rx.gaps.iter().map(|(_, g)| *g as u64).sum();
        let got = rx.received.len() as u64;
        p.loss_permille.store(if lost + got == 0 { 0 } else { (lost * 1000 / (lost + got)) as u32 }, Relaxed);

        // Driftregelung: läuft der Ring fast leer, einen Rahmen vorab doppelt einsetzen;
        // ist er dauerhaft übervoll, einen Rahmen verwerfen. Beides ohne Aussetzer.
        let ns = rx.last_ns.max(1);
        let min_fill = std::mem::replace(&mut rx.min_fill, usize::MAX);
        let target_now = p.target_frames.load(Relaxed) as usize;
        if min_fill != usize::MAX && p.streaming(now) && !p.priming.load(Relaxed) {
            if min_fill < ns / 2 {
                if rx.last_frame.is_empty() {
                    for _ in 0..ns {
                        let _ = slot.producer.push(0);
                    }
                } else {
                    for &s in &rx.last_frame {
                        let _ = slot.producer.push(s / 2);
                    }
                }
                rx.pads += 1;
            } else if min_fill > (target_now + 1) * ns {
                p.skip_samples.fetch_add(ns as u32, Relaxed);
            }
        }
        let mut target = p.target_frames.load(Relaxed);
        if !auto {
            p.target_frames.store(fixed.clamp(MIN_TARGET, MAX_TARGET), Relaxed);
            continue;
        }
        let und = p.underruns.load(Relaxed);
        if und != rx.prev_underruns {
            rx.prev_underruns = und;
            rx.last_underrun_ms = now;
            if p.streaming(now) {
                target = (target + 1).min(MAX_TARGET);
            }
        } else {
            let frame_us = (rx.last_ns as u64 * 1_000_000 / RATE as u64).max(1);
            let desired = (((p95 as u64 + frame_us - 1) / frame_us) as u32 + 1).clamp(2, MAX_TARGET);
            if desired > target {
                target = desired;
            } else if desired < target
                && now.saturating_sub(rx.last_underrun_ms) > 30_000
                && now.saturating_sub(rx.last_shrink_ms) > 30_000
                && (p.remote_muted.load(Relaxed) || p.level_f() < 0.01)
            {
                target -= 1;
                rx.last_shrink_ms = now;
                p.skip_samples.fetch_add(rx.last_ns as u32, Relaxed);
            }
        }
        p.target_frames.store(target.clamp(MIN_TARGET, MAX_TARGET), Relaxed);
    }
}

/// „ip“ oder „ip:port“ oder „[v6]:port“ → Adresse; Port fehlt → Vorgabe.
pub fn parse_peer(s: &str, default_port: u16) -> Result<SocketAddr, String> {
    let s = s.trim();
    if let Ok(a) = s.parse::<SocketAddr>() {
        return Ok(a);
    }
    if let Ok(ip) = s.trim_start_matches('[').trim_end_matches(']').parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, default_port));
    }
    Err(format!("Keine gültige Adresse: {s} (erwartet 192.168.1.5, 192.168.1.5:4711 oder [fe80::1]:4711)"))
}
