# Holler — Interface-Entwurf

Zwei Rechner nebeneinander, zwei Funk-Headsets, mindestens ein Rechner im
WLAN. Jeder hört den anderen mit so wenig Verzögerung, wie diese Hardware
zulässt. Kein Server, kein Account, kein Codec, kein Browser: eine Exe mit
einem kleinen nativen Fenster.

Der Arbeitsname war `lanvoice`; seit Version 0.1 heisst das Programm **Holler**. Im Text unten steht noch der alte Name.

## 0. Ausgangslage (Stand 3. September 2026)

| Punkt                 | Antwort                                                  |
|-----------------------|----------------------------------------------------------|
| Headsets              | Beide 2,4-GHz-Funk, kein Kabelmodus                       |
| Netz                  | Mindestens ein Rechner per WLAN, kein Kabel möglich       |
| Stack                 | Rust, natives Fenster, eine Exe, keine Web-UI             |
| Bedienung             | Leitung immer offen, Stumm-Hotkey                         |
| Betriebssystem        | Windows 10/11 auf beiden Rechnern (Annahme)               |
| Spiel                 | Hunt: Showdown, nutzt dasselbe Headset gleichzeitig       |

## 1. Ehrliches Ziel und Latenzbudget

Das Gehör verschmilzt zwei Signale bis etwa 30 ms zu einem. Darüber wird ein
Echo daraus. Mit zwei Funkstrecken und WLAN ist die 30-ms-Marke **nicht
erreichbar**. Das Ziel ist deshalb:

1. **So kurz wie die Hardware zulässt** — Software-Anteil unter 25 ms.
2. **Konstant statt schwankend.** Ein Echo mit fester Verzögerung ist
   erträglich, eines mit wanderndem Abstand nicht.
3. **Messbar.** Das Fenster zeigt, wo die Zeit bleibt, damit die
   Hardware-Entscheidung (Kabel-Headset, Netzkabel) später mit Zahlen
   getroffen werden kann.

| Glied der Kette                         | Budget       | Beeinflussbar durch das Tool          |
|-----------------------------------------|--------------|---------------------------------------|
| Funk-Mikro Sprecher → PC                | 15–30 ms     | Nein                                  |
| Aufnahmepuffer (WASAPI Shared)          | 3–10 ms      | Ja, kleinste Periode des Treibers     |
| Rahmen bilden, UDP senden               | < 1 ms       | Ja, kein Codec                        |
| WLAN-Laufzeit                           | 2–8 ms       | Teils, WMM-Sprachpriorität (DSCP)     |
| Jitter-Puffer                           | 10–30 ms     | Ja, adaptiv, so klein wie es geht     |
| Wiedergabepuffer (WASAPI Shared)        | 3–10 ms      | Ja                                    |
| PC → Funk-Kopfhörer Hörer               | 15–30 ms     | Nein                                  |
| **Summe**                               | **50–100 ms**| Software-Anteil: 15–40 ms             |

Konsequenzen für das Interface:

- **Kein Codec.** Opus kostet 5–20 ms Algorithmus-Verzögerung. Rohes PCM
  16 bit mono 48 kHz sind 768 kbit/s, auch im WLAN unkritisch.
- **Shared Mode, nicht Exclusive.** Exclusive wäre schneller, blockiert aber
  das Headset für das Spiel. Shared Mode über `IAudioClient3` erlaubt
  Perioden bis ~2,7 ms, wenn der Treiber mitspielt.
- **Verlust ist egal, Jitter nicht.** Ein verlorener Rahmen = 5 ms Stille,
  unhörbar. Ein zu kleiner Jitter-Puffer = Knacken. Der Puffer regelt sich
  selbst (Abschnitt 4.4).
- **Broadcast kann im WLAN scheitern** (Client-Isolation im Router). Deshalb
  gibt es neben der automatischen Suche immer die manuelle Peer-IP.

Ausserhalb der Software, aber im Fenster als Hinweis:

- WLAN-Rechner auf **5 GHz** verbinden. 2,4-GHz-WLAN und 2,4-GHz-Headsets
  teilen sich denselben Funkbereich und stören sich gegenseitig.
- Headset-Dongle per USB-Verlängerung weg vom PC-Gehäuse und weg von der
  WLAN-Antenne.
- Falls das Headset einen „Low Latency“- oder „Gaming“-Modus hat: einschalten.

## 2. Fenster

Ein Fenster, keine Menüs, keine Tabs, keine Einstellungsdialoge. Beim Spielen
schaut man nur kurz rüber; alles Wichtige steht in der ersten Zeile.

