//! Drahtprotokoll: UDP, symmetrisch, drei Pakettypen.
//!   HELLO 0x01: [typ, version, instanz-id u32, name utf8]        Suche per Broadcast
//!   AUDIO 0x02: [typ, flags, seq u16, ts u32, pcm i16 * n]       ein Rahmen pro Paket
//!   PONG  0x03: [typ, 0, 0, 0, ts u32]                            Laufzeitmessung
//! Alle Werte little-endian. Flags Bit 0 = Sender stumm (dann kein PCM).

use crate::state::{Shared, MAX_TARGET, MIN_TARGET, RATE};
use std::collections::VecDeque;
use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::time::Duration;

pub const VERSION: u8 = 1;
const T_HELLO: u8 = 0x01;
const T_AUDIO: u8 = 0x02;
const T_PONG: u8 = 0x03;
const HEADER: usize = 8;
const MAX_GAP_FILL: u32 = 8;

pub fn bind(port: u16) -> Result<Arc<UdpSocket>, String> {
    let sock = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port))
        .map_err(|e| format!("UDP-Port {port} belegt oder nicht bindbar: {e}"))?;
    sock.set_broadcast(true).map_err(|e| e.to_string())?;
    sock.set_read_timeout(Some(Duration::from_millis(100))).map_err(|e| e.to_string())?;
    Ok(Arc::new(sock))
}

/// Wird im Aufnahme-Callback benutzt: baut ein Audio-Paket und schickt es sofort ab.
pub struct Wire {
    socket: Arc<UdpSocket>,
    shared: Arc<Shared>,
    buf: Vec<u8>,
}

impl Wire {
    pub fn new(socket: Arc<UdpSocket>, shared: Arc<Shared>) -> Self {
        Wire { socket, shared, buf: Vec::with_capacity(HEADER + 2 * 1024) }
    }

    pub fn send_frame(&mut self, samples: &[i16]) {
        let seq = self.shared.tx_seq.fetch_add(1, Relaxed);
        let Some(addr) = self.shared.peer_addr() else { return };
        let muted = self.shared.muted.load(Relaxed);
        self.buf.clear();
        self.buf.push(T_AUDIO);
        self.buf.push(muted as u8);
        self.buf.extend_from_slice(&seq.to_le_bytes());
        self.buf.extend_from_slice(&self.shared.now_us().to_le_bytes());
        if !muted {
            for s in samples {
                self.buf.extend_from_slice(&s.to_le_bytes());
            }
        }
        let _ = self.socket.send_to(&self.buf, addr);
    }
}

pub struct NetCfg {
    pub port: u16,
    pub peer: Option<Ipv4Addr>,
    pub name: String,
}

struct Rx {
    last_seq: Option<u16>,
    last_arrival_us: u32,
    last_ns: usize,
    last_frame: Vec<i16>,
    jitter: VecDeque<(u64, u32)>,
    gaps: VecDeque<(u64, u32)>,
    received: VecDeque<u64>,
    late: u32,
}

pub fn spawn(socket: Arc<UdpSocket>, shared: Arc<Shared>, producer: rtrb::Producer<i16>, cfg: NetCfg) {
    std::thread::Builder::new()
        .name("lanvoice-net".into())
        .spawn(move || run(socket, shared, producer, cfg))
        .expect("Netz-Thread");
}

