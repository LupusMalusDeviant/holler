//! Desktop-Audio: was auf dem eigenen Rechner läuft, als zweiter Kanal senden.
//!
//! Windows: Prozess-Loopback über WASAPI, „alles ausser Holler“ (eigener
//! Prozessbaum ausgeschlossen) oder „nur Programm X“. Hollers eigene Ausgabe ist
//! damit nie enthalten, kein Echo. Andere Plattformen: ein Aufnahmegerät aus der
//! Liste (Monitor unter PipeWire/Pulse, BlackHole unter macOS).
//! Liefert 48 kHz stereo in 10-ms-Rahmen an den Sendepfad.

use crate::audio::{Devices, PushResampler};
use crate::net::{Sockets, Wire};
use crate::state::{Shared, RATE};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Arc;

/// Samples je Kanal und Rahmen (10 ms).
pub const FRAME: usize = 480;

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// Alles ausser Holler (nur Windows).
    All,
    /// Nur ein Programm samt Kindprozessen (nur Windows).
    Process { pid: u32, name: String },
    /// Ein Aufnahmegerät (alle Plattformen).
    Device(String),
}

impl Source {
    #[allow(dead_code)]
    pub fn to_key(&self) -> String {
        match self {
            Source::All => "all".into(),
            Source::Process { pid, name } => format!("pid:{pid}:{name}"),
            Source::Device(d) => format!("dev:{d}"),
        }
    }

    pub fn parse(s: &str) -> Source {
        let s = s.trim();
        if let Some(rest) = s.strip_prefix("dev:") {
            return Source::Device(rest.to_string());
        }
        if let Some(rest) = s.strip_prefix("pid:") {
            let (pid, name) = rest.split_once(':').unwrap_or((rest, ""));
            if let Ok(pid) = pid.parse::<u32>() {
                return Source::Process { pid, name: name.to_string() };
            }
        }
        Source::All
    }

    pub fn label(&self) -> String {
        match self {
            Source::All => "Alles ausser Holler".into(),
            Source::Process { name, pid } => format!("Nur {name} ({pid})"),
            Source::Device(d) => format!("Gerät: {d}"),
        }
    }
}

pub struct Capture {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    _stream: Option<cpal::Stream>,
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Stereo-Rahmen verstärken, Pegel messen, senden.
struct Sink {
    shared: Arc<Shared>,
    wire: Wire,
    frame: Vec<i16>,
}

impl Sink {
    fn new(shared: Arc<Shared>, sockets: Arc<Sockets>) -> Self {
        Sink { wire: Wire::new_desktop(sockets, shared.clone()), shared, frame: Vec::with_capacity(FRAME * 2) }
    }