```
┌─ lanvoice ────────────────────────────────────────────── ─ □ × ┐
│ ⚠ WLAN auf 2,4 GHz. Das ist der Funkbereich eurer Headsets.     │
│   Auf 5 GHz wechseln, wenn der Router es anbietet.              │
│                                                                 │
│  Gegenüber   ● verbunden   PC-Anna (192.168.178.42)             │
│              Laufzeit 6 ms · Jitter 9 ms · Verlust 0,4 %        │
│                                                                 │
│  Latenz      Software 27 ms  +  Headset-Funk ≈ 40 ms  ≈ 67 ms   │
│  Puffer      [ Auto ▾ ]   ▮▮▮▯▯▯▯▯   3 Rahmen (15 ms)           │
│                                                                 │
│  Mikrofon    [ Headset-Mikrofon (Arctis Nova 7)          ▾ ]    │
│              ▮▮▮▮▮▮▮▮▮▮▮▮▮▮▮▯▯▯▯▯▯▯▯▯▯▯▯▯▯▯                     │
│                                                                 │
│  Ausgabe     [ Kopfhörer (Arctis Nova 7)                 ▾ ]    │
│              ▮▮▮▮▮▮▮▮▯▯▯▯▯▯▯▯▯▯▯▯▯▯▯▯▯▯▯▯▯▯                     │
│                                                                 │
│  Partner-Lautstärke  ─────────●────────  80 %                   │
│                                                                 │
│  [   Stumm  (F9)   ]                                            │
└─────────────────────────────────────────────────────────────────┘
```

### Elemente und ihr Verhalten

| Element                   | Verhalten                                                                 |
|---------------------------|---------------------------------------------------------------------------|
| **Hinweiszeile** (gelb)   | Nur sichtbar, wenn es etwas zu sagen gibt: WLAN statt Kabel, 2,4-GHz-Band, Treiber erlaubt nur 10-ms-Perioden, zweiter Peer gesehen. Sonst weg. |
| **Gegenüber**             | `○ suche…` / `● verbunden` / `● keine Pakete seit 2 s`. Name und IP. Automatische Suche, darunter ein Eingabefeld „IP manuell“ mit Knopf „Verbinden“. |
| Laufzeit / Jitter / Verlust | Laufzeit = halbe Rundlaufzeit über das eigene Protokoll. Jitter = 95. Perzentil der Ankunftsabweichung der letzten 5 s. Verlust = fehlende Sequenznummern der letzten 5 s. |
| **Latenz**                | Software = Aufnahmeperiode (10 ms angenommen) + gemessener Pufferfüllstand + Wiedergabeperiode (10 ms angenommen). Headset-Funk = fester Schätzwert (konfigurierbar, Vorgabe 40 ms für beide Richtungen zusammen), damit die Summe ehrlich ist. |
| **Puffer**                | `Auto` (Vorgabe) oder feste Rahmenzahl 1–8. Der Balken zeigt den Füllstand live; er soll ruhig stehen, nicht zappeln. |
| **Mikrofon**-Auswahl      | Alle Aufnahmegeräte, vorbelegt mit dem Windows-Kommunikationsgerät. Wechsel wirkt sofort. |
| Pegel darunter            | Eigener Sendepegel. Rot bei Übersteuerung. Steht still, wenn stumm. |
| **Ausgabe**-Auswahl       | Alle Wiedergabegeräte, gleiche Vorbelegung. Wechsel sofort. |
| Pegel darunter            | Empfangspegel des Partners. Die sichtbare Bestätigung „da kommt was an“. |
| **Partner-Lautstärke**    | Nur die Stimme des Partners, nie das Spiel. 0–150 %, über 100 % für leise Mikros. |
| **Stumm**                 | Eigenes Mikrofon. Globaler Hotkey (Vorgabe F9), wirkt auch mit dem Spiel im Vordergrund. Fensterrahmen rot, solange stumm. Kein Push-to-Talk. |
| Schliessen (×)            | Beendet das Programm und speichert die Konfiguration. Tray-Symbol ist für eine spätere Version vorgesehen. |

Das Fenster zeichnet mit 30 Hz und nur, wenn es sichtbar ist. Es läuft in
einem eigenen Thread und berührt die Audio-Threads nicht (Abschnitt 5).

Nicht im Fenster: Port, Rahmenlänge, DSCP, Hotkey-Belegung. Das sind
Konfigurationswerte, keine Bedienung.

## 3. Kommandozeile und Konfiguration

