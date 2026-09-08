//! Das eine Fenster. Liest nur Atomics, zeichnet mit 30 Hz.
//! Kopfzeile mit Status, Karten Raum, Teilnehmer, Mikrofon, Ausgabe, darunter der Live/Stumm-Knopf.

use crate::codec;
use crate::config::Config;
use crate::desktop;
use crate::crypto;
use crate::engine::Engine;
use crate::invite::Invite;
use crate::net;
use crate::single;
use crate::state::{lin_to_db, path_name, Shared, CODEC_OPUS, KIND_VOICE, MAX_TARGET, SLOTS};
use crate::tray;
use crate::update::{self, State as UpState, Updater};
use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke};
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
    mic_gain_db: f32,
    gate_on: bool,
    denoise: bool,
    jitter_choice: u32,
    name_edit: String,
    room_edit: String,
    pw_edit: String,
    peer_edit: String,
    peer_error: Option<String>,
    hub_edit: String,
    codec_choice: u32,
    desktop_on: bool,
    desktop_music: bool,
    desktop_gain: f32,
    desktop_source: String,
    procs: Vec<(u32, String)>,
    procs_at: Option<Instant>,
    invite_tick: u32,
    copied_at: Option<Instant>,
    meters: Vec<Meter>,
    updater: Arc<Updater>,
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

const METER_MIC: usize = SLOTS;
const METER_MIX: usize = SLOTS + 1;
const METER_DESK: usize = SLOTS + 2;