    /// `stereo` sind FRAME*2 interleaved Samples in [-1, 1].
    fn push(&mut self, stereo: &[f32]) {
        if !self.shared.desktop_on.load(Relaxed) {
            return;
        }
        let gain = self.shared.desktop_gain_f();
        let mut sum_sq = 0.0f32;
        self.frame.clear();
        for &v in stereo {
            let g = (v * gain).clamp(-1.0, 1.0);
            sum_sq += g * g;
            self.frame.push((g * 32767.0) as i16);
        }
        self.shared.desktop_level.store((sum_sq / stereo.len().max(1) as f32).sqrt().to_bits(), Relaxed);
        self.wire.send_desktop_frame(&self.frame);
    }
}

pub fn start(source: &Source, shared: Arc<Shared>, sockets: Arc<Sockets>, devices: &Devices) -> Result<Capture, String> {
    match source {
        Source::Device(name) => start_device(name, shared, sockets, devices),
        #[cfg(windows)]
        Source::All => start_process_loopback(std::process::id(), false, shared, sockets),
        #[cfg(windows)]
        Source::Process { pid, .. } => start_process_loopback(*pid, true, shared, sockets),
        #[cfg(not(windows))]
        _ => Err("Auf diesem System nur über ein Aufnahmegerät (Monitor oder virtuelles Gerät)".into()),
    }
}

/// Beliebiges Aufnahmegerät → 48 kHz stereo.
fn start_device(name: &str, shared: Arc<Shared>, sockets: Arc<Sockets>, devices: &Devices) -> Result<Capture, String> {
    use cpal::traits::{DeviceTrait, StreamTrait};
    let lname = name.to_lowercase();
    let (_, dev) = devices
        .inputs
        .iter()
        .find(|(n, _)| n.to_lowercase() == lname)
        .or_else(|| devices.inputs.iter().find(|(n, _)| n.to_lowercase().contains(&lname)))
        .ok_or_else(|| format!("Desktop-Quelle nicht gefunden: {name}"))?;
    let sup = dev.default_input_config().map_err(|e| format!("Desktop-Quelle: {e}"))?;
    let rate = sup.sample_rate().0;
    let ch = sup.channels().max(1) as usize;
    let config: cpal::StreamConfig = sup.clone().into();
    let mut sink = Sink::new(shared, sockets);
    let mut rs_l = PushResampler::new(rate, RATE);
    let mut rs_r = PushResampler::new(rate, RATE);
    let mut l: Vec<f32> = Vec::with_capacity(8192);
    let mut r: Vec<f32> = Vec::with_capacity(8192);
    let mut ol: Vec<f32> = Vec::with_capacity(8192);
    let mut or: Vec<f32> = Vec::with_capacity(8192);
    let mut acc: Vec<f32> = Vec::with_capacity(16384);
    let build = |dev: &cpal::Device, config: &cpal::StreamConfig| -> Result<cpal::Stream, String> {
        macro_rules! stream {
            ($t:ty) => {
                dev.build_input_stream(
                    config,
                    move |data: &[$t], _: &cpal::InputCallbackInfo| {
                        use cpal::Sample;
                        l.clear();
                        r.clear();
                        for fr in data.chunks(ch) {
                            let a = f32::from_sample(fr[0]);
                            let b = if ch >= 2 { f32::from_sample(fr[1]) } else { a };
                            l.push(a);
                            r.push(b);
                        }
                        ol.clear();
                        or.clear();
                        rs_l.process(&l, &mut ol);
                        rs_r.process(&r, &mut or);
                        for i in 0..ol.len().min(or.len()) {
                            acc.push(ol[i]);
                            acc.push(or[i]);
                        }
                        while acc.len() >= FRAME * 2 {
                            sink.push(&acc[..FRAME * 2]);
                            acc.drain(..FRAME * 2);
                        }
                    },
                    |e| eprintln!("Desktop-Aufnahmefehler: {e}"),
                    None,
                )
                .map_err(|e| format!("Desktop-Quelle öffnen: {e}"))
            };
        }
        match sup.sample_format() {
            cpal::SampleFormat::F32 => stream!(f32),
            cpal::SampleFormat::I16 => stream!(i16),
            cpal::SampleFormat::U16 => stream!(u16),
            cpal::SampleFormat::I32 => stream!(i32),
            other => Err(format!("Desktop-Quelle: Format {other:?} nicht unterstützt")),
        }
    };
    let stream = build(dev, &config)?;
    stream.play().map_err(|e| format!("Desktop-Quelle starten: {e}"))?;
    eprintln!("Desktop-Audio von Gerät „{name}“ ({rate} Hz, {ch} Kanäle)");
    Ok(Capture { stop: Arc::new(AtomicBool::new(false)), thread: None, _stream: Some(stream) })
}

/// Windows: Prozess-Loopback. `include` = nur dieser Prozessbaum, sonst alles ausser ihm.
#[cfg(windows)]
fn start_process_loopback(pid: u32, include: bool, shared: Arc<Shared>, sockets: Arc<Sockets>) -> Result<Capture, String> {
    use std::collections::VecDeque;
    use wasapi::{AudioClient, Direction, SampleType, StreamMode, WaveFormat};
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let thread = std::thread::Builder::new()
        .name("holler-desktop".into())
        .spawn(move || {
            let _ = wasapi::initialize_mta();
            let setup = (|| -> Result<(AudioClient, wasapi::Handle, wasapi::AudioCaptureClient), String> {
                let mut client = AudioClient::new_application_loopback_client(pid, include).map_err(|e| format!("Prozess-Loopback: {e}"))?;
                let fmt = WaveFormat::new(32, 32, &SampleType::Float, RATE as usize, 2, None);
                client
                    .initialize_client(&fmt, &Direction::Capture, &StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: 0 })
                    .map_err(|e| format!("Prozess-Loopback einrichten: {e}"))?;
                let h = client.set_get_eventhandle().map_err(|e| format!("Loopback-Ereignis: {e}"))?;
                let cap = client.get_audiocaptureclient().map_err(|e| format!("Loopback-Client: {e}"))?;
                client.start_stream().map_err(|e| format!("Loopback starten: {e}"))?;
                Ok((client, h, cap))
            })();
            let (client, h, cap) = match setup {
                Ok(v) => v,
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            };
            let _ = tx.send(Ok(()));
            let mut sink = Sink::new(shared, sockets);
            let mut q: VecDeque<u8> = VecDeque::with_capacity(65536);
            let mut frame = vec![0.0f32; FRAME * 2];
            let bytes_per_frame = FRAME * 2 * 4;
            while !stop2.load(Relaxed) {
                if h.wait_for_event(500).is_err() {
                    continue;
                }
                let n = cap.get_next_packet_size().ok().flatten().unwrap_or(0);
                if n > 0 {
                    if cap.read_from_device_to_deque(&mut q).is_err() {
                        break;
                    }
                }
                while q.len() >= bytes_per_frame {
                    for v in frame.iter_mut() {
                        let b = [q.pop_front().unwrap(), q.pop_front().unwrap(), q.pop_front().unwrap(), q.pop_front().unwrap()];
                        *v = f32::from_le_bytes(b);
                    }
                    sink.push(&frame);
                }
            }
            let _ = client.stop_stream();
        })
        .map_err(|e| format!("Desktop-Thread: {e}"))?;
    match rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(Ok(())) => {
            eprintln!("Desktop-Audio: Prozess-Loopback ({})", if include { format!("nur PID {pid}") } else { "alles ausser Holler".into() });
            Ok(Capture { stop, thread: Some(thread), _stream: None })
        }
        Ok(Err(e)) => {
            stop.store(true, Relaxed);
            let _ = thread.join();
            Err(e)
        }
        Err(_) => {
            stop.store(true, Relaxed);
            Err("Desktop-Audio: keine Antwort vom Audiosystem".into())
        }
    }
}