Ohne Argumente startet das Fenster mit gespeicherten oder Standardwerten.
Argument schlägt Datei, Datei schlägt Vorgabe.

```
lanvoice [Optionen]

  --peer <ip>            Peer fest vorgeben statt Broadcast-Suche (im WLAN oft nötig)
  --port <n>             UDP-Port für Audio und Suche              (Vorgabe 4711)
  --in  <name|index>     Aufnahmegerät, Teilstring oder Index      (Vorgabe: Windows-Kommunikationsgerät)
  --out <name|index>     Wiedergabegerät, dito
  --frame <ms>           Rahmenlänge 5 oder 10                     (Vorgabe 5)
  --jitter auto|<n>      Jitter-Puffer in Rahmen, 1–8              (Vorgabe auto)
  --volume <prozent>     Partner-Lautstärke 0–150                  (Vorgabe 100)
  --name <text>          Anzeigename für den Partner               (Vorgabe: Rechnername)
  --hotkey <taste>       Stumm-Hotkey                              (Vorgabe F9)
  --headset-ms <n>       Schätzwert Headset-Funk für die Anzeige   (Vorgabe 40)
  --headless             Kein Fenster, Statuszeile in der Konsole
  --list-devices         Geräte mit Index ausgeben und beenden
  --version / --help
```

Konfigurationsdatei `%APPDATA%\lanvoice\config.toml`, wird beim Beenden mit
dem aktuellen Fensterzustand geschrieben:

```toml
port       = 4711
frame_ms   = 5
jitter     = "auto"               # oder Zahl 1–8
volume     = 80
name       = "PC-Ben"
hotkey     = "F9"
headset_ms = 40
in         = "Arctis Nova 7"      # Teilstring, nicht Index: Indizes wandern nach USB-Umstecken
out        = "Arctis Nova 7"
peer       = "192.168.178.42"     # weglassen = Broadcast-Suche
```

Exit-Codes: `0` normal, `2` Gerät nicht gefunden, `3` Port belegt,
`4` Treiber kann kein Shared-Mode-Audio in dem Format.

`--headless` schreibt alle 500 ms eine sich überschreibende Statuszeile:

```
● PC-Anna 192.168.178.42  rtt 12ms  jit 9ms  loss 0.4%  buf 3/15ms  sw 27ms  mic ▮▮▮▮▯▯  spk ▮▮▯▯▯▯  [F9 stumm]
```

## 4. Drahtprotokoll (UDP, symmetrisch)

Kein Server, kein Client. Beide Instanzen lauschen auf demselben Port und
senden an den Peer. Drei Pakettypen, unterschieden am ersten Byte. Alle
Mehrbyte-Werte little-endian.

### 4.1 Suche — `HELLO` (Typ `0x01`)

Alle 1 s per Broadcast an `255.255.255.255:<port>` **und**, falls `--peer`
gesetzt, zusätzlich per Unicast dorthin. Sobald Audio ankommt, alle 5 s als
Lebenszeichen.

```
Offset  Länge  Inhalt
0       1      0x01
1       1      Protokollversion (1)
2       4      Instanz-Kennung, zufällig, damit der eigene Broadcast ignoriert wird
6       n      Anzeigename, UTF-8, max. 32 Byte
```

Der erste fremde `HELLO` mit passender Version wird der Peer. Ein zweiter
fremder Rechner wird ignoriert und in der Hinweiszeile gezeigt. Eine
Aushandlung der Rahmenlänge gibt es nicht: der Empfänger nimmt jede
Paketlänge, wie sie kommt, jede Seite sendet mit ihrer eigenen Rahmenlänge.

### 4.2 Audio — `AUDIO` (Typ `0x02`)

Ein Rahmen pro Paket, kein Sammeln.

```
Offset  Länge  Inhalt
0       1      0x02
1       1      Flags: Bit 0 = Sender ist stumm (dann folgt kein PCM)
2       2      Sequenznummer, uint16, überlaufend
4       4      Sendezeitstempel, uint32, Mikrosekunden, überlaufend
8       480    PCM, 240 Samples int16 mono (bei 10 ms: 960 Byte)
```

488 Byte, 200 Pakete/s je Richtung bei 5 ms. Bei Stumm nur der 8-Byte-Kopf,
damit der Peer „stumm“ von „Verbindung weg“ unterscheiden kann. Der
Zeitstempel dient der Jitter-Messung (Abweichung zwischen Sende- und
Ankunftsabstand), nicht der Wiedergabesteuerung.

