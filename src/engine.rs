//! Verdrahtung: Socket, Netz-Thread, Geräte, Aufnahme- und Wiedergabestrom.
//! Lebt auf dem Hauptthread; Streams werden bei Gerätewechsel neu geöffnet.

use crate::audio::{self, Capture, Devices, Playback, SharedConsumer};
use crate::config::Config;
use crate::net::{self, NetCfg, Wire};
use crate::state::Shared;
use std::net::{Ipv4Addr, UdpSocket};
use std::sync::{Arc, Mutex};

pub struct Engine {
    pub devices: Devices,
    pub in_idx: Option<usize>,
    pub out_idx: Option<usize>,
    pub in_error: Option<String>,
    pub out_error: Option<String>,
    capture: Option<Capture>,
    playback: Option<Playback>,
    consumer: SharedConsumer,
    socket: Arc<UdpSocket>,
    shared: Arc<Shared>,
}

pub enum EngineError {
    Port(String),
    Device(String),
}

impl Engine {
    pub fn new(cfg: &Config, shared: Arc<Shared>) -> Result<Engine, EngineError> {
        let socket = net::bind(cfg.port).map_err(EngineError::Port)?;
        let (producer, consumer) = rtrb::RingBuffer::<i16>::new(48_000);
        let peer = match &cfg.peer {
            Some(p) => Some(p.trim().parse::<Ipv4Addr>().map_err(|_| EngineError::Device(format!("Peer-Adresse unlesbar: {p}")))?),
            None => None,
        };
        net::spawn(socket.clone(), shared.clone(), producer, NetCfg { port: cfg.port, peer, name: cfg.name.clone() });

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
            consumer: Arc::new(Mutex::new(consumer)),
            socket,
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
                match audio::open_capture(dev, self.shared.clone(), Wire::new(self.socket.clone(), self.shared.clone())) {
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
                match audio::open_playback(dev, self.shared.clone(), self.consumer.clone()) {
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
}