/// Programme mit aktiver Tonausgabe (Windows), ohne Holler selbst.
#[cfg(windows)]
pub fn list_processes() -> Vec<(u32, String)> {
    use wasapi::{DeviceEnumerator, Direction, SessionState};
    let _ = wasapi::initialize_mta();
    let mut out: Vec<(u32, String)> = Vec::new();
    let me = std::process::id();
    let Ok(en) = DeviceEnumerator::new() else { return out };
    let Ok(coll) = en.get_device_collection(&Direction::Render) else { return out };
    let n = coll.get_nbr_devices().unwrap_or(0);
    for i in 0..n {
        let Ok(dev) = coll.get_device_at_index(i) else { continue };
        let Ok(mgr) = dev.get_iaudiosessionmanager() else { continue };
        let Ok(se) = mgr.get_audiosessionenumerator() else { continue };
        let cnt = se.get_count().unwrap_or(0);
        for j in 0..cnt {
            let Ok(c) = se.get_session(j) else { continue };
            if !matches!(c.get_state(), Ok(SessionState::Active)) {
                continue;
            }
            let Ok(pid) = c.get_process_id() else { continue };
            if pid == 0 || pid == me || out.iter().any(|(p, _)| *p == pid) {
                continue;
            }
            out.push((pid, process_name(pid).unwrap_or_else(|| format!("PID {pid}"))));
        }
    }
    out.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    out
}

#[cfg(not(windows))]
pub fn list_processes() -> Vec<(u32, String)> {
    Vec::new()
}

#[cfg(windows)]
fn process_name(pid: u32) -> Option<String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        if ok == 0 {
            return None;
        }
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        let name = full.rsplit(['\\', '/']).next().unwrap_or(&full).to_string();
        Some(name.trim_end_matches(".exe").trim_end_matches(".EXE").to_string())
    }
}