### 4.3 Laufzeit — `PONG` (Typ `0x03`)

Jedes 200. Audio-Paket (1×/s) spiegelt der Empfänger mit dem Zeitstempel des
Absenders zurück. Daraus ergibt sich die Laufzeitanzeige.

```
Offset  Länge  Inhalt
0       1      0x03
1       3      reserviert (0)
4       4      zurückgespiegelter Sendezeitstempel
```

### 4.4 Empfangslogik und adaptiver Jitter-Puffer

Der Puffer ist ein Ring aus `N` Rahmen. Die Wiedergabe beginnt, sobald
`ziel` Rahmen vorliegen. Regeln:

- **Rahmen fehlt** zum Wiedergabezeitpunkt: Stille einfügen, nicht warten.
  Fehlen zwei oder mehr in Folge, den letzten Rahmen mit 50 % Pegel
  wiederholen, dann Stille (billige Verlustverschleierung, kein Codec).
- **Rahmen zu spät** (Sequenz schon abgespielt): verwerfen, als Verlust zählen.
- **Zu viele Rahmen gestaut** (mehr als `ziel + 2`): ältesten verwerfen.
  Hält die Latenz konstant, statt sie wachsen zu lassen.
- **Auto-Modus:**
  - `ziel = ceil(p95_jitter_5s / rahmen_ms) + 1`, mindestens 2, höchstens 8.
  - **Wachsen sofort:** bei einem Unterlauf wird `ziel` um 1 erhöht.
  - **Schrumpfen langsam:** erst nach 30 s ohne Unterlauf und nur, während
    der Partner stumm ist oder sein Pegel unter der Hörschwelle liegt, damit
    das Wegwerfen eines Rahmens nicht hörbar wird.
- **Uhrendrift:** die beiden Soundkarten laufen nicht exakt gleich schnell.
  Läuft der Puffer über Minuten langsam voll oder leer, greift die Regel
  „zu viele gestaut“ bzw. der Unterlauf. Kein Resampling in Version 1.
- 2 s ohne Paket → Zustand „keine Pakete“, die Suche startet wieder.

### 4.5 Sprachpriorität im WLAN (DSCP)

UDP-Pakete werden mit DSCP `EF (46)` markiert. WMM-fähige WLAN-Router und
-Karten ordnen das der Sprachklasse `AC_VO` zu, die auf der Luft Vorrang vor
Spiel- und Browserverkehr hat. Windows ignoriert die Markierung aus
Anwendungen standardmässig; nötig ist einmalig der Registry-Wert
`HKLM\SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\DisableUserTOSSetting = 0`
oder eine QoS-Richtlinie. In Version 0.1 ist die Markierung **noch nicht umgesetzt**; die Option
`--dscp` ist reserviert.

Was das Protokoll bewusst **nicht** hat: Verschlüsselung, Authentifizierung,
Kompression, mehr als zwei Teilnehmer, Stereo. Es ist eure Wohnung.

## 5. Interne Schnittstellen (Rust)

Vier Bausteine, drei Threads, dazwischen nur wartefreie Ringpuffer.

```
 ┌──────────┐  Frame   ┌──────────┐   UDP    ┌──────────┐  Frame   ┌──────────┐
 │ Capture  │ ───────▶ │  Sender  │ ───────▶ │ Receiver │ ───────▶ │ Playback │
 └──────────┘          └──────────┘          └──────────┘          └──────────┘
  WASAPI-Thread          Netz-Thread          Netz-Thread          WASAPI-Thread

                        ┌──────────┐
                        │   UI     │  liest nur Atomics, 30 Hz, eigener Thread
                        └──────────┘
```

```rust
struct Frame { seq: u16, samples: [i16; 240] }          // 480 bei 10 ms

struct DeviceId { name: String, index: usize, is_default_comm: bool }

enum PeerState { Searching, Connected { name, ip, rtt_ms, jitter_ms, loss_pct },
                 Lost { since: Instant } }

trait Capture   { fn open(dev: &DeviceId, frame_ms: u8) -> Result<Self>;
                  fn set_muted(&self, on: bool);
                  fn level(&self) -> f32;                 // RMS 0..1, atomar
                  fn period_ms(&self) -> f32; }           // tatsächliche WASAPI-Periode

trait Playback  { fn open(dev: &DeviceId, frame_ms: u8) -> Result<Self>;
                  fn set_volume(&self, v: f32);           // 0..1.5
                  fn level(&self) -> f32;
                  fn period_ms(&self) -> f32; }

struct Sender   { fn start(port: u16, peer: Option<Ipv4Addr>, dscp: bool) -> Result<Self>;
                  fn peer_state(&self) -> PeerState; }

struct Receiver { fn start(port: u16, jitter: JitterMode) -> Result<Self>;
                  fn stats(&self) -> Stats; }             // rtt, jitter, loss, buffered, ziel

enum JitterMode { Auto, Fixed(u8) }

fn devices() -> (Vec<DeviceId>, Vec<DeviceId>)
fn software_latency_ms(cap: &impl Capture, rx: &Receiver, pb: &impl Playback) -> f32
    // cap.period_ms() + rx.stats().buffered * frame_ms + pb.period_ms()
```

