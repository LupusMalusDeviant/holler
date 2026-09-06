//! Raum-ID und Raumschlüssel, Ver- und Entschlüsselung der Nutzlast.
//!
//! Raum-ID = BLAKE3-Ableitung aus dem Raumnamen (darf jeder sehen).
//! Schlüssel = Argon2id(Passwort, Salz = Raum-ID), absichtlich langsam.
//! Nutzlast: ChaCha20-Poly1305, Nonce = Absender-Kennung + Sequenz, der
//! Klartext-Kopf ist als Zusatzdaten mit authentifiziert.

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::AeadInPlace;
use chacha20poly1305::{ChaCha20Poly1305, Key, KeyInit, Nonce};

pub const ROOM_ID_LEN: usize = 32;
#[allow(dead_code)]
pub const TAG_LEN: usize = 16;

pub struct Room {
    pub name: String,
    pub id: [u8; ROOM_ID_LEN],
    cipher: ChaCha20Poly1305,
}

pub fn room_id(name: &str) -> [u8; ROOM_ID_LEN] {
    blake3::derive_key("holler room id v2", name.trim().to_lowercase().as_bytes())
}

/// Dauert absichtlich einige hundert Millisekunden. Nicht im UI-Thread aufrufen.
pub fn derive(name: &str, password: &str) -> Room {
    let id = room_id(name);
    let params = Params::new(32 * 1024, 3, 1, Some(32)).expect("Argon2-Parameter");
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    argon.hash_password_into(password.as_bytes(), &id, &mut key).expect("Argon2");
    Room { name: name.trim().to_string(), id, cipher: ChaCha20Poly1305::new(Key::from_slice(&key)) }
}

impl Room {
    /// Verschlüsselt `buf` an Ort und Stelle und hängt das 16-Byte-Tag an.
    pub fn seal(&self, header: &[u8], nonce: &[u8; 12], buf: &mut Vec<u8>) -> bool {
        self.cipher.encrypt_in_place(Nonce::from_slice(nonce), header, buf).is_ok()
    }

    /// Prüft und entschlüsselt `buf` an Ort und Stelle. `false` = falscher Schlüssel oder manipuliert.
    pub fn open(&self, header: &[u8], nonce: &[u8; 12], buf: &mut Vec<u8>) -> bool {
        self.cipher.decrypt_in_place(Nonce::from_slice(nonce), header, buf).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rundlauf_und_falsches_passwort() {
        let a = derive("Hunt-Abend", "geheim");
        let b = derive("hunt-abend ", "geheim");
        let c = derive("hunt-abend", "anders");
        assert_eq!(a.id, b.id);
        let header = [1u8, 2, 3];
        let nonce = [7u8; 12];
        let mut buf = b"hallo".to_vec();
        assert!(a.seal(&header, &nonce, &mut buf));
        assert_eq!(buf.len(), 5 + TAG_LEN);
        let mut wrong = buf.clone();
        assert!(!c.open(&header, &nonce, &mut wrong));
        assert!(b.open(&header, &nonce, &mut buf));
        assert_eq!(buf, b"hallo");
    }
}