fn run(socket: Arc<UdpSocket>, shared: Arc<Shared>, mut producer: rtrb::Producer<i16>, cfg: NetCfg) {
    let my_id: u32 = {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        (t ^ (std::process::id() as u64).rotate_left(17)) as u32 ^ (t >> 32) as u32
    };
    let capacity = producer.buffer().capacity();
    let mut buf = [0u8; 4096];
    let mut rx = Rx {
        last_seq: None,
        last_arrival_us: 0,
        last_ns: shared.frame_samples.load(Relaxed) as usize,
        last_frame: Vec::new(),
        jitter: VecDeque::new(),
        gaps: VecDeque::new(),
        received: VecDeque::new(),
        late: 0,
    };
    let mut last_hello: u64 = 0;
    let mut last_stats: u64 = 0;
    let mut last_underrun_ms: u64 = shared.now_ms();
    let mut last_shrink_ms: u64 = 0;
    let mut prev_underruns = shared.underruns.load(Relaxed);

    if let Some(p) = cfg.peer {
        shared.set_peer(SocketAddr::V4(SocketAddrV4::new(p, cfg.port)));
    }

    loop {
        match socket.recv_from(&mut buf) {
            Ok((n, src)) => handle(&buf[..n], src, &socket, &shared, &mut producer, capacity, &mut rx, my_id),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::ConnectionReset) => {}
            Err(e) => {
                if let Ok(mut ne) = shared.net_error.lock() {
                    *ne = Some(format!("Netzfehler: {e}"));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }

        let now = shared.now_ms();
        let interval = if shared.connected() { 5000 } else { 1000 };
        if now.saturating_sub(last_hello) >= interval {
            last_hello = now;
            send_hello(&socket, &shared, &cfg, my_id);
        }
        if now.saturating_sub(last_stats) >= 500 {
            last_stats = now;
            stats_and_control(&shared, &mut rx, now, &mut prev_underruns, &mut last_underrun_ms, &mut last_shrink_ms);
        }
    }
}

fn send_hello(socket: &UdpSocket, shared: &Shared, cfg: &NetCfg, my_id: u32) {
    let mut pkt = vec![T_HELLO, VERSION];
    pkt.extend_from_slice(&my_id.to_le_bytes());
    let name = cfg.name.as_bytes();
    pkt.extend_from_slice(&name[..name.len().min(32)]);
    let _ = socket.send_to(&pkt, SocketAddrV4::new(Ipv4Addr::BROADCAST, cfg.port));
    if let Some(p) = cfg.peer {
        let _ = socket.send_to(&pkt, SocketAddrV4::new(p, cfg.port));
    }
    if let Some(a) = shared.peer_addr() {
        let _ = socket.send_to(&pkt, a);
    }
}

#[allow(clippy::too_many_arguments)]
fn handle(
    pkt: &[u8],
    src: SocketAddr,
    socket: &UdpSocket,
    shared: &Shared,
    producer: &mut rtrb::Producer<i16>,
    capacity: usize,
    rx: &mut Rx,
    my_id: u32,
) {
    if pkt.is_empty() || !matches!(src, SocketAddr::V4(_)) {
        return;
    }
    match pkt[0] {
        T_HELLO => {
            if pkt.len() < 6 || pkt[1] != VERSION {
                return;
            }
            let id = u32::from_le_bytes([pkt[2], pkt[3], pkt[4], pkt[5]]);
            if id == my_id {
                return;
            }
            let name = String::from_utf8_lossy(&pkt[6..]).trim().to_string();
            match shared.peer_addr() {
                None => {
                    shared.set_peer(src);
                    if let Ok(mut n) = shared.peer_name.lock() {
                        *n = name;
                    }
                }
                Some(p) if p.ip() == src.ip() => {
                    if let Ok(mut n) = shared.peer_name.lock() {
                        if n.is_empty() {
                            *n = name;
                        }
                    }
                }
                Some(_) => {
                    if let Ok(mut s) = shared.second_peer.lock() {
                        *s = Some(format!("{name} ({})", src.ip()));
                    }
                }
            }
        }
        T_AUDIO => {
            if pkt.len() < HEADER {
                return;
            }
            match shared.peer_addr() {
                None => shared.set_peer(src),
                Some(p) if p.ip() != src.ip() => return,
                Some(p) if p.port() != src.port() => shared.set_peer(src),
                _ => {}
            }
            let muted = pkt[1] & 1 == 1;
            let seq = u16::from_le_bytes([pkt[2], pkt[3]]);
            let ts = u32::from_le_bytes([pkt[4], pkt[5], pkt[6], pkt[7]]);
            let now_ms = shared.now_ms();
            let now_us = shared.now_us();
            shared.peer_muted.store(muted, Relaxed);
            shared.last_rx_ms.store(now_ms, Relaxed);

            if seq % 200 == 0 {
                let mut pong = [T_PONG, 0, 0, 0, 0, 0, 0, 0];
                pong[4..8].copy_from_slice(&ts.to_le_bytes());
                let _ = socket.send_to(&pong, src);
            }

            let ns = if muted { rx.last_ns } else { (pkt.len() - HEADER) / 2 };
            if ns == 0 {
                return;
            }
            if !muted {
                rx.last_ns = ns;
            }

            // Jitter: Abweichung des Ankunftsabstands vom Sollabstand.
            if let Some(ls) = rx.last_seq {
                let d = seq.wrapping_sub(ls) as i16;
                if d > 0 {
                    let expected = d as u64 * ns as u64 * 1_000_000 / RATE as u64;
                    let actual = now_us.wrapping_sub(rx.last_arrival_us) as u64;
                    let dev = actual.abs_diff(expected).min(1_000_000) as u32;
                    rx.jitter.push_back((now_ms, dev));
                }
            }
            rx.last_arrival_us = now_us;

            // Reihenfolge, Lücken, Nachzügler.
            if let Some(ls) = rx.last_seq {
                let d = seq.wrapping_sub(ls) as i16;
                if d <= 0 {
                    rx.late += 1;
                    return;
                }
                if d > 1 {
                    let gap = (d - 1) as u32;
                    rx.gaps.push_back((now_ms, gap));
                    if gap <= MAX_GAP_FILL {
                        for i in 0..gap {
                            if i == 0 && !rx.last_frame.is_empty() {
                                for &s in &rx.last_frame {
                                    let _ = producer.push(s / 2);
                                }
                            } else {
                                for _ in 0..ns {
                                    let _ = producer.push(0);
                                }
                            }
                        }
                    }
                }
            }
            rx.last_seq = Some(seq);
            rx.received.push_back(now_ms);

            let target = shared.target_frames.load(Relaxed) as usize;
            let fill = capacity - producer.slots();
            if fill + ns > (target + 2) * ns {
                shared.dropped.fetch_add(1, Relaxed);
                return;
            }
            rx.last_frame.clear();
            if muted {
                for _ in 0..ns {
                    let _ = producer.push(0);
                }
            } else {
                for c in pkt[HEADER..].chunks_exact(2) {
                    let s = i16::from_le_bytes([c[0], c[1]]);
                    rx.last_frame.push(s);
                    let _ = producer.push(s);
                }
            }
            let fill = capacity - producer.slots();
            if shared.priming.load(Relaxed) && fill >= target * ns {
                shared.priming.store(false, Relaxed);
            }
        }
        T_PONG => {
            if pkt.len() < 8 {
                return;
            }
            let ts = u32::from_le_bytes([pkt[4], pkt[5], pkt[6], pkt[7]]);
            let rtt = shared.now_us().wrapping_sub(ts);
            shared.rtt_us.store(rtt.min(5_000_000), Relaxed);
        }
        _ => {}
    }
}

fn stats_and_control(
    shared: &Shared,
    rx: &mut Rx,
    now: u64,
    prev_underruns: &mut u32,
    last_underrun_ms: &mut u64,
    last_shrink_ms: &mut u64,
) {
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
    shared.jitter_us.store(p95, Relaxed);

    let lost: u64 = rx.gaps.iter().map(|(_, g)| *g as u64).sum();
    let got = rx.received.len() as u64;
    let permille = if lost + got == 0 { 0 } else { (lost * 1000 / (lost + got)) as u32 };
    shared.loss_permille.store(permille, Relaxed);

    let auto = shared.jitter_auto.load(Relaxed);
    let mut target = shared.target_frames.load(Relaxed);
    let und = shared.underruns.load(Relaxed);
    if und != *prev_underruns {
        *prev_underruns = und;
        *last_underrun_ms = now;
        if auto && shared.connected() {
            target = (target + 1).min(MAX_TARGET);
        }
    } else if auto {
        let frame_us = rx.last_ns as u64 * 1_000_000 / RATE as u64;
        let desired = ((p95 as u64 + frame_us - 1) / frame_us.max(1)) as u32 + 1;
        let desired = desired.clamp(2, MAX_TARGET);
        if desired > target {
            target = desired;
        } else if desired < target
            && now.saturating_sub(*last_underrun_ms) > 30_000
            && now.saturating_sub(*last_shrink_ms) > 30_000
            && (shared.peer_muted.load(Relaxed) || shared.spk_level_f() < 0.01)
        {
            target -= 1;
            *last_shrink_ms = now;
            shared.skip_samples.fetch_add(rx.last_ns as u32, Relaxed);
        }
    }
    shared.target_frames.store(target.clamp(MIN_TARGET, MAX_TARGET), Relaxed);
}
