//! Programmicon, prozedural gezeichnet: Sprechblase mit drei Pegelbalken auf
//! einem abgerundeten Quadrat. Eine Quelle für Exe-Icon (build.rs), Fenster
//! und Tray; das Tray färbt den Hintergrund nach Zustand.
#![allow(dead_code)]

pub const GREEN: [u8; 3] = [0x2e, 0xc2, 0x8a];
pub const RED: [u8; 3] = [0xe8, 0x5d, 0x5d];
pub const AMBER: [u8; 3] = [0xf0, 0xb4, 0x3a];
pub const GREY: [u8; 3] = [0x7c, 0x83, 0x8f];

/// Vorzeichenbehafteter Abstand zu einem abgerundeten Rechteck (Mitte, Halbmasse, Radius).
fn sd_round_rect(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let dx = (px - cx).abs() - (hw - r);
    let dy = (py - cy).abs() - (hh - r);
    let ox = dx.max(0.0);
    let oy = dy.max(0.0);
    (ox * ox + oy * oy).sqrt() + dx.max(dy).min(0.0) - r
}

fn in_tri(px: f32, py: f32, a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> bool {
    let s = |p: (f32, f32), q: (f32, f32)| (px - q.0) * (p.1 - q.1) - (p.0 - q.0) * (py - q.1);
    let d1 = s(a, b);
    let d2 = s(b, c);
    let d3 = s(c, a);
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

/// Farbe an der Stelle (u, v) im Einheitsquadrat, RGBA 0..1.
fn shade(u: f32, v: f32, bg: [u8; 3]) -> [f32; 4] {
    if sd_round_rect(u, v, 0.5, 0.5, 0.5, 0.5, 0.22) > 0.0 {
        return [0.0, 0.0, 0.0, 0.0];
    }
    let b = [bg[0] as f32 / 255.0, bg[1] as f32 / 255.0, bg[2] as f32 / 255.0];
    // Pegelbalken in der Blase, dunkler Hintergrundton
    for (cx, h) in [(0.37, 0.14), (0.50, 0.30), (0.63, 0.22)] {
        if sd_round_rect(u, v, cx, 0.44, 0.045, h / 2.0, 0.045) <= 0.0 {
            return [b[0] * 0.45, b[1] * 0.45, b[2] * 0.45, 1.0];
        }
    }
    // Sprechblase mit Zipfel
    if sd_round_rect(u, v, 0.5, 0.44, 0.33, 0.24, 0.13) <= 0.0 || in_tri(u, v, (0.29, 0.62), (0.47, 0.62), (0.29, 0.81)) {
        return [0.98, 0.98, 0.98, 1.0];
    }
    // Hintergrund mit leichtem Verlauf, oben heller
    let k = 1.14 - 0.30 * v;
    [(b[0] * k).min(1.0), (b[1] * k).min(1.0), (b[2] * k).min(1.0), 1.0]
}

/// RGBA-Bild der Kantenlänge `size`, 4×4-fach überabgetastet.
pub fn render(size: u32, bg: [u8; 3]) -> Vec<u8> {
    const SS: u32 = 4;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let mut rgb = [0.0f32; 3];
            let mut a = 0.0f32;
            for sy in 0..SS {
                for sx in 0..SS {
                    let u = (x as f32 + (sx as f32 + 0.5) / SS as f32) / size as f32;
                    let v = (y as f32 + (sy as f32 + 0.5) / SS as f32) / size as f32;
                    let c = shade(u, v, bg);
                    rgb[0] += c[0] * c[3];
                    rgb[1] += c[1] * c[3];
                    rgb[2] += c[2] * c[3];
                    a += c[3];
                }
            }
            let n = (SS * SS) as f32;
            if a > 0.0 {
                out.push((rgb[0] / a * 255.0).round() as u8);
                out.push((rgb[1] / a * 255.0).round() as u8);
                out.push((rgb[2] / a * 255.0).round() as u8);
            } else {
                out.extend_from_slice(&[0, 0, 0]);
            }
            out.push((a / n * 255.0).round() as u8);
        }
    }
    out
}

/// Windows-ICO mit 32-bit-BGRA-Bitmaps in den angegebenen Grössen.
pub fn ico(sizes: &[u32], bg: [u8; 3]) -> Vec<u8> {
    let mut images: Vec<(u32, Vec<u8>)> = Vec::new();
    for &s in sizes {
        let rgba = render(s, bg);
        let mask_row = ((s + 31) / 32 * 4) as usize;
        let mut img = Vec::with_capacity(40 + (s * s * 4) as usize + mask_row * s as usize);
        // BITMAPINFOHEADER
        img.extend_from_slice(&40u32.to_le_bytes());
        img.extend_from_slice(&(s as i32).to_le_bytes());
        img.extend_from_slice(&((s * 2) as i32).to_le_bytes());
        img.extend_from_slice(&1u16.to_le_bytes());
        img.extend_from_slice(&32u16.to_le_bytes());
        img.extend_from_slice(&0u32.to_le_bytes());
        img.extend_from_slice(&((s * s * 4) as u32 + (mask_row * s as usize) as u32).to_le_bytes());
        img.extend_from_slice(&[0u8; 16]);
        // XOR-Bitmap, zeilenweise von unten, BGRA
        for y in (0..s).rev() {
            for x in 0..s {
                let i = ((y * s + x) * 4) as usize;
                img.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
            }
        }
        // AND-Maske, leer (Alpha regelt)
        img.extend(std::iter::repeat(0u8).take(mask_row * s as usize));
        images.push((s, img));
    }
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (s, img) in &images {
        let dim = if *s >= 256 { 0u8 } else { *s as u8 };
        out.push(dim);
        out.push(dim);
        out.push(0);
        out.push(0);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(img.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for (_, img) in images {
        out.extend_from_slice(&img);
    }
    out
}
