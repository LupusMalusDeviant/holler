//! holler-hub — Vermittler für Holler-Räume.
//!
//! Ein UDP-Port, IPv4 und IPv6. Kennt Räume und ihre Mitglieder, sagt jedem,
//! unter welcher öffentlichen Adresse die anderen zu erreichen sind, und
//! reicht verschlüsselte Pakete weiter, wenn eine Direktverbindung scheitert.
//! Sieht nie Klartext-Audio: die Nutzlast ist Ende-zu-Ende verschlüsselt.
//! Kein Zustand auf Platte, keine Konten, keine Abhängigkeiten.
//!
//! Paketkopf wie im Client (20 Byte): "H2", Typ, Flags, Absender u64, Seq u32, Zeit u32.
//!   JOIN     10  [version][raum-id 32][namelen][name][n][fam,ip16,port]*n    alle 5 s, auch Keepalive
//!   MEMBERS  11  [count][id u64][namelen][name][n][kind,fam,ip16,port]*n]*count  Antwort an alle im Raum
//!   RELAY    12  [ziel-id u64][inneres Paket]                                Hub leitet inneres Paket weiter
//!   LEAVE     6  leer
//!   PING     13  leer → PONG 3 mit Zeitstempel zurück (Laufzeit zum Hub)

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

const HEADER: usize = 20;
const MAGIC: [u8; 2] = *b"H2";
const T_PONG: u8 = 3;
const T_LEAVE: u8 = 6;
const T_JOIN: u8 = 10;
const T_MEMBERS: u8 = 11;
const T_RELAY: u8 = 12;
const T_PING: u8 = 13;
const VERSION: u8 = 2;
const ROOM_ID_LEN: usize = 32;
const MAX_MEMBERS: usize = 8;
const MEMBER_TIMEOUT: Duration = Duration::from_secs(15);
/// Relay-Budget je Mitglied: reicht für PCM (768 kbit/s) mit Reserve.
const RELAY_BYTES_PER_SEC: usize = 1_500_000;

struct Member {
    name: String,
    public_v4: Option<SocketAddr>,
    public_v6: Option<SocketAddr>,
    locals: Vec<SocketAddr>,
    last_seen: Instant,
    relay_window: Instant,
    relay_bytes: usize,
}

impl Member {
    /// Adresse, unter der das Mitglied zuletzt gesehen wurde, passend zur Familie des Absenders.
    fn reach(&self, prefer_v6: bool) -> Option<SocketAddr> {
        if prefer_v6 {
            self.public_v6.or(self.public_v4)
        } else {
            self.public_v4.or(self.public_v6)
        }
    }
}

struct Room {
    members: HashMap<u64, Member>,
}

/// Docker/Linux liefern IPv4-Absender als ::ffff:a.b.c.d; für die Anzeige zurückfalten.
fn unmap(a: SocketAddr) -> SocketAddr {
    match a {
        SocketAddr::V6(v6) => match v6.ip().to_ipv4_mapped() {
            Some(v4) => SocketAddr::new(IpAddr::V4(v4), v6.port()),
            None => a,
        },
        v4 => v4,
    }
}

fn write_header(buf: &mut Vec<u8>, typ: u8, sender: u64, ts: u32) {
    buf.extend_from_slice(&MAGIC);
    buf.push(typ);
    buf.push(0);
    buf.extend_from_slice(&sender.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&ts.to_le_bytes());
}

fn push_addr(buf: &mut Vec<u8>, kind: u8, a: SocketAddr) {
    let a = unmap(a);
    buf.push(kind);
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
    let fam = p[0];
    let mut oct = [0u8; 16];
    oct.copy_from_slice(&p[1..17]);
    let port = u16::from_le_bytes([p[17], p[18]]);
    let ip = match fam {
        4 => IpAddr::V4(Ipv6Addr::from(oct).to_ipv4_mapped().unwrap_or(Ipv4Addr::UNSPECIFIED)),
        _ => IpAddr::V6(Ipv6Addr::from(oct)),
    };
    Some((SocketAddr::new(ip, port), 19))
}

