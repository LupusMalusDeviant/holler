//! Verdrahtung: Sockets, Netz-Threads, Geräte, Aufnahme- und Wiedergabestrom.
//! Lebt auf dem Hauptthread; Streams werden bei Gerätewechsel neu geöffnet.

use crate::audio::{self, Capture, Consumers, Devices, Playback};
use crate::config::Config;
use crate::net::{self, NetCfg, Sockets, Wire};
use crate::desktop;
use crate::state::{Shared, SLOTS};
use std::sync::{Arc, Mutex};

pub struct Engine {
    pub devices: Devices,
    pub in_idx: Option<usize>,
    pub out_idx: Option<usize>,
    pub in_error: Option<String>,
    pub out_error: Option<String>,
    capture: Option<Capture>,
    playback: Option<Playback>,
    desktop: Option<desktop::Capture>,
    consumers: Consumers,
    pub sockets: Arc<Sockets>,
    shared: Arc<Shared>,
}

pub enum EngineError {
    Port(String),
    Device(String),
}

impl Engine {
    pub fn new(cfg: &Config, shared: Arc<Shared>) -> Result<Engine, EngineError> {
        let sockets = Sockets::bind(cfg.port).map_err(EngineError::Port)?;
        let mut producers = Vec::with_capacity(SLOTS);
        let mut consumers = Vec::with_capacity(SLOTS);
        for _ in 0..SLOTS {
            let (p, c) = rtrb::RingBuffer::<i16>::new(96_000);
            producers.push(p);
            consumers.push(c);
        }
        let mut peers = Vec::new();
        for p in &cfg.peers {
            peers.push(net::parse_peer(p, cfg.port).map_err(EngineError::Device)?);
        }
        let table = net::Table::new(producers, cfg.frame_samples() as usize);
        net::spawn(sockets.clone(), shared.clone(), table, NetCfg { peers });

        let host = cpal::default_host();
        let devices = audio::enumerate(&host);
        let in_idx = audio::find(&devices.inputs, &cfg.input, &devices.default_input);
        let out_idx = audio::find(&devices.outputs, &cfg.output, &devices.default_output);
        if cfg.input.is_some() && in_idx.is_none() {
            return Err(EngineError::Device(format!("Aufnahmegerät nicht gefunden: {}", cfg.input.clone().unwrap_or_default())));
        }
        if cfg.output.is_some() && out_idx.is_none() {
            return Err(EngineError::Device(format!("Wiedergabegerät nicht gefunden: {}", cfg.output.clone().unwrap_or_default())));
        }

        let mut e = Engine {
            devices,
            in_idx: None,
            out_idx: None,
            in_error: None,
            out_error: None,
            capture: None,
            playback: None,
            desktop: None,
            consumers: Arc::new(Mutex::new(consumers)),
            sockets,
            shared,
        };
        e.select_input(in_idx);
        e.select_output(out_idx);
        Ok(e)
    }

    pub fn select_input(&mut self, idx: Option<usize>) {
        self.capture = None;
        self.in_error = None;
        self.in_idx = idx;
        if let Some(i) = idx {
            if let Some((name, dev)) = self.devices.inputs.get(i) {
                match audio::open_capture(dev, self.shared.clone(), Wire::new(self.sockets.clone(), self.shared.clone())) {
                    Ok(c) => {
                        eprintln!("Mikrofon: {name} ({} Hz, {} Kanäle)", c.rate, c.channels);
                        self.capture = Some(c);
                    }
                    Err(e) => self.in_error = Some(e),
                }
            }
        }
    }

    pub fn select_output(&mut self, idx: Option<usize>) {
        self.playback = None;
        self.out_error = None;
        self.out_idx = idx;
        if let Some(i) = idx {
            if let Some((name, dev)) = self.devices.outputs.get(i) {
                match audio::open_playback(dev, self.shared.clone(), self.consumers.clone()) {
                    Ok(p) => {
                        eprintln!("Ausgabe: {name} ({} Hz, {} Kanäle)", p.rate, p.channels);
                        self.playback = Some(p);
                    }
                    Err(e) => self.out_error = Some(e),
                }
            }
        }
    }

    pub fn input_name(&self) -> Option<&str> {
        self.in_idx.and_then(|i| self.devices.inputs.get(i)).map(|(n, _)| n.as_str())
    }

    pub fn output_name(&self) -> Option<&str> {
        self.out_idx.and_then(|i| self.devices.outputs.get(i)).map(|(n, _)| n.as_str())
    }

    /// Desktop-Audio starten/stoppen; Quelle aus dem gemeinsamen Zustand.
    pub fn set_desktop(&mut self, on: bool) {
        self.desktop = None;
        if let Ok(mut e) = self.shared.desktop_error.lock() {
            *e = None;
        }
        self.shared.desktop_on.store(false, std::sync::atomic::Ordering::Relaxed);
        self.shared.desktop_level.store(0, std::sync::atomic::Ordering::Relaxed);
        if !on {
            return;
        }
        let src = desktop::Source::parse(&self.shared.desktop_source.lock().map(|s| s.clone()).unwrap_or_default());
        match desktop::start(&src, self.shared.clone(), self.sockets.clone(), &self.devices) {
            Ok(c) => {
                self.desktop = Some(c);
                self.shared.desktop_on.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            Err(e) => {
                eprintln!("Desktop-Audio: {e}");
                if let Ok(mut g) = self.shared.desktop_error.lock() {
                    *g = Some(e);
                }
            }
        }
    }

    pub fn desktop_running(&self) -> bool {
        self.desktop.is_some()
    }

    /// Den anderen Bescheid sagen, bevor wir gehen.
    pub fn leave(&self) {
        net::send_leave(&self.sockets, &self.shared);
    }
}
