//! Das eine Fenster. Liest nur Atomics, zeichnet mit 30 Hz.
//! Kopfzeile mit Status, drei Karten (Verbindung, Mikrofon, Partner),
//! darunter der grosse Live/Stumm-Knopf.

use crate::config::Config;
use crate::engine::Engine;
use crate::state::{lin_to_db, Shared, MAX_TARGET};
use crate::tray;
use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::net::Ipv4Addr;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::time::{Duration, Instant};

// Farbschema: dunkel, ruhig, ein Akzent.
const BG: Color32 = Color32::from_rgb(0x12, 0x14, 0x18);
const CARD: Color32 = Color32::from_rgb(0x1b, 0x1e, 0x25);
const CARD_EDGE: Color32 = Color32::from_rgb(0x26, 0x2a, 0x33);
const FIELD: Color32 = Color32::from_rgb(0x23, 0x27, 0x30);
const TEXT: Color32 = Color32::from_rgb(0xe6, 0xe8, 0xec);
const MUTED_TEXT: Color32 = Color32::from_rgb(0x8b, 0x91, 0x9c);
const GREEN: Color32 = Color32::from_rgb(0x2e, 0xc2, 0x8a);
const RED: Color32 = Color32::from_rgb(0xe8, 0x5d, 0x5d);
const AMBER: Color32 = Color32::from_rgb(0xf0, 0xb4, 0x3a);
const BLUE: Color32 = Color32::from_rgb(0x4d, 0x9d, 0xf5);
const METER_OFF: Color32 = Color32::from_rgb(0x2b, 0x30, 0x3a);

pub struct App {
    shared: Arc<Shared>,
    engine: Engine,
    cfg: Config,
    hotkey_error: Option<String>,
    tray_error: Option<String>,
    last_muted: bool,
    theme_set: bool,
    volume: f32,
    mic_gain_db: f32,
    gate_on: bool,
    denoise: bool,
    jitter_choice: u32,
    peer_edit: String,
    peer_error: Option<String>,
    meters: [Meter; 2],
}

/// Spitzenwert-Haltung für eine Pegelanzeige.
struct Meter {
    peak: f32,
    peak_at: Instant,
}

impl Meter {
    fn new() -> Self {
        Meter { peak: 0.0, peak_at: Instant::now() }
    }

    fn feed(&mut self, v: f32) -> f32 {
        let age = self.peak_at.elapsed().as_secs_f32();
        if v >= self.peak || age > 1.2 {
            self.peak = v;
            self.peak_at = Instant::now();
        } else if age > 0.6 {
            self.peak = (self.peak - 0.02).max(v);
        }
        self.peak
    }
}

impl App {
    pub fn new(shared: Arc<Shared>, engine: Engine, cfg: Config, cc: &eframe::CreationContext<'_>, start_hidden: bool) -> Self {
        let hwnd = match cc.window_handle() {
            Ok(h) => match h.as_raw() {
                RawWindowHandle::Win32(w) => w.hwnd.get() as isize,
                _ => 0,
            },
            Err(_) => 0,
        };
        let report = tray::init(shared.clone(), cc.egui_ctx.clone(), hwnd, &cfg.hotkey, start_hidden);
        let volume = shared.volume_f() * 100.0;
        let jitter_choice = if shared.jitter_auto.load(Relaxed) { 0 } else { shared.target_frames.load(Relaxed) };
        App {
            hotkey_error: report.hotkey_error,
            tray_error: report.tray_error,
            last_muted: false,
            theme_set: false,
            volume,
            mic_gain_db: cfg.mic_gain_db,
            gate_on: cfg.gate_on,
            denoise: cfg.denoise,
            jitter_choice,
            peer_edit: cfg.peer.clone().unwrap_or_default(),
            peer_error: None,
            meters: [Meter::new(), Meter::new()],
            shared,
            engine,
            cfg,
        }
    }

