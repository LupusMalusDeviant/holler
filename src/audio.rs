//! Aufnahme und Wiedergabe über cpal (WASAPI Shared Mode).
//! Beide Seiten arbeiten intern mit 48 kHz mono; auf das Geräteformat wird
//! linear umgerechnet, damit auch 16-kHz-Funkmikros und 44,1-kHz-Ausgaben gehen.

use crate::net::Wire;
use crate::state::{Shared, MAX_PEERS, RATE};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SizedSample};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};

pub struct Devices {
    pub inputs: Vec<(String, cpal::Device)>,
    pub outputs: Vec<(String, cpal::Device)>,
    pub default_input: Option<String>,
    pub default_output: Option<String>,
}

pub fn enumerate(host: &cpal::Host) -> Devices {
    fn collect<I: Iterator<Item = cpal::Device>>(it: Option<I>) -> Vec<(String, cpal::Device)> {
        it.map(|d| d.filter_map(|d| d.name().ok().map(|n| (n, d))).collect())
            .unwrap_or_default()
    }
    Devices {
        inputs: collect(host.input_devices().ok()),
        outputs: collect(host.output_devices().ok()),
        default_input: host.default_input_device().and_then(|d| d.name().ok()),
        default_output: host.default_output_device().and_then(|d| d.name().ok()),
    }
}

/// Auswahl per Index, Teilstring (ohne Gross/Klein) oder Standardgerät.
pub fn find(list: &[(String, cpal::Device)], wanted: &Option<String>, default_name: &Option<String>) -> Option<usize> {
    if let Some(w) = wanted {
        let w = w.trim();
        if let Ok(i) = w.parse::<usize>() {
            return if i < list.len() { Some(i) } else { None };
        }
        let lw = w.to_lowercase();
        return list.iter().position(|(n, _)| n.to_lowercase().contains(&lw));
    }
    if let Some(d) = default_name {
        if let Some(i) = list.iter().position(|(n, _)| n == d) {
            return Some(i);
        }
    }
    if list.is_empty() { None } else { Some(0) }
}

/// Umrechnung beim Aufnehmen: Geräterate → 48 kHz, chunkweise.
struct PushResampler {
    ratio: f64,
    pos: f64,
    last: f32,
}

impl PushResampler {
    fn new(from: u32, to: u32) -> Self {
        PushResampler { ratio: from as f64 / to as f64, pos: 0.0, last: 0.0 }
    }

    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if input.is_empty() {
            return;
        }
        let len = input.len() as isize;
        let mut pos = self.pos;
        loop {
            let i = pos.floor() as isize;
            if i + 1 >= len {
                break;
            }
            let a = if i < 0 { self.last } else { input[i as usize] };
            let b = input[(i + 1) as usize];
            let frac = (pos - i as f64) as f32;
            out.push(a + (b - a) * frac);
            pos += self.ratio;
        }
        self.last = input[input.len() - 1];
        self.pos = pos - len as f64;
    }
}

/// Umrechnung beim Abspielen: 48 kHz → Geräterate, ziehend.
struct PullResampler {
    ratio: f64,
    pos: f64,
    prev: f32,
    cur: f32,
}

impl PullResampler {
    fn new(from: u32, to: u32) -> Self {
        PullResampler { ratio: from as f64 / to as f64, pos: 1.0, prev: 0.0, cur: 0.0 }
    }

    fn next(&mut self, mut pull: impl FnMut() -> Option<f32>) -> Option<f32> {
        self.pos += self.ratio;
        while self.pos >= 1.0 {
            let v = pull()?;
            self.prev = self.cur;
            self.cur = v;
            self.pos -= 1.0;
        }
        Some(self.prev + (self.cur - self.prev) * self.pos as f32)
    }
}

pub struct Capture {
    _stream: cpal::Stream,
    pub rate: u32,
    pub channels: u16,
}

pub fn open_capture(dev: &cpal::Device, shared: Arc<Shared>, wire: Wire) -> Result<Capture, String> {
    let sup = dev.default_input_config().map_err(|e| format!("Aufnahmeformat: {e}"))?;
    let rate = sup.sample_rate().0;
    let channels = sup.channels();
    let config: cpal::StreamConfig = sup.clone().into();
    let stream = match sup.sample_format() {
        cpal::SampleFormat::F32 => build_in::<f32>(dev, &config, rate, channels, shared, wire),
        cpal::SampleFormat::I16 => build_in::<i16>(dev, &config, rate, channels, shared, wire),
        cpal::SampleFormat::U16 => build_in::<u16>(dev, &config, rate, channels, shared, wire),
        cpal::SampleFormat::I32 => build_in::<i32>(dev, &config, rate, channels, shared, wire),
        other => return Err(format!("Aufnahmeformat {other:?} nicht unterstützt")),
    }?;
    stream.play().map_err(|e| format!("Aufnahme starten: {e}"))?;
    Ok(Capture { _stream: stream, rate, channels })
}

