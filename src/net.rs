//! Drahtprotokoll v2: UDP, symmetrisch, IPv4 und IPv6, bis 8 Teilnehmer.
//!
//! Kopf (Klartext, 20 Byte): "H2", Typ, Flags, Absender-Kennung u64, Sequenz u32, Zeit µs u32.
//! Nutzlast: bei Raum verschlüsselt (ChaCha20-Poly1305, Nonce = Kennung+Sequenz, Kopf als AAD).
//!   HELLO: [version=2][port u16][raum-id 32][name utf8]         Suche per Broadcast, Klartext
//!   AUDIO: [codec][rahmen ms][pcm i16 * n]                        ein Rahmen pro Paket
//!   PONG:  leer; das Zeitfeld trägt den Zeitstempel des Fragenden  Laufzeitmessung
//!   LEAVE: leer
//! Flags: Bit 0 Sender stumm (dann keine Samples), Bit 2 Nutzlast verschlüsselt.

use crate::crypto::ROOM_ID_LEN;
use crate::state::{classify, Shared, MAX_PEERS, MAX_TARGET, MIN_TARGET, RATE};
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
const T_LEAVE: u8 = 6;
const F_MUTED: u8 = 1;
const F_ENC: u8 = 4;
const MAX_GAP_FILL: u32 = 8;

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
}

/// Wird im Aufnahme-Callback benutzt: baut ein Audio-Paket und schickt es an alle Teilnehmer.
pub struct Wire {
    sockets: Arc<Sockets>,
    shared: Arc<Shared>,
    buf: Vec<u8>,
}

impl Wire {
    pub fn new(sockets: Arc<Sockets>, shared: Arc<Shared>) -> Self {
        Wire { sockets, shared, buf: Vec::with_capacity(HEADER + 2 + 2 * 1024 + 16) }
    }

    pub fn send_frame(&mut self, samples: &[i16]) {
        let seq = self.shared.tx_seq.fetch_add(1, Relaxed);
        let mut targets: [Option<SocketAddr>; MAX_PEERS] = [None; MAX_PEERS];
        let mut any = false;
        for (i, p) in self.shared.peers.iter().enumerate() {
            if p.active.load(Relaxed) {
                if let Ok(a) = p.addr.try_lock() {
                    targets[i] = *a;
                    any |= a.is_some();
                }
            }
        }
        if !any {
            return;
        }
        let muted = self.shared.muted.load(Relaxed);
        let room = self.shared.room.try_read().ok().and_then(|r| r.clone());
        let mut flags = if muted { F_MUTED } else { 0 };
        if room.is_some() {
            flags |= F_ENC;
        }
        let h = Header { typ: T_AUDIO, flags, sender: self.shared.peer_id, seq, ts: self.shared.now_us() };
        self.buf.clear();
        write_header(&mut self.buf, &h);
        let frame_ms = (samples.len() as u32 * 1000 / RATE) as u8;
        let mut payload = Vec::with_capacity(2 + 2 * samples.len() + 16);
        payload.push(crate::state::CODEC_PCM);
        payload.push(frame_ms);
        if !muted {
            for s in samples {
                payload.extend_from_slice(&s.to_le_bytes());
            }
        }
        if let Some(r) = &room {
            if !r.seal(&self.buf[..HEADER], &nonce(h.sender, h.seq), &mut payload) {
                return;
            }
        }
        self.buf.extend_from_slice(&payload);
        for t in targets.iter().flatten() {
            self.sockets.send_to(&self.buf, *t);
        }
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
                Slot { producer: p, capacity, rx: Rx::new(frame) }
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
        if p.active.load(Relaxed) {
            if let Some(a) = p.addr() {
                sockets.send_to(&pkt, a);
            }
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
}

/// Teilnehmer finden oder anlegen. None = Liste voll.
fn peer_slot(shared: &Shared, table: &mut Table, id: u64, name: &str, addr: SocketAddr, now: u64) -> Option<usize> {
    if let Some(i) = shared.find_peer(id) {
        return Some(i);
    }
    let i = shared.free_slot()?;
    let path = classify(addr.ip());
    shared.peers[i].assign(id, name, addr, path, now, shared.initial_target(), shared.default_volume_f());
    table.slots[i].rx.reset(shared.frame_samples.load(Relaxed) as usize, now);
    shared.room_full.store(false, Relaxed);
    eprintln!("Teilnehmer {name} ({addr}) auf Platz {i}, Weg {}", crate::state::path_name(path));
    Some(i)
}

fn handle(pkt: &[u8], src: SocketAddr, sockets: &Sockets, shared: &Shared, table: &mut Table) {
    let Some(h) = parse_header(pkt) else { return };
    if h.sender == shared.peer_id {
        return;
    }
    let body = &pkt[HEADER..];
    let now_ms = shared.now_ms();
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
            match peer_slot(shared, table, h.sender, &name, addr, now_ms) {
                Some(i) => {
                    let p = &shared.peers[i];
                    p.last_seen_ms.store(now_ms, Relaxed);
                    if let Ok(mut n) = p.name.lock() {
                        if !name.is_empty() {
                            *n = name;
                        }
                    }
                    if p.addr() != Some(addr) {
                        if let Ok(mut a) = p.addr.lock() {
                            *a = Some(addr);
                        }
                        p.path.store(classify(addr.ip()), Relaxed);
                    }
                }
                None => shared.room_full.store(true, Relaxed),
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
            if payload.len() < 2 || payload[0] != crate::state::CODEC_PCM {
                return;
            }
            let Some(i) = peer_slot(shared, table, h.sender, "", src, now_ms) else {
                shared.room_full.store(true, Relaxed);
                return;
            };
            let p = &shared.peers[i];
            let muted = h.flags & F_MUTED != 0;
            let now_us = shared.now_us();
            p.remote_muted.store(muted, Relaxed);
            p.last_rx_ms.store(now_ms, Relaxed);
            p.last_seen_ms.store(now_ms, Relaxed);
            if p.addr() != Some(src) {
                if let Ok(mut a) = p.addr.lock() {
                    *a = Some(src);
                }
                p.path.store(classify(src.ip()), Relaxed);
            }
            if h.seq % 200 == 0 {
                let mut pong = Vec::with_capacity(HEADER);
                write_header(&mut pong, &Header { typ: T_PONG, flags: 0, sender: shared.peer_id, seq: 0, ts: h.ts });
                sockets.send_to(&pong, src);
            }

            let slot = &mut table.slots[i];
            let rx = &mut slot.rx;
            let samples = &payload[2..];
            let ns = if muted { rx.last_ns } else { samples.len() / 2 };
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
                        for g in 0..gap {
                            if g == 0 && !rx.last_frame.is_empty() {
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
            if fill + ns > (target + 2) * ns {
                p.dropped.fetch_add(1, Relaxed);
                return;
            }
            rx.last_frame.clear();
            if muted {
                for _ in 0..ns {
                    let _ = slot.producer.push(0);
                }
            } else {
                for c in samples.chunks_exact(2) {
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
            if let Some(i) = shared.find_peer(h.sender) {
                let rtt = shared.now_us().wrapping_sub(h.ts);
                shared.peers[i].rtt_us.store(rtt.min(5_000_000), Relaxed);
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