impl App {
    pub fn new(shared: Arc<Shared>, engine: Engine, cfg: Config, cc: &eframe::CreationContext<'_>, start_hidden: bool, updater: Arc<Updater>) -> Self {
        #[cfg(windows)]
        let hwnd = {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            match cc.window_handle() {
                Ok(h) => match h.as_raw() {
                    RawWindowHandle::Win32(w) => w.hwnd.get() as isize,
                    _ => 0,
                },
                Err(_) => 0,
            }
        };
        #[cfg(not(windows))]
        let hwnd: isize = 0;
        let report = tray::init(shared.clone(), cc.egui_ctx.clone(), hwnd, &cfg.hotkey, start_hidden);
        let jitter_choice = if shared.jitter_auto.load(Relaxed) { 0 } else { shared.jitter_fixed.load(Relaxed) };
        App {
            hotkey_error: report.hotkey_error,
            tray_error: report.tray_error,
            last_muted: false,
            theme_set: false,
            mic_gain_db: cfg.mic_gain_db,
            gate_on: cfg.gate_on,
            denoise: cfg.denoise,
            jitter_choice,
            name_edit: cfg.name.clone(),
            room_edit: cfg.room.clone(),
            pw_edit: cfg.room_password.clone(),
            peer_edit: String::new(),
            peer_error: None,
            hub_edit: cfg.hub.clone(),
            codec_choice: codec::parse_choice(&cfg.codec),
            desktop_on: cfg.desktop_on,
            desktop_music: cfg.desktop_quality.trim().eq_ignore_ascii_case("musik"),
            desktop_gain: cfg.desktop_gain.min(300) as f32,
            desktop_source: cfg.desktop_source.clone(),
            procs: Vec::new(),
            procs_at: None,
            invite_tick: 0,
            copied_at: None,
            meters: (0..SLOTS + 3).map(|_| Meter::new()).collect(),
            updater,
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
        if let Some(e) = s.desktop_error.lock().ok().and_then(|g| g.clone()) {
            v.push((AMBER, format!("Desktop-Audio: {e}")));
        }
        if s.room_full.load(Relaxed) {
            v.push((AMBER, "Raum voll: mehr als 8 Teilnehmer, jemand wurde abgewiesen.".into()));
        }
        if s.bad_auth.load(Relaxed) > 0 {
            v.push((AMBER, "Jemand sendet mit anderem Passwort oder ohne Raum. Diese Pakete werden verworfen.".into()));
        }
        if s.foreign_room.load(Relaxed) > 0 && s.peer_count() == 0 {
            v.push((AMBER, "Im Netz ist jemand in einem anderen Raum. Gleicher Raumname und gleiches Passwort nötig.".into()));
        }
        let now = s.now_ms();
        for p in s.peers.iter().filter(|p| p.active.load(Relaxed)) {
            if p.streaming(now) && p.jitter_us.load(Relaxed) > 20_000 {
                v.push((AMBER, format!("{}: Jitter über 20 ms. WLAN auf 5 GHz, Headset-Dongle weg vom Gehäuse.", p.name())));
            }
            if p.streaming(now) && p.loss_permille.load(Relaxed) > 20 {
                v.push((AMBER, format!("{}: über 2 % Paketverlust, Funknetz überlastet oder gestört.", p.name())));
            }
        }
        if s.peer_count() == 0 && !s.room_busy.load(Relaxed) {
            v.push((AMBER, "Noch niemand da. Läuft Holler drüben? Firewall für private Netze erlauben, sonst IP eintragen.".into()));
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
    fn meter(ui: &mut egui::Ui, meter: &mut Meter, level: f32, clip: bool, dim: bool, width: f32) {
        let v = (level * 3.0).clamp(0.0, 1.0);
        let peak = meter.feed(v);
        let h = 12.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, h), egui::Sense::hover());
        let p = ui.painter();
        let n = (width / 9.0).clamp(12.0, 40.0) as usize;
        let gap = 2.0;
        let w = (rect.width() - gap * (n as f32 - 1.0)) / n as f32;
        let lit = (v * n as f32).round() as usize;
        let peak_i = ((peak * n as f32).round() as usize).clamp(0, n);
        for i in 0..n {
            let x = rect.left() + i as f32 * (w + gap);
            let seg = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(w, h));
            let frac = (i + 1) as f32 / n as f32;
            let on_color = if clip || frac > 0.9 {
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

    fn device_combo(ui: &mut egui::Ui, id: &str, list: &[(String, cpal::Device)], current: Option<usize>) -> Option<usize> {
        let mut sel = current;
        let text = current.and_then(|i| list.get(i)).map(|(n, _)| n.as_str()).unwrap_or("— kein Gerät —");
        egui::ComboBox::from_id_salt(id).width(ui.available_width()).selected_text(text).show_ui(ui, |ui| {
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

    fn dot(ui: &mut egui::Ui, color: Color32, filled: bool) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
        if filled {
            ui.painter().circle_filled(rect.center(), 5.0, color);
        } else {
            ui.painter().circle_stroke(rect.center(), 5.0, Stroke::new(1.5, color));
        }
    }

    fn join_room(&mut self) {
        if let Some(inv) = Invite::parse(&self.room_edit) {
            self.room_edit = inv.room;
            self.pw_edit = inv.password;
            if let Some(h) = inv.hub {
                self.hub_edit = h.clone();
                self.cfg.hub = h;
                self.shared.set_hub(&self.cfg.hub);
            }
        }
        let name = self.room_edit.trim().to_string();
        let pw = self.pw_edit.clone();
        self.cfg.room = name.clone();
        self.cfg.room_password = pw.clone();
        self.cfg.save();
        self.engine.leave();
        if name.is_empty() {
            self.shared.set_room(None);
            return;
        }
        self.shared.room_busy.store(true, Relaxed);
        self.shared.clear_peers();
        let sh = self.shared.clone();
        std::thread::Builder::new()
            .name("holler-room".into())
            .spawn(move || {
                let room = crypto::derive(&name, &pw);
                eprintln!("Raum „{}“ beigetreten, Pakete verschlüsselt.", room.name);
                sh.set_room(Some(room));
                sh.room_busy.store(false, Relaxed);
            })
            .ok();
    }

    fn leave_room(&mut self) {
        self.engine.leave();
        self.shared.set_room(None);
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.theme_set {
            Self::apply_theme(ctx);
            self.theme_set = true;
        }

        // Schliessen = in den Tray, ausser das Tray-Menü hat „Beenden“ gewählt.
        if cfg!(windows) && ctx.input(|i| i.viewport().close_requested()) && !tray::quit_requested() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.cfg.save();
            tray::hide_window();
        }

        self.invite_tick += 1;
        if self.invite_tick % 45 == 0 {
            if let Some(url) = single::take_invite() {
                if Invite::parse(&url).is_some() {
                    self.room_edit = url;
                    self.join_room();
                }
            }
        }

        let s = self.shared.clone();
        let now = s.now_ms();
        let muted = s.muted.load(Relaxed);
        if muted != self.last_muted {
            self.last_muted = muted;
            let title = if muted { "Holler — STUMM" } else { "Holler" };
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.to_string()));
        }

        let room = s.room();
        let busy = s.room_busy.load(Relaxed);
        let count = s.peer_count();
        let (conn_color, conn_short) = if busy {
            (AMBER, "Schlüssel …".to_string())
        } else if s.connected() {
            (GREEN, if room.is_some() { format!("im Raum · {count}") } else { format!("LAN · {count}") })
        } else if count > 0 {
            (AMBER, format!("{count} da, kein Audio"))
        } else {
            (MUTED_TEXT, "suche".to_string())
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

                // Update-Hinweis
                let up = self.updater.state();
                match &up {
                    UpState::Available(info) | UpState::Downloading(info, _) => {
                        let info = info.clone();
                        egui::Frame::new()
                            .fill(BLUE.linear_multiply(0.14))
                            .stroke(Stroke::new(1.0, BLUE.linear_multiply(0.5)))
                            .corner_radius(CornerRadius::same(8))
                            .inner_margin(Margin::symmetric(12, 8))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    if let UpState::Downloading(_, pct) = up {
                                        ui.label(RichText::new(format!("Version {} wird geladen … {pct} %", info.version)).color(BLUE).strong());
                                        ui.add(egui::ProgressBar::new(pct as f32 / 100.0).desired_width(120.0).desired_height(8.0).fill(BLUE));
                                    } else {
                                        ui.label(RichText::new(format!("Version {} ist verfügbar (installiert: {})", info.version, update::VERSION)).color(BLUE).strong());
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if update::is_installed() {
                                                if ui.button("Jetzt aktualisieren").clicked() {
                                                    self.cfg.save();
                                                    self.updater.spawn_install(info.clone());
                                                }
                                            } else {
                                                ui.hyperlink_to("Download-Seite öffnen", &info.page_url);
                                                ui.label(RichText::new("portable Exe:").color(MUTED_TEXT).size(12.0));
                                            }
                                        });
                                    }
                                });
                            });
                        ui.add_space(4.0);
                    }
                    UpState::ReadyToQuit => {
                        if !tray::quit_requested() {
                            tray::quit();
                        }
                    }
                    _ => {}
                }
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

                // ---------------- Raum ----------------
                let mut do_join = false;
                let mut do_leave = false;
                Self::card(ui, "Raum", |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Mein Name").color(MUTED_TEXT));
                        if ui.add(egui::TextEdit::singleline(&mut self.name_edit).desired_width(180.0)).changed() {
                            let n = self.name_edit.trim().to_string();
                            if !n.is_empty() {
                                if let Ok(mut g) = s.name.lock() {
                                    *g = n.clone();
                                }
                                self.cfg.name = n;
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Raum").color(MUTED_TEXT));
                        let r1 = ui.add_enabled(room.is_none() && !busy, egui::TextEdit::singleline(&mut self.room_edit).desired_width(150.0).hint_text("Name oder holler://-Link"));
                        ui.label(RichText::new("Passwort").color(MUTED_TEXT));
                        let r2 = ui.add_enabled(room.is_none() && !busy, egui::TextEdit::singleline(&mut self.pw_edit).desired_width(130.0).password(true));
                        let enter = (r1.lost_focus() || r2.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if room.is_some() {
                            if ui.button("Verlassen").clicked() {
                                do_leave = true;
                            }
                            let label = if self.copied_at.is_some_and(|t| t.elapsed().as_secs_f32() < 2.0) { "Kopiert!" } else { "Einladungslink" };
                            if ui.button(label).on_hover_text("Link in die Zwischenablage: Raum, Passwort und Hub. Der andere fügt ihn im Raumfeld ein oder öffnet ihn.").clicked() {
                                let inv = Invite { room: self.cfg.room.clone(), password: self.cfg.room_password.clone(), hub: Some(self.cfg.hub.clone()) };
                                ui.ctx().copy_text(inv.to_url());
                                self.copied_at = Some(Instant::now());
                            }
                        } else if busy {
                            ui.add_enabled(false, egui::Button::new("Schlüssel …"));
                        } else if ui.button("Beitreten").clicked() || enter {
                            do_join = true;
                        }
                    });
                    let sw = s.software_latency_ms();
                    let status = if busy {
                        "Raumschlüssel wird berechnet, dauert einen Moment …".to_string()
                    } else if let Some(r) = &room {
                        format!("Im Raum „{}“: alle Pakete verschlüsselt, nur wer Raum und Passwort kennt, hört mit.", r.name)
                    } else {
                        "Ohne Raum: offenes LAN wie bisher, unverschlüsselt. Für Verschlüsselung Raum und Passwort setzen.".to_string()
                    };
                    ui.label(RichText::new(status).color(MUTED_TEXT).size(12.0));
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Vermittler").color(MUTED_TEXT));
                        let r = ui.add(egui::TextEdit::singleline(&mut self.hub_edit).desired_width(200.0).hint_text("host:port, leer = nur LAN"));
                        if r.lost_focus() && self.hub_edit.trim() != self.cfg.hub.trim() {
                            self.cfg.hub = self.hub_edit.trim().to_string();
                            self.cfg.save();
                            s.set_hub(&self.cfg.hub);
                        }
                        let hub_text = if let Some(e) = s.hub_error.lock().ok().and_then(|g| g.clone()) {
                            (RED, e)
                        } else if !s.hub_configured() {
                            (MUTED_TEXT, "kein Vermittler, nur LAN".to_string())
                        } else if room.is_none() {
                            (MUTED_TEXT, "wird mit dem Raum verbunden".to_string())
                        } else if s.hub_alive() {
                            let pub_addr = s.public_addr.lock().ok().and_then(|g| *g).map(|a| format!(" · von aussen {a}")).unwrap_or_default();
                            (GREEN, format!("verbunden · {:.0} ms{pub_addr}", s.hub_rtt_us.load(Relaxed) as f32 / 1000.0))
                        } else {
                            (AMBER, "keine Antwort (UDP 4712 offen? Adresse richtig?)".to_string())
                        };
                        ui.label(RichText::new(hub_text.1).color(hub_text.0).size(12.0));
                    });
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Qualität für Ferne").color(MUTED_TEXT));
                        let before = self.codec_choice;
                        egui::ComboBox::from_id_salt("codec")
                            .width(150.0)
                            .selected_text(codec::choice_label(self.codec_choice))
                            .show_ui(ui, |ui| {
                                for (k, label, _) in codec::CHOICES {
                                    ui.selectable_value(&mut self.codec_choice, k, label);
                                }
                            });
                        if before != self.codec_choice {
                            s.codec_kbps.store(self.codec_choice, Relaxed);
                            self.cfg.codec = codec::choice_key(self.codec_choice).to_string();
                            self.cfg.save();
                        }
                        let expl = codec::CHOICES.iter().find(|c| c.0 == self.codec_choice).map(|c| c.2).unwrap_or("");
                        ui.label(RichText::new(expl).color(MUTED_TEXT).size(11.5));
                    });
                    ui.label(RichText::new("Gilt für das eigene Senden an alle, die nicht im selben Netz sind. Im LAN geht immer rohes PCM.").color(MUTED_TEXT).size(11.0));
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("IP manuell").color(MUTED_TEXT));
                        let r = ui.add(egui::TextEdit::singleline(&mut self.peer_edit).desired_width(170.0).hint_text("192.168.1.5 oder [fe80::1]:4711"));
                        let go = ui.button("Hinzufügen").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                        if go && !self.peer_edit.trim().is_empty() {
                            match net::parse_peer(&self.peer_edit, self.cfg.port) {
                                Ok(a) => {
                                    self.peer_error = None;
                                    if let Ok(mut m) = s.manual_peers.lock() {
                                        if !m.contains(&a) {
                                            m.push(a);
                                        }
                                    }
                                    if !self.cfg.peers.iter().any(|p| p == self.peer_edit.trim()) {
                                        self.cfg.peers.push(self.peer_edit.trim().to_string());
                                        self.cfg.save();
                                    }
                                    self.peer_edit.clear();
                                }
                                Err(e) => self.peer_error = Some(e),
                            }
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(format!("Software ≈ {sw:.0} ms + Funk {} ms", self.cfg.headset_ms)).color(MUTED_TEXT).size(12.0));
                        });
                    });
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Puffer").color(MUTED_TEXT));
                        let before = self.jitter_choice;
                        egui::ComboBox::from_id_salt("jitter")
                            .width(110.0)
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
                                s.jitter_fixed.store(self.jitter_choice, Relaxed);
                                self.cfg.jitter = self.jitter_choice.to_string();
                            }
                        }
                        let manual: Vec<String> = s.manual_peers.lock().map(|m| m.iter().map(|a| a.to_string()).collect()).unwrap_or_default();
                        if !manual.is_empty() {
                            ui.label(RichText::new(format!("feste Adressen: {}", manual.join(", "))).color(MUTED_TEXT).size(12.0));
                        }
                    });
                });
                if do_join {
                    self.join_room();
                }
                if do_leave {
                    self.leave_room();
                }

                // ---------------- Teilnehmer ----------------
                let mut meters = std::mem::take(&mut self.meters);
                Self::card(ui, &format!("Teilnehmer · {count}"), |ui| {
                    let mut any = false;
                    let mut order: Vec<usize> = (0..SLOTS).filter(|&i| s.peers[i].active.load(Relaxed) && s.peers[i].kind.load(Relaxed) == KIND_VOICE).collect();
                    order.sort_by_key(|&i| (s.peers[i].path.load(Relaxed), s.peers[i].joined_ms.load(Relaxed)));
                    for i in order {
                        let p = &s.peers[i];
                        any = true;
                        let streaming = p.streaming(now);
                        let ever = p.last_rx_ms.load(Relaxed) != u64::MAX;
                        let (color, filled) = if streaming { (GREEN, true) } else if ever { (RED, true) } else { (AMBER, false) };
                        ui.horizontal(|ui| {
                            Self::dot(ui, color, filled);
                            let name = p.name();
                            ui.label(RichText::new(if name.is_empty() { "…".to_string() } else { name }).size(14.0).strong());
                            let m = if p.remote_muted.load(Relaxed) { " · stumm" } else { "" };
                            let codec_text = if p.codec.load(Relaxed) == CODEC_OPUS { format!("Opus {} kbit/s", p.codec_kbps.load(Relaxed)) } else { "PCM".to_string() };
                            let info = format!(
                                "{} · {:.1} ms · Jitter {:.0} · Puffer {}/{:.0} ms · {codec_text}{m}",
                                path_name(p.path.load(Relaxed)),
                                p.rtt_us.load(Relaxed) as f32 / 2000.0,
                                p.jitter_us.load(Relaxed) as f32 / 1000.0,
                                p.target_frames.load(Relaxed),
                                p.buffered_samples.load(Relaxed) as f32 / 48.0,
                            );
                            ui.label(RichText::new(info).color(MUTED_TEXT).size(11.5));
                        });
                        ui.horizontal(|ui| {
                            ui.add_space(22.0);
                            let lm = p.local_mute.load(Relaxed);
                            Self::meter(ui, &mut meters[i], p.level_f(), false, lm || !streaming, 150.0);
                            let mut vol = p.volume_f() * 100.0;
                            ui.spacing_mut().slider_width = 150.0;
                            if ui.add(egui::Slider::new(&mut vol, 0.0..=300.0).suffix(" %").fixed_decimals(0)).changed() {
                                p.volume.store((vol / 100.0).to_bits(), Relaxed);
                                let id = p.id.load(Relaxed);
                                if let Ok(mut m) = s.volumes.lock() {
                                    m.insert(id, vol / 100.0);
                                }
                                self.cfg.volumes.insert(format!("{id:016x}"), vol.round() as u32);
                            }
                            let label = if lm { "Ton an" } else { "Ton aus" };
                            let btn = egui::Button::new(RichText::new(label).size(12.0));
                            let btn = if lm { btn.fill(RED.linear_multiply(0.3)) } else { btn };
                            if ui.add(btn).on_hover_text("Nur bei mir stumm, die anderen hören die Person weiter").clicked() {
                                p.local_mute.store(!lm, Relaxed);
                            }
                        });
                        // Desktop-Kanal dieser Person
                        if let Some(d) = s.find_desktop(p.id.load(Relaxed)) {
                            let dp = &s.peers[d];
                            let dstreaming = dp.streaming(now);
                            ui.horizontal(|ui| {
                                ui.add_space(22.0);
                                ui.label(RichText::new("Desktop").color(BLUE).size(12.0).strong());
                                let codec_text = if dp.codec.load(Relaxed) == CODEC_OPUS { format!("Opus {} kbit/s", dp.codec_kbps.load(Relaxed)) } else { "PCM".to_string() };
                                ui.label(RichText::new(format!("{} · {} · Puffer {}/{:.0} ms", if dp.channels.load(Relaxed) == 2 { "stereo" } else { "mono" }, codec_text, dp.target_frames.load(Relaxed), dp.buffered_samples.load(Relaxed) as f32 / 48.0)).color(MUTED_TEXT).size(11.5));
                            });
                            ui.horizontal(|ui| {
                                ui.add_space(22.0);
                                let lm = dp.local_mute.load(Relaxed);
                                Self::meter(ui, &mut meters[d], dp.level_f(), false, lm || !dstreaming, 150.0);
                                let mut vol = dp.volume_f() * 100.0;
                                ui.spacing_mut().slider_width = 150.0;
                                if ui.add(egui::Slider::new(&mut vol, 0.0..=300.0).suffix(" %").fixed_decimals(0)).changed() {
                                    dp.volume.store((vol / 100.0).to_bits(), Relaxed);
                                }
                                let label = if lm { "Ton an" } else { "Ton aus" };
                                let btn = egui::Button::new(RichText::new(label).size(12.0));
                                let btn = if lm { btn.fill(RED.linear_multiply(0.3)) } else { btn };
                                if ui.add(btn).on_hover_text("Desktop-Audio dieser Person nur bei mir aus").clicked() {
                                    dp.local_mute.store(!lm, Relaxed);
                                }
                            });
                        }
                        ui.add_space(4.0);
                    }
                    if !any {
                        let text = if busy {
                            "Schlüssel wird berechnet …"
                        } else if room.is_some() {
                            "Noch niemand im Raum. Im selben Netz findet Holler die anderen von selbst, sonst IP oben eintragen."
                        } else {
                            "Noch niemand da. Im selben Netz findet Holler die anderen von selbst, sonst IP oben eintragen."
                        };
                        ui.label(RichText::new(text).color(MUTED_TEXT).size(12.0));
                    }
                });

                // ---------------- Mikrofon ----------------
                Self::card(ui, "Mikrofon", |ui| {
                    let sel = Self::device_combo(ui, "in", &self.engine.devices.inputs, self.engine.in_idx);
                    if sel != self.engine.in_idx {
                        self.engine.select_input(sel);
                        self.cfg.input = self.engine.input_name().map(|n| n.to_string());
                    }
                    let level = s.mic_level_f();
                    let gate_closed = self.gate_on && !s.gate_open.load(Relaxed);
                    let w = ui.available_width();
                    Self::meter(ui, &mut meters[METER_MIC], level, s.mic_clip.load(Relaxed), gate_closed || muted, w);
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

                // ---------------- Desktop-Audio ----------------
                let mut desktop_restart = false;
                Self::card(ui, "Desktop-Audio", |ui| {
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut self.desktop_on, "Desktop-Audio senden").changed() {
                            desktop_restart = true;
                        }
                        let running = self.engine.desktop_running();
                        let (c, t) = if running { (GREEN, "läuft") } else if self.desktop_on { (AMBER, "startet nicht") } else { (MUTED_TEXT, "aus") };
                        ui.label(RichText::new(t).color(c).size(12.0));
                    });
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Quelle").color(MUTED_TEXT));
                        // Programmliste (Windows) alle 3 s auffrischen, solange die Karte sichtbar ist.
                        if cfg!(windows) && self.procs_at.is_none_or(|t| t.elapsed().as_secs_f32() > 3.0) {
                            self.procs = desktop::list_processes();
                            self.procs_at = Some(Instant::now());
                        }
                        let current = desktop::Source::parse(&self.desktop_source);
                        let mut sel = self.desktop_source.clone();
                        egui::ComboBox::from_id_salt("desk-src").width(320.0).selected_text(current.label()).show_ui(ui, |ui| {
                            if cfg!(windows) {
                                ui.selectable_value(&mut sel, "all".to_string(), "Alles ausser Holler");
                                for (pid, name) in &self.procs {
                                    ui.selectable_value(&mut sel, format!("pid:{pid}:{name}"), format!("Nur {name}"));
                                }
                            }
                            for (n, _) in &self.engine.devices.inputs {
                                ui.selectable_value(&mut sel, format!("dev:{n}"), format!("Gerät: {n}"));
                            }
                        });
                        if sel != self.desktop_source {
                            self.desktop_source = sel.clone();
                            self.cfg.desktop_source = sel.clone();
                            if let Ok(mut g) = s.desktop_source.lock() {
                                *g = sel;
                            }
                            self.cfg.save();
                            if self.desktop_on {
                                desktop_restart = true;
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Qualität").color(MUTED_TEXT));
                        let before = self.desktop_music;
                        egui::ComboBox::from_id_salt("desk-q")
                            .width(180.0)
                            .selected_text(if self.desktop_music { "Musik · Opus 160" } else { "Spiel · Opus 96" })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.desktop_music, false, "Spiel · Opus 96");
                                ui.selectable_value(&mut self.desktop_music, true, "Musik · Opus 160");
                            });
                        if before != self.desktop_music {
                            s.desktop_music.store(self.desktop_music, Relaxed);
                            s.desktop_kbps.store(if self.desktop_music { 160 } else { 96 }, Relaxed);
                            self.cfg.desktop_quality = if self.desktop_music { "musik".into() } else { "spiel".into() };
                            self.cfg.save();
                        }
                        let expl = if self.desktop_music { "Stereo, 160 kbit/s, Musikprofil. Für gemeinsames Musikhören." } else { "Stereo, 96 kbit/s. Spielsound, sprachtauglich. Im LAN immer rohes PCM." };
                        ui.label(RichText::new(expl).color(MUTED_TEXT).size(11.5));
                    });
                    let w = ui.available_width();
                    Self::meter(ui, &mut meters[METER_DESK], s.desktop_level_f(), false, !self.engine.desktop_running(), w);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Sendepegel").color(MUTED_TEXT));
                        if ui.add(egui::Slider::new(&mut self.desktop_gain, 0.0..=300.0).suffix(" %").fixed_decimals(0)).changed() {
                            s.desktop_gain.store((self.desktop_gain / 100.0).to_bits(), Relaxed);
                            self.cfg.desktop_gain = self.desktop_gain.round() as u32;
                        }
                    });
                    let hint = if cfg!(windows) {
                        "„Alles ausser Holler“ nimmt jedes Programm auf, auch Discord. Läuft Discord parallel, besser „Nur <Spiel>“. Stumm (F9) betrifft nur die Stimme."
                    } else {
                        "Quelle ist ein Aufnahmegerät: unter Linux ein „Monitor of …“, unter macOS ein virtuelles Gerät wie BlackHole. Holler selbst auf ein anderes Ausgabegerät legen, sonst hört das Gegenüber sich selbst."
                    };
                    ui.label(RichText::new(hint).color(MUTED_TEXT).size(11.0));
                });
                if desktop_restart {
                    self.cfg.desktop_on = self.desktop_on;
                    self.cfg.save();
                    self.engine.set_desktop(self.desktop_on);
                }

                // ---------------- Ausgabe ----------------
                Self::card(ui, "Ausgabe", |ui| {
                    let sel = Self::device_combo(ui, "out", &self.engine.devices.outputs, self.engine.out_idx);
                    if sel != self.engine.out_idx {
                        self.engine.select_output(sel);
                        self.cfg.output = self.engine.output_name().map(|n| n.to_string());
                    }
                    let w = ui.available_width();
                    Self::meter(ui, &mut meters[METER_MIX], s.spk_level_f(), false, false, w);
                    ui.label(RichText::new("Summe aller Teilnehmer. Lautstärke pro Person in der Liste oben.").color(MUTED_TEXT).size(11.0));
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
                if cfg!(windows) {
                    ui.label(RichText::new("× legt das Fenster ins Tray neben der Uhr (evtl. hinter dem Pfeil ^). Beenden über Rechtsklick auf das Tray-Symbol.").color(MUTED_TEXT).size(11.0));
                } else {
                    ui.label(RichText::new("× beendet Holler. Kein Tray auf diesem System.").color(MUTED_TEXT).size(11.0));
                }
                let up_text = match self.updater.state() {
                    UpState::Off => "Update-Prüfung aus".to_string(),
                    UpState::Checking => "prüfe auf Updates …".to_string(),
                    UpState::UpToDate => "aktuell".to_string(),
                    UpState::Available(i) => format!("Version {} verfügbar", i.version),
                    UpState::Downloading(_, p) => format!("lade Update {p} %"),
                    UpState::ReadyToQuit => "Installer läuft, Holler startet neu".to_string(),
                    UpState::Failed(e) => format!("Update-Prüfung: {e}"),
                };
                ui.label(
                    RichText::new(format!(
                        "Holler {} · Lupus Malus Deviant · {}{}{}",
                        update::VERSION,
                        up_text,
                        if update::is_installed() { "" } else { " · portabel" },
                        if s.ipv6.load(Relaxed) { " · IPv4+IPv6" } else { " · nur IPv4" }
                    ))
                    .color(MUTED_TEXT)
                    .size(11.0),
                );
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
        self.engine.leave();
        self.cfg.save();
    }
}
