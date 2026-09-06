//! Einladungslinks: `holler://join?room=<name>&pw=<passwort>&hub=<host:port>`.
//! Prozent-kodiert, damit Leerzeichen und Sonderzeichen überleben.

#[derive(Debug, Clone, PartialEq)]
pub struct Invite {
    pub room: String,
    pub password: String,
    pub hub: Option<String>,
}

fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = &s[i + 1..i + 3];
                match u8::from_str_radix(hex, 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

impl Invite {
    pub fn to_url(&self) -> String {
        let mut u = format!("holler://join?room={}&pw={}", encode(&self.room), encode(&self.password));
        if let Some(h) = &self.hub {
            if !h.trim().is_empty() {
                u.push_str(&format!("&hub={}", encode(h.trim())));
            }
        }
        u
    }

    pub fn parse(url: &str) -> Option<Invite> {
        let rest = url.trim().strip_prefix("holler://")?;
        let query = rest.split_once('?').map(|(_, q)| q).unwrap_or("");
        let mut room = None;
        let mut password = String::new();
        let mut hub = None;
        for part in query.split('&') {
            let (k, v) = part.split_once('=').unwrap_or((part, ""));
            match k {
                "room" => room = Some(decode(v)),
                "pw" | "password" => password = decode(v),
                "hub" => hub = Some(decode(v)).filter(|h| !h.is_empty()),
                _ => {}
            }
        }
        let room = room?.trim().to_string();
        if room.is_empty() {
            return None;
        }
        Some(Invite { room, password, hub })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rundlauf() {
        let i = Invite { room: "Hunt Abend".into(), password: "ge#heim&ß".into(), hub: Some("holler.app.lupusmalus.dev:4712".into()) };
        let u = i.to_url();
        assert!(u.starts_with("holler://join?room=Hunt%20Abend&pw=ge%23heim%26"));
        assert_eq!(Invite::parse(&u), Some(i));
        assert_eq!(Invite::parse("holler://join?room=x"), Some(Invite { room: "x".into(), password: String::new(), hub: None }));
        assert_eq!(Invite::parse("https://example"), None);
        assert_eq!(Invite::parse("holler://join?pw=a"), None);
    }
}
