//! Update-Prüfung gegen die GitHub-Releases und stiller Installer-Lauf.
//!
//! Beim Start fragt ein Hintergrund-Thread das neueste Release ab. Ist es neuer
//! als die laufende Version, zeigt das Fenster einen Hinweis. Auf Klick wird
//! der Installer geladen, gegen den vom API gelieferten SHA-256 geprüft und
//! still gestartet; Holler beendet sich, der Installer tauscht die Dateien und
//! startet Holler wieder. Nur Releases, nie der main-Branch.

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const REPO: &str = "LupusMalusDeviant/holler";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug)]
pub struct UpdateInfo {
    pub version: String,
    pub setup_url: String,
    pub page_url: String,
    pub digest: Option<String>,
    pub size: u64,
}

#[derive(Clone, Debug)]
pub enum State {
    Off,
    Checking,
    UpToDate,
    Available(UpdateInfo),
    Downloading(UpdateInfo, u8),
    ReadyToQuit,
    Failed(String),
}

pub struct Updater {
    state: Mutex<State>,
}

impl Updater {
    pub fn new(enabled: bool) -> Arc<Self> {
        Arc::new(Updater { state: Mutex::new(if enabled { State::Checking } else { State::Off }) })
    }

    pub fn state(&self) -> State {
        self.state.lock().map(|s| s.clone()).unwrap_or(State::Off)
    }

    fn set(&self, s: State) {
        if let Ok(mut g) = self.state.lock() {
            *g = s;
        }
    }

    /// Prüfung im Hintergrund, mit kurzer Verzögerung, damit der Start nicht wartet.
    pub fn spawn_check(self: &Arc<Self>) {
        if matches!(self.state(), State::Off) {
            return;
        }
        let me = self.clone();
        std::thread::Builder::new()
            .name("holler-update".into())
            .spawn(move || {
                std::thread::sleep(Duration::from_secs(2));
                me.set(State::Checking);
                match check() {
                    Ok(Some(info)) => {
                        eprintln!("Update: Version {} verfuegbar ({})", info.version, info.setup_url);
                        me.set(State::Available(info));
                    }
                    Ok(None) => {
                        eprintln!("Update: {VERSION} ist aktuell");
                        me.set(State::UpToDate);
                    }
                    Err(e) => {
                        eprintln!("Update-Pruefung: {e}");
                        me.set(State::Failed(e));
                    }
                }
            })
            .ok();
    }

    /// Installer laden, prüfen, still starten. Danach ReadyToQuit: das Fenster beendet Holler.
    pub fn spawn_install(self: &Arc<Self>, info: UpdateInfo) {
        let me = self.clone();
        std::thread::Builder::new()
            .name("holler-install".into())
            .spawn(move || match download_and_launch(&me, &info) {
                Ok(()) => me.set(State::ReadyToQuit),
                Err(e) => me.set(State::Failed(e)),
            })
            .ok();
    }
}

/// Läuft Holler aus dem Installationsordner? Sonst ist es die portable Exe.
pub fn is_installed() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let Some(local) = std::env::var_os("LOCALAPPDATA") else { return false };
    let root = std::path::PathBuf::from(local).join("Programs").join("Holler");
    let a = exe.to_string_lossy().to_lowercase();
    let b = root.to_string_lossy().to_lowercase();
    a.starts_with(&b)
}

fn agent() -> String {
    format!("holler/{VERSION} (+https://github.com/{REPO})")
}

fn check() -> Result<Option<UpdateInfo>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = ureq::get(&url)
        .set("User-Agent", &agent())
        .set("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(404, _) => "Noch kein Release veröffentlicht".to_string(),
            other => format!("GitHub nicht erreichbar: {other}"),
        })?;
    let v: serde_json::Value = resp.into_json().map_err(|e| format!("Antwort unlesbar: {e}"))?;
    let tag = v["tag_name"].as_str().unwrap_or("").trim().trim_start_matches('v').to_string();
    if parse_version(&tag) <= parse_version(VERSION) {
        return Ok(None);
    }
    let page_url = v["html_url"].as_str().unwrap_or("").to_string();
    let asset = v["assets"]
        .as_array()
        .and_then(|a| {
            a.iter().find(|x| {
                let n = x["name"].as_str().unwrap_or("");
                n.starts_with("Holler-Setup") && n.ends_with(".exe")
            })
        })
        .ok_or_else(|| format!("Release {tag} hat keinen Installer"))?;
    Ok(Some(UpdateInfo {
        version: tag,
        setup_url: asset["browser_download_url"].as_str().unwrap_or("").to_string(),
        page_url,
        digest: asset["digest"].as_str().map(|d| d.trim_start_matches("sha256:").to_lowercase()),
        size: asset["size"].as_u64().unwrap_or(0),
    }))
}

fn parse_version(s: &str) -> (u64, u64, u64) {
    let mut it = s.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty()).map(|p| p.parse::<u64>().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(0))
}

fn download_and_launch(up: &Arc<Updater>, info: &UpdateInfo) -> Result<(), String> {
    if info.setup_url.is_empty() {
        return Err("Keine Download-Adresse".into());
    }
    let path = std::env::temp_dir().join(format!("Holler-Setup-{}.exe", info.version));
    let resp = ureq::get(&info.setup_url)
        .set("User-Agent", &agent())
        .timeout(Duration::from_secs(120))
        .call()
        .map_err(|e| format!("Download fehlgeschlagen: {e}"))?;
    let total = resp
        .header("Content-Length")
        .and_then(|l| l.parse::<u64>().ok())
        .unwrap_or(info.size)
        .max(1);
    let mut reader = resp.into_reader();
    let mut file = std::fs::File::create(&path).map_err(|e| format!("Temporäre Datei: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut done: u64 = 0;
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("Download abgebrochen: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| format!("Schreiben: {e}"))?;
        hasher.update(&buf[..n]);
        done += n as u64;
        up.set(State::Downloading(info.clone(), ((done * 100) / total).min(99) as u8));
    }
    drop(file);
    if let Some(expected) = &info.digest {
        let got = format!("{:x}", hasher.finalize());
        if &got != expected {
            let _ = std::fs::remove_file(&path);
            return Err("Prüfsumme des Installers stimmt nicht, Download verworfen".into());
        }
    }
    std::process::Command::new(&path)
        .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS"])
        .spawn()
        .map_err(|e| format!("Installer starten: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_version;

    #[test]
    fn versionen_vergleichen() {
        assert!(parse_version("v0.2.0") > parse_version("0.1.0"));
        assert!(parse_version("0.10.0") > parse_version("0.9.9"));
        assert!(parse_version("1.0.0") > parse_version("0.99.99"));
        assert_eq!(parse_version("v0.2.0"), parse_version("0.2.0"));
        assert!(parse_version("0.2.0") <= parse_version("0.2.0"));
        assert_eq!(parse_version("kaputt"), (0, 0, 0));
    }
}