fn build_in<T>(
    dev: &cpal::Device,
    config: &cpal::StreamConfig,
    rate: u32,
    channels: u16,
    shared: Arc<Shared>,
    mut wire: Wire,
) -> Result<cpal::Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    use nnnoiseless::DenoiseState;
    const CHUNK: usize = DenoiseState::FRAME_SIZE; // 480 = 10 ms bei 48 kHz
    let ch = channels.max(1) as usize;
    let mut rs = PushResampler::new(rate, RATE);
    let mut mono: Vec<f32> = Vec::with_capacity(8192);
    let mut out: Vec<f32> = Vec::with_capacity(8192);
    let mut acc: Vec<f32> = Vec::with_capacity(16384);
    let mut ready: Vec<f32> = Vec::with_capacity(16384);
    let mut frame: Vec<i16> = Vec::with_capacity(1024);
    let mut denoiser = DenoiseState::new();
    let mut den_in = [0.0f32; CHUNK];
    let mut den_out = [0.0f32; CHUNK];
    // Sprechsperre: RNNoise liefert pro 10-ms-Block eine Sprechwahrscheinlichkeit.
    // Öffnet sofort, hält 400 ms, blendet über 50 ms aus.
    let mut gate_env = 1.0f32;
    let mut hold_left: u32 = 0;
    const HOLD_CHUNKS: u32 = 40;
    dev.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            mono.clear();
            for fr in data.chunks(ch) {
                let mut s = 0.0f32;
                for &x in fr {
                    s += f32::from_sample(x);
                }
                mono.push(s / ch as f32);
            }
            out.clear();
            rs.process(&mono, &mut out);
            acc.extend_from_slice(&out);

            let muted = shared.muted.load(Relaxed);
            let gain = shared.mic_gain_f();
            let denoise = shared.denoise.load(Relaxed);
            let threshold = shared.gate_threshold_f();

            while acc.len() >= CHUNK {
                // Verstärkung und Übersteuerung
                let mut clip = false;
                for (i, v) in acc[..CHUNK].iter().enumerate() {
                    let g = v * gain;
                    if g >= 0.99 || g <= -0.99 {
                        clip = true;
                    }
                    den_in[i] = g.clamp(-1.0, 1.0) * 32767.0;
                }
                // RNNoise: entrauscht und schätzt, ob gesprochen wird
                let vad = if denoise || threshold > 0.0 {
                    denoiser.process_frame(&mut den_out, &den_in)
                } else {
                    1.0
                };
                let src: &[f32] = if denoise { &den_out } else { &den_in };
                let mut sum_sq = 0.0f32;
                for &v in src {
                    let f = v / 32767.0;
                    sum_sq += f * f;
                }
                let rms = (sum_sq / CHUNK as f32).sqrt();
                // Sperre
                let want_open = threshold <= 0.0 || (vad >= threshold && rms > 0.001);
                if want_open {
                    hold_left = HOLD_CHUNKS;
                } else {
                    hold_left = hold_left.saturating_sub(1);
                }
                let target_env = if want_open || hold_left > 0 { 1.0 } else { 0.0 };
                gate_env = if target_env > gate_env { 1.0 } else { (gate_env - 0.2).max(0.0) };
                shared.gate_open.store(gate_env > 0.0, Relaxed);
                for &v in src {
                    ready.push((v / 32767.0 * gate_env).clamp(-1.0, 1.0));
                }
                shared.mic_level.store(if muted { 0.0 } else { rms }.to_bits(), Relaxed);
                shared.mic_clip.store(clip && !muted, Relaxed);
                acc.drain(..CHUNK);
            }

            let n = shared.frame_samples.load(Relaxed) as usize;
            while ready.len() >= n {
                frame.clear();
                for &v in &ready[..n] {
                    frame.push((v * 32767.0) as i16);
                }
                wire.send_frame(&frame);
                ready.drain(..n);
            }
        },
        |e| eprintln!("Aufnahmefehler: {e}"),
        None,
    )
    .map_err(|e| format!("Aufnahme öffnen: {e}"))
}