    fn apply_theme(ctx: &egui::Context) {
        ctx.set_zoom_factor(1.15);
        let mut v = egui::Visuals::dark();
        v.panel_fill = BG;
        v.window_fill = BG;
        v.extreme_bg_color = FIELD;
        v.faint_bg_color = CARD;
        v.override_text_color = Some(TEXT);
        v.selection.bg_fill = GREEN.linear_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, GREEN);
        v.hyperlink_color = BLUE;
        let r = CornerRadius::same(6);
        for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            w.corner_radius = r;
        }
        v.widgets.inactive.bg_fill = FIELD;
        v.widgets.inactive.weak_bg_fill = FIELD;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, CARD_EDGE);
        v.widgets.hovered.bg_fill = Color32::from_rgb(0x2c, 0x31, 0x3c);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x2c, 0x31, 0x3c);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x3a, 0x40, 0x4c));
        v.widgets.active.bg_fill = Color32::from_rgb(0x33, 0x39, 0x45);
        v.widgets.active.weak_bg_fill = Color32::from_rgb(0x33, 0x39, 0x45);
        v.widgets.open.bg_fill = FIELD;
        v.widgets.open.weak_bg_fill = FIELD;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, CARD_EDGE);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, MUTED_TEXT);
        v.slider_trailing_fill = true;
        ctx.set_visuals(v);
        ctx.style_mut(|st| {
            st.spacing.item_spacing = egui::vec2(10.0, 8.0);
            st.spacing.button_padding = egui::vec2(12.0, 6.0);
            st.spacing.combo_width = 200.0;
            st.spacing.interact_size.y = 26.0;
        });
    }

    fn hints(&self) -> Vec<(Color32, String)> {
        let s = &self.shared;
        let mut v = Vec::new();
        if let Some(e) = &self.engine.in_error {
            v.push((RED, format!("Mikrofon: {e}")));
        }
        if let Some(e) = &self.engine.out_error {
            v.push((RED, format!("Ausgabe: {e}")));
        }
        if let Ok(ne) = s.net_error.lock() {
            if let Some(e) = ne.as_ref() {
                v.push((RED, e.clone()));
            }
        }
        for e in [&self.hotkey_error, &self.tray_error, &self.peer_error].into_iter().flatten() {
            v.push((AMBER, e.clone()));
        }
        if let Ok(sp) = s.second_peer.lock() {
            if let Some(n) = sp.as_ref() {
                v.push((AMBER, format!("Zweiter Rechner gesehen und ignoriert: {n}")));
            }
        }
        if s.connected() {
            if s.jitter_us.load(Relaxed) > 20_000 {
                v.push((AMBER, "Jitter über 20 ms. WLAN-Rechner auf 5 GHz, Headset-Dongle weg vom Gehäuse.".into()));
            }
            if s.loss_permille.load(Relaxed) > 20 {
                v.push((AMBER, "Über 2 % Paketverlust. Das Funknetz ist überlastet oder gestört.".into()));
            }
        } else if s.peer_addr().is_none() {
            v.push((AMBER, "Kein Gegenüber gefunden. Läuft Holler drüben? Firewall für private Netze erlauben, sonst IP eintragen.".into()));
        }
        v
    }

    fn card(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
        egui::Frame::new()
            .fill(CARD)
            .stroke(Stroke::new(1.0, CARD_EDGE))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::symmetric(16, 14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().slider_width = (ui.available_width() - 190.0).max(120.0);
                ui.label(RichText::new(title.to_uppercase()).color(MUTED_TEXT).size(11.0).strong());
                ui.add_space(4.0);
                add(ui);
            });
        ui.add_space(10.0);
    }

    /// Segmentierte Pegelanzeige mit Spitzenwert. `level` ist RMS 0..1.
    fn meter(ui: &mut egui::Ui, meter: &mut Meter, level: f32, clip: bool, dim: bool) {
        let v = (level * 3.0).clamp(0.0, 1.0);
        let peak = meter.feed(v);
        let h = 12.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::hover());
        let p = ui.painter();
        let n = 36usize;
        let gap = 2.0;
        let w = (rect.width() - gap * (n as f32 - 1.0)) / n as f32;
        let lit = (v * n as f32).round() as usize;
        let peak_i = ((peak * n as f32).round() as usize).clamp(0, n);
        for i in 0..n {
            let x = rect.left() + i as f32 * (w + gap);
            let seg = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(w, h));
            let frac = (i + 1) as f32 / n as f32;
            let on_color = if clip {
                RED
            } else if frac > 0.9 {
                RED
            } else if frac > 0.72 {
                AMBER
            } else {
                GREEN
            };
            let color = if dim {
                if i < lit { Color32::from_rgb(0x4a, 0x50, 0x5c) } else { METER_OFF }
            } else if i < lit {
                on_color
            } else if i + 1 == peak_i && peak_i > lit {
                on_color.linear_multiply(0.8)
            } else {
                METER_OFF
            };
            p.rect_filled(seg, CornerRadius::same(2), color);
        }
    }

    fn device_combo(ui: &mut egui::Ui, id: &str, list: &[(String, cpal::Device)], current: Option<usize>, none_label: Option<&str>) -> Option<usize> {
        let mut sel = current;
        let text = current.and_then(|i| list.get(i)).map(|(n, _)| n.as_str()).unwrap_or(none_label.unwrap_or("— kein Gerät —"));
        egui::ComboBox::from_id_salt(id).width(ui.available_width()).selected_text(text).show_ui(ui, |ui| {
            if let Some(l) = none_label {
                ui.selectable_value(&mut sel, None, l);
            }
            for (i, (n, _)) in list.iter().enumerate() {
                ui.selectable_value(&mut sel, Some(i), n);
            }
        });
        sel
    }

    fn pill(ui: &mut egui::Ui, color: Color32, text: &str) {
        egui::Frame::new()
            .fill(color.linear_multiply(0.18))
            .stroke(Stroke::new(1.0, color.linear_multiply(0.6)))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::symmetric(10, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().circle_filled(r.center(), 4.0, color);
                    ui.label(RichText::new(text).color(color).size(12.5).strong());
                });
            });
    }

}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.theme_set {
            Self::apply_theme(ctx);
            self.theme_set = true;
        }

        // Schliessen = in den Tray, ausser das Tray-Menü hat „Beenden“ gewählt.
        if ctx.input(|i| i.viewport().close_requested()) && !tray::quit_requested() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.cfg.save();
            tray::hide_window();
        }

        let s = self.shared.clone();
        let muted = s.muted.load(Relaxed);
        if muted != self.last_muted {
            self.last_muted = muted;
            let title = if muted { "Holler — STUMM" } else { "Holler" };
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.to_string()));
        }

        // Verbindungszustand für Kopfzeile und Karte.
        let (conn_color, conn_short, conn_long) = match (s.peer_addr(), s.connected()) {
            (Some(a), true) => {
                let name = s.peer_name.lock().map(|n| n.clone()).unwrap_or_default();
                let name = if name.is_empty() { "Peer".to_string() } else { name };
                let m = if s.peer_muted.load(Relaxed) { "  ·  stumm" } else { "" };
                (GREEN, format!("verbunden · {name}"), format!("{name}  ({}){m}", a.ip()))
            }
            (Some(a), false) if s.ever_connected() => (RED, "keine Pakete".to_string(), format!("Verbindung zu {} abgerissen", a.ip())),
            (Some(a), false) => (AMBER, "warte".to_string(), format!("warte auf Antwort von {}", a.ip())),
            (None, _) => (MUTED_TEXT, "suche".to_string(), "suche im Netz…".to_string()),
        };

        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(Margin::symmetric(18, 14))).show(ctx, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                // ---------------- Kopfzeile ----------------
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Holler").size(22.0).strong());
                    ui.label(RichText::new("LAN-Funk ohne Umwege").color(MUTED_TEXT).size(12.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if muted {
                            Self::pill(ui, RED, "STUMM");
                        }
                        Self::pill(ui, conn_color, &conn_short);
                    });
                });
                ui.add_space(6.0);
                for (color, text) in self.hints() {
                    egui::Frame::new()
                        .fill(color.linear_multiply(0.12))
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(Margin::symmetric(10, 6))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(RichText::new(text).color(color).size(12.5));
                        });
                    ui.add_space(4.0);
                }
                ui.add_space(4.0);

                // ---------------- Verbindung ----------------
                Self::card(ui, "Verbindung", |ui| {
                    ui.label(RichText::new(&conn_long).size(15.0));
                    let sw = s.software_latency_ms();
                    ui.horizontal(|ui| {
                        let stat = |ui: &mut egui::Ui, label: &str, value: String| {
                            ui.vertical(|ui| {
                                ui.label(RichText::new(label).color(MUTED_TEXT).size(11.0));
                                ui.label(RichText::new(value).size(14.0).strong());
                            });
                            ui.add_space(14.0);
                        };
                        if s.connected() {
                            stat(ui, "Laufzeit", format!("{:.1} ms", s.rtt_us.load(Relaxed) as f32 / 2000.0));
                            stat(ui, "Jitter", format!("{:.0} ms", s.jitter_us.load(Relaxed) as f32 / 1000.0));
                            stat(ui, "Verlust", format!("{:.1} %", s.loss_permille.load(Relaxed) as f32 / 10.0));
                        }
                        stat(ui, "Software", format!("{sw:.0} ms"));
                        stat(ui, "Gesamt ≈", format!("{:.0} ms", sw + self.cfg.headset_ms as f32));
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("IP manuell").color(MUTED_TEXT));
                        let r = ui.add(egui::TextEdit::singleline(&mut self.peer_edit).desired_width(140.0).hint_text("192.168.178.42"));
                        let go = ui.button("Verbinden").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                        if go {
                            match self.peer_edit.trim().parse::<Ipv4Addr>() {
                                Ok(ip) => {
                                    self.peer_error = None;
                                    self.cfg.peer = Some(ip.to_string());
                                    s.clear_peer();
                                    s.set_peer(std::net::SocketAddr::V4(std::net::SocketAddrV4::new(ip, self.cfg.port)));
                                }
                                Err(_) => self.peer_error = Some(format!("Keine gültige IPv4-Adresse: {}", self.peer_edit.trim())),
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Puffer").color(MUTED_TEXT));
                        {
                            let target = s.target_frames.load(Relaxed);
                            let frame_ms = s.frame_ms();
                            let buffered_ms = s.buffered_samples.load(Relaxed) as f32 / 48.0;
                            let fill = buffered_ms / (MAX_TARGET as f32 * frame_ms);
                            let before = self.jitter_choice;
                            egui::ComboBox::from_id_salt("jitter")
                                .width(96.0)
                                .selected_text(if self.jitter_choice == 0 { "Auto".to_string() } else { format!("{} Rahmen", self.jitter_choice) })
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut self.jitter_choice, 0, "Auto");
                                    for n in 1..=MAX_TARGET {
                                        ui.selectable_value(&mut self.jitter_choice, n, format!("{n} Rahmen"));
                                    }
                                });
                            if before != self.jitter_choice {
                                if self.jitter_choice == 0 {
                                    s.jitter_auto.store(true, Relaxed);
                                    self.cfg.jitter = "auto".into();
                                } else {
                                    s.jitter_auto.store(false, Relaxed);
                                    s.target_frames.store(self.jitter_choice, Relaxed);
                                    self.cfg.jitter = self.jitter_choice.to_string();
                                }
                            }
                            ui.add(egui::ProgressBar::new(fill.clamp(0.0, 1.0)).desired_width(120.0).desired_height(8.0).fill(BLUE));
                            ui.label(RichText::new(format!("{buffered_ms:.0} ms gefüllt · Ziel {target} Rahmen ({:.0} ms)", target as f32 * frame_ms)).color(MUTED_TEXT).size(12.0));
                        }
                    });
                });

                // ---------------- Mikrofon ----------------
                let mut meters = std::mem::replace(&mut self.meters, [Meter::new(), Meter::new()]);
                Self::card(ui, "Mikrofon", |ui| {
                    let sel = Self::device_combo(ui, "in", &self.engine.devices.inputs, self.engine.in_idx, None);
                    if sel != self.engine.in_idx {
                        self.engine.select_input(sel);
                        self.cfg.input = self.engine.input_name().map(|n| n.to_string());
                    }
                    let level = s.mic_level_f();
                    let gate_closed = self.gate_on && !s.gate_open.load(Relaxed);
                    Self::meter(ui, &mut meters[0], level, s.mic_clip.load(Relaxed), gate_closed || muted);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{:>4.0} dB", lin_to_db(level))).color(MUTED_TEXT).size(12.0).monospace());
                        let (c, state) = if muted {
                            (RED, "stumm")
                        } else if gate_closed {
                            (MUTED_TEXT, "Sperre zu · sendet nicht")
                        } else if self.gate_on {
                            (GREEN, "sendet")
                        } else {
                            (GREEN, "sendet immer")
                        };
                        ui.label(RichText::new(state).color(c).size(12.0));
                    });
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Verstärkung").color(MUTED_TEXT));
                        if ui.add(egui::Slider::new(&mut self.mic_gain_db, 0.0..=30.0).suffix(" dB").fixed_decimals(0)).changed() {
                            s.set_mic_gain_db(self.mic_gain_db);
                            self.cfg.mic_gain_db = self.mic_gain_db;
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut self.denoise, "Rauschunterdrückung").changed() {
                            self.cfg.denoise = self.denoise;
                            s.denoise.store(self.denoise, Relaxed);
                        }
                        ui.add_space(16.0);
                        if ui.checkbox(&mut self.gate_on, "Sprechsperre").changed() {
                            self.cfg.gate_on = self.gate_on;
                            s.set_gate(self.cfg.gate());
                        }
                    });
                    ui.label(RichText::new("Rauschunterdrückung nimmt Grundrauschen, Lüfter und Tastatur aus der Stimme. Die Sprechsperre sendet nur, wenn gesprochen wird.").color(MUTED_TEXT).size(11.0));
                });

                // ---------------- Partner ----------------
                Self::card(ui, "Partner hören", |ui| {
                    let sel = Self::device_combo(ui, "out", &self.engine.devices.outputs, self.engine.out_idx, None);
                    if sel != self.engine.out_idx {
                        self.engine.select_output(sel);
                        self.cfg.output = self.engine.output_name().map(|n| n.to_string());
                    }
                    Self::meter(ui, &mut meters[1], s.spk_level_f(), false, false);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Lautstärke").color(MUTED_TEXT));
                        if ui.add(egui::Slider::new(&mut self.volume, 0.0..=300.0).suffix(" %").fixed_decimals(0)).changed() {
                            s.set_volume_f(self.volume / 100.0);
                            self.cfg.volume = self.volume.round() as u32;
                        }
                    });
                });

                self.meters = meters;

                // ---------------- Stumm ----------------
                ui.add_space(4.0);
                let (fill, edge, label) = if muted {
                    (RED.linear_multiply(0.25), RED, format!("STUMM   ·   {} schaltet frei", self.cfg.hotkey))
                } else {
                    (GREEN.linear_multiply(0.18), GREEN, format!("LIVE   ·   {} schaltet stumm", self.cfg.hotkey))
                };
                let btn = egui::Button::new(RichText::new(label).size(16.0).strong().color(edge))
                    .fill(fill)
                    .stroke(Stroke::new(1.5, edge))
                    .corner_radius(CornerRadius::same(10))
                    .min_size(egui::vec2(ui.available_width(), 46.0));
                if ui.add(btn).clicked() {
                    tray::toggle_mute();
                }
                ui.add_space(6.0);
                ui.label(RichText::new("× legt das Fenster ins Tray neben der Uhr (evtl. hinter dem Pfeil ^). Beenden über Rechtsklick auf das Tray-Symbol.").color(MUTED_TEXT).size(11.0));
                ui.label(RichText::new(format!("Holler {} · Lupus Malus Deviant", env!("CARGO_PKG_VERSION"))).color(MUTED_TEXT).size(11.0));
                ui.add_space(6.0);
            });
        });

        if muted {
            let rect = ctx.screen_rect();
            ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("mute-frame"))).rect_stroke(
                rect,
                CornerRadius::ZERO,
                Stroke::new(3.0, RED),
                egui::StrokeKind::Inside,
            );
        }

        ctx.request_repaint_after(Duration::from_millis(33));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.cfg.save();
    }
}