fn members_packet(room_id_hint: u8, room: &Room) -> Vec<u8> {
    let _ = room_id_hint;
    let mut buf = Vec::with_capacity(HEADER + 1 + room.members.len() * 96);
    write_header(&mut buf, T_MEMBERS, 0, 0);
    buf.push(room.members.len() as u8);
    for (id, m) in &room.members {
        buf.extend_from_slice(&id.to_le_bytes());
        let name = m.name.as_bytes();
        let n = name.len().min(32);
        buf.push(n as u8);
        buf.extend_from_slice(&name[..n]);
        let mut addrs: Vec<(u8, SocketAddr)> = Vec::new();
        if let Some(a) = m.public_v4 {
            addrs.push((1, a));
        }
        if let Some(a) = m.public_v6 {
            addrs.push((1, a));
        }
        for a in &m.locals {
            addrs.push((2, *a));
        }
        buf.push(addrs.len() as u8);
        for (kind, a) in addrs {
            push_addr(&mut buf, kind, a);
        }
    }
    buf
}

fn main() {
    let mut port: u16 = 4712;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--port" {
            port = args.next().and_then(|p| p.parse().ok()).unwrap_or(4712);
        }
    }
    // Linux: [::] ist dual-stack, IPv4-Absender erscheinen als ::ffff:a.b.c.d.
    let socket = UdpSocket::bind(("::", port)).unwrap_or_else(|e| {
        eprintln!("Port {port} nicht bindbar: {e}");
        std::process::exit(3);
    });
    socket.set_read_timeout(Some(Duration::from_millis(1000))).expect("Timeout");
    eprintln!("holler-hub lauscht auf UDP {port} (IPv4+IPv6)");

    let start = Instant::now();
    let mut rooms: HashMap<[u8; ROOM_ID_LEN], Room> = HashMap::new();
    let mut buf = [0u8; 4096];
    let mut last_sweep = Instant::now();
    let mut out = Vec::with_capacity(4096);

    loop {
        match socket.recv_from(&mut buf) {
            Ok((n, src)) => {
                let pkt = &buf[..n];
                if n < HEADER || pkt[0..2] != MAGIC {
                    continue;
                }
                let typ = pkt[2];
                let sender = u64::from_le_bytes(pkt[4..12].try_into().unwrap());
                let ts = u32::from_le_bytes(pkt[16..20].try_into().unwrap());
                let body = &pkt[HEADER..];
                let now = Instant::now();
                match typ {
                    T_PING => {
                        out.clear();
                        write_header(&mut out, T_PONG, 0, ts);
                        let _ = socket.send_to(&out, src);
                    }
                    T_JOIN => {
                        if body.len() < 1 + ROOM_ID_LEN + 1 || body[0] != VERSION || sender == 0 {
                            continue;
                        }
                        let mut room_id = [0u8; ROOM_ID_LEN];
                        room_id.copy_from_slice(&body[1..1 + ROOM_ID_LEN]);
                        let mut p = 1 + ROOM_ID_LEN;
                        let nl = body[p] as usize;
                        p += 1;
                        if body.len() < p + nl + 1 {
                            continue;
                        }
                        let name = String::from_utf8_lossy(&body[p..p + nl]).to_string();
                        p += nl;
                        let na = body[p] as usize;
                        p += 1;
                        let mut locals = Vec::new();
                        for _ in 0..na.min(8) {
                            let Some((a, used)) = read_addr(&body[p..]) else { break };
                            locals.push(a);
                            p += used;
                        }
                        let room = rooms.entry(room_id).or_insert_with(|| Room { members: HashMap::new() });
                        let is_new = !room.members.contains_key(&sender);
                        if is_new && room.members.len() >= MAX_MEMBERS {
                            continue;
                        }
                        let m = room.members.entry(sender).or_insert_with(|| Member {
                            name: String::new(),
                            public_v4: None,
                            public_v6: None,
                            locals: Vec::new(),
                            last_seen: now,
                            relay_window: now,
                            relay_bytes: 0,
                        });
                        let joined_name = name.clone();
                        m.name = name;
                        m.locals = locals;
                        m.last_seen = now;
                        match unmap(src).ip() {
                            IpAddr::V4(_) => m.public_v4 = Some(src),
                            IpAddr::V6(_) => m.public_v6 = Some(src),
                        }
                        if is_new {
                            eprintln!(
                                "[{:>7.1}s] Raum {:02x}{:02x}…: {} ({}) beigetreten, {} Mitglieder",
                                start.elapsed().as_secs_f32(),
                                room_id[0],
                                room_id[1],
                                joined_name,
                                unmap(src),
                                room.members.len()
                            );
                        }
                        // Mitgliederliste an alle im Raum, damit jeder den Neuen sieht.
                        let pkt = members_packet(room_id[0], room);
                        for m in room.members.values() {
                            for a in [m.public_v4, m.public_v6].into_iter().flatten() {
                                let _ = socket.send_to(&pkt, a);
                            }
                        }
                    }
                    T_RELAY => {
                        if body.len() < 8 + HEADER {
                            continue;
                        }
                        let dest = u64::from_le_bytes(body[0..8].try_into().unwrap());
                        let inner = &body[8..];
                        // Raum des Absenders finden (kein Raumfeld im Paket: der Absender ist eindeutig).
                        let Some(room) = rooms.values_mut().find(|r| r.members.contains_key(&sender)) else { continue };
                        let allowed = {
                            let Some(from) = room.members.get_mut(&sender) else { continue };
                            if now.duration_since(from.relay_window) >= Duration::from_secs(1) {
                                from.relay_window = now;
                                from.relay_bytes = 0;
                            }
                            from.relay_bytes += inner.len();
                            from.last_seen = now;
                            from.relay_bytes <= RELAY_BYTES_PER_SEC
                        };
                        if !allowed {
                            continue;
                        }
                        let prefer_v6 = matches!(unmap(src).ip(), IpAddr::V6(_));
                        if let Some(to) = room.members.get(&dest).and_then(|m| m.reach(prefer_v6)) {
                            let _ = socket.send_to(inner, to);
                        }
                    }
                    T_LEAVE => {
                        let mut empty = None;
                        for (rid, room) in rooms.iter_mut() {
                            if let Some(m) = room.members.remove(&sender) {
                                eprintln!("[{:>7.1}s] {} hat den Raum verlassen", start.elapsed().as_secs_f32(), m.name);
                                let pkt = members_packet(rid[0], room);
                                for m in room.members.values() {
                                    for a in [m.public_v4, m.public_v6].into_iter().flatten() {
                                        let _ = socket.send_to(&pkt, a);
                                    }
                                }
                                if room.members.is_empty() {
                                    empty = Some(*rid);
                                }
                                break;
                            }
                        }
                        if let Some(rid) = empty {
                            rooms.remove(&rid);
                        }
                    }
                    _ => {}
                }
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::ConnectionReset) => {}
            Err(e) => {
                eprintln!("Netzfehler: {e}");
                std::thread::sleep(Duration::from_millis(100));
            }
        }

        if last_sweep.elapsed() >= Duration::from_secs(2) {
            last_sweep = Instant::now();
            let now = Instant::now();
            let mut changed: Vec<[u8; ROOM_ID_LEN]> = Vec::new();
            for (rid, room) in rooms.iter_mut() {
                let before = room.members.len();
                room.members.retain(|_, m| now.duration_since(m.last_seen) < MEMBER_TIMEOUT);
                if room.members.len() != before {
                    changed.push(*rid);
                }
            }
            for rid in changed {
                if let Some(room) = rooms.get(&rid) {
                    if room.members.is_empty() {
                        rooms.remove(&rid);
                        continue;
                    }
                    let pkt = members_packet(rid[0], room);
                    for m in room.members.values() {
                        for a in [m.public_v4, m.public_v6].into_iter().flatten() {
                            let _ = socket.send_to(&pkt, a);
                        }
                    }
                }
            }
        }
    }
}