pub struct Playback {
    _stream: cpal::Stream,
    pub rate: u32,
    pub channels: u16,
}

/// Ein Ring pro Teilnehmerplatz; der Mixer sperrt einmal pro Callback.
pub type Consumers = Arc<Mutex<Vec<rtrb::Consumer<i16>>>>;

pub fn open_playback(dev: &cpal::Device, shared: Arc<Shared>, consumers: Consumers) -> Result<Playback, String> {
    let sup = dev.default_output_config().map_err(|e| format!("Wiedergabeformat: {e}"))?;
    let rate = sup.sample_rate().0;
    let channels = sup.channels();
    let config: cpal::StreamConfig = sup.clone().into();
    let stream = match sup.sample_format() {
        cpal::SampleFormat::F32 => build_out::<f32>(dev, &config, rate, channels, shared, consumers),
        cpal::SampleFormat::I16 => build_out::<i16>(dev, &config, rate, channels, shared, consumers),
        cpal::SampleFormat::U16 => build_out::<u16>(dev, &config, rate, channels, shared, consumers),
        cpal::SampleFormat::I32 => build_out::<i32>(dev, &config, rate, channels, shared, consumers),
        other => return Err(format!("Wiedergabeformat {other:?} nicht unterstützt")),
    }?;
    stream.play().map_err(|e| format!("Wiedergabe starten: {e}"))?;
    Ok(Playback { _stream: stream, rate, channels })
}

/// Weicher Begrenzer: bis 0,8 linear, darüber gestaucht, nie über 1.
fn limit(x: f32) -> f32 {
    let a = x.abs();
    if a <= 0.8 {
        x
    } else {
        let y = 0.8 + (a - 0.8) / (1.0 + (a - 0.8) * 3.0);
        y.min(1.0) * x.signum()
    }
}

/// Mixer: summiert alle aktiven Teilnehmer bei 48 kHz, dann Umrechnung auf die Geräterate.
fn build_out<T>(
    dev: &cpal::Device,
    config: &cpal::StreamConfig,
    rate: u32,
    channels: u16,
    shared: Arc<Shared>,
    consumers: Consumers,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let ch = channels.max(1) as usize;
    let mut rs = PullResampler::new(RATE, rate);
    dev.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let mut cons = match consumers.lock() {
                Ok(c) => c,
                Err(p) => p.into_inner(),
            };
            for (i, p) in shared.peers.iter().enumerate() {
                let skip = p.skip_samples.swap(0, Relaxed);
                for _ in 0..skip {
                    if cons[i].pop().is_err() {
                        break;
                    }
                }
            }
            let mut sums = [0.0f32; MAX_PEERS];
            let mut pulled: u32 = 0;
            let mut mix_sq = 0.0f32;
            let frames = (data.len() / ch).max(1);
            for fr in data.chunks_mut(ch) {
                let s = rs
                    .next(|| {
                        pulled += 1;
                        let mut sum = 0.0f32;
                        for (i, p) in shared.peers.iter().enumerate() {
                            if !p.active.load(Relaxed) || p.priming.load(Relaxed) {
                                continue;
                            }
                            match cons[i].pop() {
                                Ok(v) => {
                                    let f = v as f32 / 32768.0;
                                    sums[i] += f * f;
                                    if !p.local_mute.load(Relaxed) {
                                        sum += f * p.volume_f();
                                    }
                                }
                                Err(_) => {
                                    p.priming.store(true, Relaxed);
                                    p.underruns.fetch_add(1, Relaxed);
                                }
                            }
                        }
                        Some(limit(sum))
                    })
                    .unwrap_or(0.0);
                mix_sq += s * s;
                for x in fr.iter_mut() {
                    *x = T::from_sample(s);
                }
            }
            let n48 = pulled.max(1) as f32;
            for (i, p) in shared.peers.iter().enumerate() {
                if p.active.load(Relaxed) {
                    p.level.store((sums[i] / n48).sqrt().to_bits(), Relaxed);
                    p.buffered_samples.store(cons[i].slots() as u32, Relaxed);
                }
            }
            shared.spk_level.store((mix_sq / frames as f32).sqrt().to_bits(), Relaxed);
        },
        |e| eprintln!("Wiedergabefehler: {e}"),
        None,
    )
    .map_err(|e| format!("Wiedergabe öffnen: {e}"))
}

