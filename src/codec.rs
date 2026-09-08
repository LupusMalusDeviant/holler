//! Opus für die Internet-Strecke, reines Rust über `opus-pure`.
//! 48 kHz mono, 10-ms-Rahmen (480 Samples), VoIP-Profil. LAN-Peers bekommen
//! weiter rohes PCM; Opus ist nur für Wege, die das Haus verlassen.

use opus_pure::{Application, OpusDecoder, OpusEncoder};

/// Samples je Opus-Rahmen: 10 ms bei 48 kHz.
pub const FRAME: usize = 480;
pub const MAX_PACKET: usize = 1500;

/// Wählbare Qualitätsstufen für Ferne: 0 = rohes PCM.
pub const CHOICES: [(u32, &str, &str); 4] = [
    (0, "Rohes PCM", "Beste Qualität, kein Codec, 768 kbit/s je Stimme. Braucht sehr guten Upload; bei 4 Leuten empfängt jeder 2,3 Mbit/s."),
    (64, "Opus 64 kbit/s", "Von PCM nicht zu unterscheiden, rund 15 ms Verzögerung. Für alle mit normalem DSL oder Kabel."),
    (32, "Opus 32 kbit/s", "Sprachqualität wie Discord, schont schwachen Upload. Die Vorgabe."),
    (16, "Opus 16 kbit/s", "Telefonqualität. Für Mobilfunk oder Hotel-WLAN."),
];

pub fn choice_label(kbps: u32) -> &'static str {
    CHOICES.iter().find(|c| c.0 == kbps).map(|c| c.1).unwrap_or("Opus")
}

pub fn parse_choice(s: &str) -> u32 {
    match s.trim().to_ascii_lowercase().as_str() {
        "pcm" | "raw" | "0" => 0,
        "opus64" | "64" => 64,
        "opus16" | "16" => 16,
        _ => 32,
    }
}

pub fn choice_key(kbps: u32) -> &'static str {
    match kbps {
        0 => "pcm",
        64 => "opus64",
        16 => "opus16",
        _ => "opus32",
    }
}

pub struct Encoder {
    inner: OpusEncoder,
    kbps: u32,
    channels: usize,
}

impl Encoder {
    pub fn new(kbps: u32) -> Option<Self> {
        Self::new_ch(kbps, 1, false)
    }

    /// `music` = Audio-Profil (Musik/Spielsound) statt Sprachprofil.
    pub fn new_ch(kbps: u32, channels: usize, music: bool) -> Option<Self> {
        let app = if music { Application::Audio } else { Application::Voip };
        let mut inner = OpusEncoder::new(48_000, channels, app).ok()?;
        inner.bitrate_bps = (kbps.max(6) * 1000) as i32;
        inner.complexity = 6;
        Some(Encoder { inner, kbps, channels })
    }

    #[allow(dead_code)]
    pub fn kbps(&self) -> u32 {
        self.kbps
    }

    pub fn set_kbps(&mut self, kbps: u32) {
        if kbps != self.kbps {
            self.kbps = kbps;
            self.inner.bitrate_bps = (kbps.max(6) * 1000) as i32;
        }
    }

    /// Kodiert genau FRAME Rahmen (interleaved bei stereo) in `out` (wird passend gekürzt).
    pub fn encode(&mut self, pcm: &[i16], out: &mut Vec<u8>) -> bool {
        if pcm.len() != FRAME * self.channels {
            return false;
        }
        out.clear();
        out.resize(MAX_PACKET, 0);
        match self.inner.encode_s16(pcm, FRAME, out) {
            Ok(n) if n > 0 => {
                out.truncate(n);
                true
            }
            _ => {
                out.clear();
                false
            }
        }
    }
}

pub struct Decoder {
    inner: OpusDecoder,
    channels: usize,
}

impl Decoder {
    #[allow(dead_code)]
    pub fn new() -> Option<Self> {
        Self::new_ch(1)
    }

    pub fn new_ch(channels: usize) -> Option<Self> {
        Some(Decoder { inner: OpusDecoder::new(48_000, channels).ok()?, channels })
    }

    /// Dekodiert ein Paket nach `out` (FRAME Samples). Leeres Paket = Verlustverschleierung.
    pub fn decode(&mut self, packet: &[u8], out: &mut Vec<i16>) -> bool {
        out.clear();
        out.resize(FRAME * self.channels, 0);
        match self.inner.decode_s16(packet, FRAME, out) {
            Ok(n) if n > 0 => {
                out.truncate(n * self.channels);
                true
            }
            _ => {
                out.clear();
                false
            }
        }
    }

    pub fn conceal(&mut self, out: &mut Vec<i16>) -> bool {
        self.decode(&[], out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(v: &[i16]) -> f64 {
        (v.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>() / v.len().max(1) as f64).sqrt()
    }

    #[test]
    fn rundlauf_und_verschleierung() {
        let mut enc = Encoder::new(32).expect("Encoder");
        let mut dec = Decoder::new().expect("Decoder");
        let mut pkt = Vec::new();
        let mut out = Vec::new();
        let mut in_rms = 0.0;
        let mut out_rms = 0.0;
        for f in 0..20 {
            let frame: Vec<i16> = (0..FRAME)
                .map(|i| {
                    let t = (f * FRAME + i) as f64 / 48_000.0;
                    ((t * 440.0 * std::f64::consts::TAU).sin() * 8000.0) as i16
                })
                .collect();
            assert!(enc.encode(&frame, &mut pkt), "encode");
            assert!(pkt.len() < 200, "32 kbit/s ergibt kleine Pakete, war {}", pkt.len());
            assert!(dec.decode(&pkt, &mut out), "decode");
            assert_eq!(out.len(), FRAME);
            if f >= 10 {
                in_rms += rms(&frame);
                out_rms += rms(&out);
            }
        }
        let ratio = out_rms / in_rms;
        assert!((0.5..1.5).contains(&ratio), "Pegel nach Rundlauf unplausibel: {ratio}");
        assert!(dec.conceal(&mut out));
        assert_eq!(out.len(), FRAME);
        let mut e2 = Encoder::new(64).unwrap();
        e2.set_kbps(16);
        assert_eq!(e2.kbps(), 16);
        assert_eq!(parse_choice("Opus64"), 64);
        assert_eq!(parse_choice("pcm"), 0);
        assert_eq!(choice_key(parse_choice("unsinn")), "opus32");
    }

    #[test]
    fn stereo_rundlauf() {
        let mut enc = Encoder::new_ch(96, 2, true).expect("stereo");
        let mut dec = Decoder::new_ch(2).expect("stereo");
        let mut pkt = Vec::new();
        let mut out = Vec::new();
        for f in 0..20 {
            let frame: Vec<i16> = (0..FRAME * 2)
                .map(|i| {
                    let t = (f * FRAME + i / 2) as f64 / 48_000.0;
                    let hz = if i % 2 == 0 { 440.0 } else { 660.0 };
                    ((t * hz * std::f64::consts::TAU).sin() * 8000.0) as i16
                })
                .collect();
            assert!(enc.encode(&frame, &mut pkt));
            assert!(dec.decode(&pkt, &mut out));
            assert_eq!(out.len(), FRAME * 2);
        }
        let l = rms(&out.iter().step_by(2).copied().collect::<Vec<_>>());
        let r = rms(&out.iter().skip(1).step_by(2).copied().collect::<Vec<_>>());
        assert!(l > 2000.0 && r > 2000.0, "beide Kanäle müssen Pegel haben: {l} {r}");
    }
}