Kandidaten für Crates (bei der Umsetzung prüfen, nicht jetzt festlegen):

| Aufgabe          | Crate                         | Anmerkung                                                        |
|------------------|-------------------------------|------------------------------------------------------------------|
| Fenster          | `eframe` / `egui` (glow)      | Immediate-Mode, 30 Hz, GPU-Last vernachlässigbar                 |
| Audio            | `wasapi` oder `windows` direkt| Shared Mode mit `IAudioClient3`-Perioden. `cpal` nutzt nur 10-ms-Perioden, reicht für die Anzeige, nicht fürs Ziel |
| Ringpuffer       | `rtrb`                        | SPSC, wartefrei, keine Allokation nach dem Start                 |
| Hotkey           | `global-hotkey`               | `RegisterHotKey`, wirkt auch mit dem Spiel im Vordergrund         |
| Tray             | `tray-icon`                   |                                                                  |
| Config           | `toml` + `serde`              |                                                                  |
| Netz             | `std::net::UdpSocket`         | Kein async nötig, ein blockierender Thread                       |

Regeln, die alle Module einhalten:

- **Audio-Threads allokieren nicht, sperren nicht, loggen nicht.** Nur
  Ringpuffer und Atomics. Thread-Priorität über `AvSetMmThreadCharacteristics("Pro Audio")`.
- Pegel werden im Audio-Thread als RMS berechnet und atomar abgelegt.
- Gerätewechsel = altes Modul schliessen, neues öffnen. Sender und Receiver
  merken davon nichts.
- Die UI liest nur. Jede Änderung (Stumm, Lautstärke, Puffer) ist ein
  atomarer Schreibzugriff, den der betroffene Thread beim nächsten Rahmen liest.

## 6. Bewusst weggelassen

| Weggelassen                   | Warum                                                                  |
|-------------------------------|------------------------------------------------------------------------|
| Push-to-Talk                  | Leitung offen, Stumm-Taste reicht. Ihr sitzt nebeneinander.            |
| Echo-Unterdrückung (AEC)      | Geschlossene Kopfhörer koppeln kaum zurück. Erst, wenn es tatsächlich pfeift. |
| Rauschsperre                  | Kostet die erste Silbe. Bei Bedarf später als Option.                  |
| Opus / Kompression            | Latenz. Bandbreite ist kein Problem.                                   |
| Resampling gegen Uhrendrift   | Puffer-Regeln fangen es ab. Erst, wenn es alle paar Minuten knackt.    |
| Mehr als zwei Teilnehmer      | Anderes Produkt.                                                       |
| Internet / NAT                | Dafür gibt es Discord.                                                 |
| Web-UI, Browser, Electron     | Ausdrücklich nicht gewünscht.                                          |

## 7. Offene Punkte und nächster Schritt

1. **Erst messen, dann bauen.** Schritt 1 ist ein `--headless`-Durchstich
   ohne Fenster: Capture → UDP → Playback mit fester Pufferzahl, dazu die
   Statuszeile. Ein Abend Hunt damit zeigt, ob 50–70 ms erträglich sind. Ist
   das Echo noch störend, entscheidet ihr über Kabel-Headsets **bevor** das
   Fenster gebaut wird.
2. **5 oder 10 ms Rahmen.** Im WLAN halbieren 10-ms-Rahmen die Paketrate und
   damit die Chance auf Kollisionen, kosten aber 5 ms. Messen, nicht raten.
3. **Welche Perioden erlauben eure Treiber?** `--list-devices` gibt die
   kleinste Shared-Mode-Periode je Gerät aus. Liegt sie bei 10 ms, kostet
   das 14 ms gegenüber dem Optimum.
4. **DSCP-Registry-Wert** braucht einmalig Adminrechte auf beiden Rechnern.
   Das Tool setzt ihn nicht selbst, es zeigt nur, ob er fehlt.
