# Holler 1.0 „Räume“ — Entwurf für Internet, Mehrfach-Teilnehmer, IPv6

Stand 6. September 2026. Baut auf `docs/INTERFACE.md` (LAN-Version 0.2) auf.
Entscheidungen des Nutzers: direkte Verbindung per NAT-Hole-Punching, LAN-Paar
bleibt direkt, Codec wählbar mit Erklärung, bis 8 Personen pro Raum.

## 0. Was sich ändert, in einem Absatz

Aus „zwei Rechner finden sich im LAN“ wird „bis zu acht Leute treffen sich in
einem Raum“. Ein kleiner Vermittler-Server auf einem deiner Hetzner-Rechner
kennt die Räume, sagt jedem, unter welcher Adresse die anderen zu erreichen
sind, und hilft beim Durchstossen der Heimrouter. Audio läuft danach direkt
von Rechner zu Rechner; nur wenn ein Router das verhindert, reicht der Server
die Pakete weiter. Ihr zwei nebeneinander bleibt auf der LAN-Strecke mit rohem
PCM wie heute. Alles, was das Haus verlässt, ist verschlüsselt und läuft
wahlweise mit Opus statt PCM. IPv4 und IPv6 gleichwertig.

## 1. Ehrliche Grenzen

| Punkt | Aussage |
|---|---|
| Latenz Fern-Teilnehmer | Internet-Laufzeit (15–60 ms je Richtung) + Opus 15 ms + Jitter-Puffer. Realistisch 60–120 ms einfach. Deutlich besser als Discord, aber nicht LAN. |
| Latenz ihr zwei | Unverändert: LAN-Strecke, PCM, rund 25–35 ms Software. |
| Hole-Punching | Klappt bei den meisten Heimroutern. Bei CGNAT (viele Mobilfunk- und manche Glasfaseranschlüsse) oder symmetrischem NAT nicht; dann automatisch Relay über den Server, spürbar an 10–30 ms mehr. Das Fenster zeigt pro Person, welcher Weg aktiv ist. |
| Server | Ohne laufenden Vermittler gibt es nur LAN. Der Server ist klein (ein Rust-Binary im Container), braucht einen offenen UDP-Port und im schlimmsten Fall 8 × 768 kbit/s Durchsatz. |
| Sicherheit | Raumpasswort → Schlüssel, jedes Paket verschlüsselt und authentifiziert. Der Server sieht keinen Klartext. Keine Perfect Forward Secrecy: wer das Passwort und eine Aufzeichnung hat, kann sie entschlüsseln. Für Spielabende angemessen, bewusst einfach gehalten. |
| Mehr als 8 | Nicht vorgesehen. Ab da lohnt ein mischender Server, anderer Bauplan. |

## 2. Fenster

Die Karte „Verbindung“ wird zur Karte „Raum“; darunter erscheint eine
Teilnehmerliste. Mikrofon-Karte und Stumm-Knopf bleiben, „Partner hören“ geht
in der Liste auf (Lautstärke pro Person).

```
┌─ Holler ──────────────────────────────────────────────  ─ □ × ┐
│ Holler  LAN-Funk ohne Umwege                 ● im Raum · 3   │
│                                                              │
│ RAUM                                                         │
│  Raum  [ hunt-abend        ]  Passwort [ ••••••••  ] [Verlassen]  │
│  Vermittler  holler.lupusmalus.de   ·  verbunden  ·  12 ms   │
│  Qualität für Ferne  [ Opus 32 kbit/s ▾ ]                    │
│    Sprachqualität wie Discord, 15 ms, schont schwachen Upload. │
│                                                              │
│ TEILNEHMER                                                   │
│  ● BAD-WOLF       LAN direkt · 0,6 ms · PCM     ▮▮▮▮▮▯▯  ──●──  🔇 │
│  ● Mate-Kai       direkt IPv6 · 24 ms · Opus 32  ▮▮▯▯▯▯▯  ───●─  🔇 │
│  ● Jonas          Relay · 41 ms · Opus 32        ▯▯▯▯▯▯▯  ──●──  🔇 │
│                                                              │
│ MIKROFON                                                     │
│  [ Mikrofon (beyerdynamic LL Adapter)                    ▾ ] │
│  ▮▮▮▮▮▮▮▮▮▯▯▯▯▯▯▯▯▯▯▯   -22 dB  sendet                       │
│  Verstärkung ─────●────── 12 dB                              │
│  ☑ Rauschunterdrückung   ☑ Sprechsperre                      │
│                                                              │
│ [                LIVE  ·  F9 schaltet stumm                ] │
└──────────────────────────────────────────────────────────────┘
```

### Elemente

| Element | Verhalten |
|---|---|
| **Raum / Passwort** | Freier Name, beliebiges Passwort. Beide zusammen ergeben Raum-ID und Schlüssel (Abschnitt 4.3). Wer beides kennt, ist drin. „Beitreten“ merkt sich beides für den nächsten Klick, tritt aber nicht automatisch bei (Entscheidung des Nutzers). Leer = LAN-Modus wie bisher, ohne Server. |
| **Vermittler** | Vorbelegt mit deinem Hub (eigener Server, „Apps-Server“, Phase 2), änderbar in der Konfiguration. Zeigt Zustand und Laufzeit zum Server. Fällt der Server aus, bleiben bestehende Direktverbindungen offen; Relay-Verbindungen brechen ab, das Fenster sagt es. |
| **Qualität für Ferne** | Auswahl mit einer Erklärzeile darunter, siehe Abschnitt 5. Gilt nur für das eigene Senden an Nicht-LAN-Peers. LAN-Peers bekommen immer PCM, automatisch. |
| **Teilnehmerzeile** | Punkt (grün = Audio kommt, gelb = verbindet, rot = weg), Name, Weg (LAN direkt / direkt IPv6 / direkt IPv4 / Relay), Laufzeit, Codec, Pegel, Lautstärkeregler 0–300 %, lokaler Stumm-Schalter (nur ich höre die Person nicht). Reihenfolge: LAN zuerst, dann nach Beitritt. |
| **Mein Name** | Frei wählbar, Vorgabe Rechnername. Steht bei den anderen in der Liste. |
| **Kopfzeile** | Pille zeigt „im Raum · n“ oder „LAN · verbunden“ oder „suche“. |
| **Stumm (F9)** | Unverändert, gilt für alle Empfänger. |

Nicht im Fenster: Port, Rahmenlänge, Server-Wechsel, Codec-Feineinstellung.
Das sind Konfigurationswerte.

## 3. Bausteine

```
 Rechner A (LAN)  ◄──── PCM direkt, wie heute ────►  Rechner B (LAN)
        │  ▲                                              │  ▲
        │  │ Opus, verschlüsselt, direkt (Hole-Punching)  │  │
        ▼  │                                              ▼  │
   Mate-Kai (Internet, IPv6)                         Jonas (CGNAT)
        ▲                                                 ▲
        │            ┌────────────────────────┐           │
        └────────────┤  holler-hub (Hetzner)  ├───────────┘
                     │  Räume, Adressen,      │   Relay nur hier,
                     │  Relay-Rückfall        │   weil direkt scheitert
                     └────────────────────────┘
```

Zwei Programme im selben Repo:

- **`holler`** (Client, Windows): wie bisher plus Raum-Modus, Mehrfach-Mixer,
  Verschlüsselung, Opus, IPv6.
- **`holler-hub`** (Server, Linux, Docker): ein UDP-Port, IPv4 und IPv6.
  Raumverwaltung, Adress-Reflexion, Vermittlung, Relay. Kein Zustand auf
  Platte, kein Web, keine Konten. Sieht keinen Klartext.

Jede Verbindung zwischen zwei Teilnehmern ist unabhängig: eigener Weg, eigener
Codec, eigener Jitter-Puffer. Der Mixer summiert alle Empfangsströme auf die
Ausgabe (mit Soft-Limiter, damit vier laute Stimmen nicht übersteuern).

## 4. Protokoll v2

### 4.1 Paketkopf (Klartext, 20 Byte)

```
0   2   Magic "H2"
2   1   Typ
3   1   Flags (Bit 0 stumm, Bit 1 Relay-Weiterleitung erwünscht)
4   8   Absender-Kennung (zufällig pro Installation, in der Konfiguration)
12  4   Sequenznummer
16  4   Zeitstempel µs (für Laufzeit und Jitter)
20  …   Nutzlast (verschlüsselt, ausser bei Server-Paketen ohne Geheimnis)
```

Typen: `PROBE`, `PROBE-ACK` (Direktverbindung aufbauen/halten), `AUDIO`, `PONG`,
`HELLO` (LAN-Broadcast, bleibt), Server-seitig `JOIN`, `MEMBERS`, `RELAY`,
`KEEPALIVE`, `LEAVE`.

### 4.2 Audio-Nutzlast (verschlüsselt)

```
0   1   Codec: 0 = PCM 48 kHz i16, 1 = Opus
1   1   Rahmenlänge in ms (5 oder 10)
2   …   PCM-Samples oder ein Opus-Paket
```

Der Empfänger dekodiert, was ankommt; Codec-Wechsel des Senders brauchen keine
Aushandlung. LAN-Peers: PCM 5 ms wie heute. Ferne: Opus 10 ms (SILK-Modus
braucht mindestens 10 ms; 5 ms wären CELT-only und schlechter für Sprache).

### 4.3 Schlüssel und Verschlüsselung

- Raum-ID = BLAKE3(„holler-room“ ‖ Raumname). Der Server kennt nur die ID.
- Raumschlüssel = Argon2id(Passwort, Salz = Raum-ID), 256 bit, einmal beim
  Beitreten berechnet (rund 0,5 s, absichtlich langsam gegen Raten).
- Jede Nutzlast: ChaCha20-Poly1305, Nonce = Absender-Kennung (8 Byte) +
  Sequenz (4 Byte), Zusatzdaten = Paketkopf. Falsches Passwort → Paket wird
  verworfen, der Absender erscheint als „Passwort passt nicht“.
- HELLO im LAN trägt die Raum-ID, damit sich nur Rechner desselben Raums
  automatisch finden; ohne Raum verhält es sich wie heute.

### 4.4 Verbindungsaufbau mit dem Hub

1. `JOIN` (Raum-ID, Kennung, Name, lokale Adressen: LAN-IPv4, IPv6 global).
2. Hub antwortet `MEMBERS`: alle Mitglieder mit Kennung, Name, öffentlicher
   Adresse (wie der Hub sie sieht, IPv4 und/oder IPv6) und lokalen Adressen.
   Alle anderen Mitglieder bekommen ebenfalls ein aktualisiertes `MEMBERS`.
3. Für jedes neue Mitglied schicken beide Seiten gleichzeitig `PROBE` an alle
   Kandidaten: LAN-Adresse, IPv6 direkt, öffentliche IPv4. Das erste
   `PROBE-ACK` legt den Weg fest, Vorrang LAN › IPv6 › IPv4.
4. Kommt binnen 2 s kein `PROBE-ACK`, gehen die Pakete als `RELAY` über den
   Hub. Die Probes laufen alle 10 s weiter; klappt es später direkt, wechselt
   der Weg ohne Unterbrechung (kurze Doppelphase, Jitter-Puffer fängt es ab).
5. `KEEPALIVE` an den Hub alle 5 s hält die NAT-Bindung offen. Direktpeers
   halten sich über `PROBE` alle 5 s. 15 s Stille = Mitglied weg.

### 4.5 IPv6

Ein Socket, dual-stack (`[::]:port`, v4-mapped Adressen erlaubt). Alle
Adressen im Protokoll als 16 Byte + Port. Anzeige kürzt v6-Adressen. Die
Firewall-Regel auf beiden Rechnern gilt für beide Familien.

## 5. Codec-Wahl, so steht sie im Fenster

| Auswahl | Bandbreite je Stimme | Zusatzlatenz | Erklärzeile |
|---|---|---|---|
| **Rohes PCM** | 768 kbit/s | 0 ms | „Beste Qualität, kein Codec. Braucht sehr guten Upload; bei 4 Leuten empfängt jeder 2,3 Mbit/s.“ |
| **Opus 64** | 64 kbit/s | ≈15 ms | „Von PCM nicht zu unterscheiden. Für alle mit normalem DSL/Kabel.“ |
| **Opus 32** (Vorgabe) | 32 kbit/s | ≈15 ms | „Sprachqualität wie Discord, schont schwachen Upload.“ |
| **Opus 16** | 16 kbit/s | ≈15 ms | „Telefonqualität. Für Mobilfunk oder Hotel-WLAN.“ |

Die Wahl gilt für das eigene Senden. Jeder stellt ein, was sein Anschluss
hergibt; die anderen hören ihn entsprechend. LAN-Peers bekommen immer PCM.
Opus-Paketverlust wird vom Dekoder verschleiert (PLC), das ist hörbar besser
als bei PCM.

Implementierung mit `opus-pure` (reines Rust, bitgenau zur Referenz libopus,
kein C-Werkzeug nötig). Verifikation in Phase 3 mit Hörtest; Rückfall wären
die C-Bindings `opus-head-sys`.

## 6. Der Hub

- Rust, ein Binary `holler-hub`, Argumente `--port 4712 --max-rooms 64`.
- Docker-Image aus dem Repo (Dockerfile), Deploy auf einem Hetzner-Server über
  Whiskers, UDP-Port in UFW frei. Kein TLS nötig (Nutzlast ist Ende-zu-Ende
  verschlüsselt), kein Login, kein Speicher.
- Ressourcen: Speicher ein paar MB; Netz im schlimmsten Fall (alle 8 über Relay,
  alle PCM) 6 Mbit/s je Richtung. Mit Opus unter 0,5 Mbit/s.
- Schutz vor Missbrauch: Räume ohne Mitglieder verfallen nach 60 s, Pakete
  ohne gültigen Kopf werden verworfen, pro Absender-Adresse höchstens 1 MB/s
  Relay. Eine Raum-ID rät niemand.
- Öffentlich sichtbar ist nur: es gibt Räume mit n Mitgliedern und ihre
  Adressen. Kein Name, kein Audio.

## 7. Kommandozeile und Konfiguration, neu

```
--room <name> --room-password <text>   Raum beitreten (statt Fenster)
--hub <host:port>                      Vermittler (Vorgabe aus Konfiguration)
--codec pcm|opus64|opus32|opus16       Qualität für Ferne
--lan-only                             Kein Hub, nur Broadcast wie 0.2
```

`config.toml` bekommt `room`, `room_password`, `hub`, `codec`, `peer_id`
(erzeugt beim ersten Start) und pro Teilnehmer gespeicherte Lautstärken.
`peer` und `port` bleiben für den LAN-Modus.

## 8. Phasen, jede liefert ein Release

| Phase | Inhalt | Ergebnis | Aufwand |
|---|---|---|---|
| **1** | Protokoll v2, Verschlüsselung, IPv6, Mehrfach-Mixer, Teilnehmerliste, LAN-Broadcast mit Raum-ID | Bis 8 Leute im selben LAN, alles verschlüsselt. Ihr zwei merkt keinen Unterschied. | 1 Sitzung |
| **2** | `holler-hub`, JOIN/MEMBERS, Hole-Punching, Relay-Rückfall, Weg-Anzeige, Deploy auf Hetzner | Ferne kommen dazu, noch mit PCM (funktioniert bei gutem Anschluss). | 1–2 Sitzungen |
| **3** | Opus, Codec-Wahl mit Erklärung, PLC, Bandbreitenanzeige | Alltagstauglich für alle Anschlüsse. Release 1.0. | 1 Sitzung |
| **4** | Feinschliff: Wegwechsel ohne Aussetzer, Lautstärken merken, Raum-Einladungslink `holler://raum/…` | Komfort. | nach Bedarf |

Version 0.2 bleibt als „LAN-only“ voll funktionsfähig; Phase 1 ist
protokollinkompatibel zu 0.2, also beide Rechner gleichzeitig aktualisieren
(der Updater macht das ohnehin).

## 9. Entscheidungen (6. September 2026)

1. **Hub-Server**: ein eigener Server, voraussichtlich der „Apps-Server“. Adresse
   und Domain werden in Phase 2 festgelegt.
2. **Namen**: frei wählbar im Fenster, Vorgabe Rechnername.
3. **Beitreten**: jedes Mal per Klick, kein automatisches Beitreten. Raum und
   Passwort bleiben vorausgefüllt.
4. **Raumpasswort-Weitergabe**: per Hand; Einladungslink bleibt Phase 4.
5. Entwurf freigegeben. **Phase 1 umgesetzt als Version 0.3.0** (6. September
   2026): Protokoll v2, Räume mit Verschlüsselung, IPv4+IPv6, bis 8 Teilnehmer,
   Mixer mit Begrenzer, Teilnehmerliste. Nebenbei behoben: Taktdrift zwischen
   Aufnahme- und Wiedergabegerät liess den Jitter-Puffer in 0.2 auf 30–40 ms
   kriechen; jetzt regelt eine Füllstandsregelung pro Teilnehmer (Rahmen
   vorab doppelt einsetzen bzw. verwerfen), Zielpuffer bleibt bei 2–3 Rahmen.
   IPv6-Suche im LAN läuft noch nicht per Multicast, nur über feste Adressen.
6. **Phase 2 umgesetzt als Version 0.4.0** (6. September 2026): `holler-hub`
   läuft als Docker-Container im Host-Netz auf dem AppServer (Hetzner,
   168.119.111.164 / 2a01:4f8:c17:bb4d::1, UDP 4712, UFW offen). Client:
   JOIN/MEMBERS, Hole-Punching mit PROBE/PROBE-ACK und Vorrang LAN › IPv6 › IPv4,
   nach 2 s ohne Direktweg Relay über den Hub, Rückfall bei totem Weg, eigene
   öffentliche Adresse im Fenster. Gemessen von hier: Hub 23 ms Laufzeit, Relay
   mit 6 ms Jitter und 3–4 Rahmen Puffer. Hub-Domain `holler.app.lupusmalus.dev` seit 1.1.1 öffentlich bei Infomaniak
   gesetzt und Vorgabe im Client; alte Konfigurationen mit der IP werden beim
   Laden umgestellt.
7. **Phase 3 umgesetzt als Version 1.0.0** (6. September 2026): Opus über
   `opus-pure` (reines Rust, bitgenau zu libopus), 10-ms-Rahmen, VoIP-Profil,
   Qualität wählbar im Fenster mit Erklärzeile (PCM / 64 / 32 / 16 kbit/s),
   LAN-Peers immer PCM. Getrennte Sequenzräume für PCM und Opus (oberes Bit),
   damit die Nonce je Schlüssel eindeutig bleibt; Codec-Wechsel setzt den
   Sequenzzähler des Empfängers neu. Verlustverschleierung des Dekoders für
   bis zu zwei fehlende Rahmen. Puffer-Überlaufgrenze auf Ziel + 4 Rahmen
   erweitert, weil Jitter-Schübe im Internet sonst Pakete kosteten.
8. **Phase 4 umgesetzt als Version 1.1.0** (6. September 2026): Einladungslink
   `holler://join?room=…&pw=…&hub=…` (Knopf kopiert, Raumfeld nimmt Links an,
   Installer registriert das Schema, laufende Instanz übernimmt Links über
   eine Übergabedatei), Lautstärken je Teilnehmer-Kennung gemerkt, nur eine
   Instanz je Port (Mutex, zweiter Start holt das Fenster nach vorn).
   Wegwechsel direkt ↔ Relay war schon nahtlos (Sequenz läuft weiter).
   Testhygiene: `--mute --exit-after` für Testinstanzen, nachdem liegen
   gebliebene Testinstanzen das Mikrofon des Nutzers auf seinen Kopfhörer
   geschleift hatten.
9. **Version 1.3.0** (7. September 2026): Rauschunterdrückung von RNNoise auf
   DeepFilterNet3 (ll, tract, `deep_filter` v0.5.6 per Git-Tag, kstring auf
   2.0.2 gepinnt wegen rustc-Anforderung) umgestellt, nachdem der Nutzer Kauen,
   Tippen und Klappern hörte und der Haken „nichts änderte“. Messung mit
   TTS-Sprache plus Rauschen/Klicks/Kaubursts: Restrauschen in Pausen −77 dBFS
   (DF) gegen −56 dBFS (RNNoise) bei −40 dBFS Eingang, Sprache −0,6 dB, 0,4 ms je
   Rahmen, Vorausschau 0. RNNoise bleibt Sprachdetektor für die Sperre und
   Rückfall. Exe 10 → 62 MB. Version 1.2.0 (6.9.): Linux/macOS-Builds, cargo test
   in CI, Hub-Healthcheck, Whiskers-Alarmregel.


## 10. Desktop-Audio (Entwurf 8. September 2026, vom Nutzer entschieden)

**Ziel:** Wer will, sendet zusätzlich zur Stimme, was auf seinem Rechner läuft
(Spielsound, Musik). Opt-in je Sender, eigener Kanal je Person, stereo, bei den
Empfängern getrennt regelbar. Alle Plattformen.

**Entscheidungen:** eigener Kanal statt Beimischen; Qualität umschaltbar (Spiel
/ Musik); Windows, Linux und macOS.

### Quelle je Plattform

| Plattform | Quelle | Echo-Schutz |
|---|---|---|
| Windows | Prozess-Loopback (WASAPI, ab Windows 10 2004): „Alles ausser Holler“ (eigener Prozessbaum ausgeschlossen) oder „Nur Programm …“ (Liste laufender Programme mit Ton) | eingebaut, Hollers Ausgabe ist nie enthalten |
| Linux | ein Aufnahmegerät aus der Liste, üblicherweise „Monitor of …“ (PipeWire/Pulse) | Hinweis im Fenster: Holler auf ein anderes Ausgabegerät legen, sonst hört das Gegenüber sich selbst |
| macOS | ein Aufnahmegerät aus der Liste, üblicherweise ein virtuelles Gerät (BlackHole) mit Multi-Output | wie Linux |

Ohne passende Quelle bleibt die Option grau mit Erklärung.

### Protokoll

- Zweiter Audiostrom je Sender, Kopf-Flag Bit 3 = Desktop. Eigener
  Sequenzraum (Bit 30), damit die Nonce je Schlüssel eindeutig bleibt; Bit 31
  bleibt Opus.
- Nutzlast wie AUDIO, aber `[codec][rahmen ms][kbps][kanäle=2][daten]`.
  Stereo interleaved. LAN: PCM stereo 5 ms (1,5 Mbit/s). Fern: Opus stereo
  10 ms, Spiel 96 kbit/s (VoIP-Profil aus, Audio-Profil), Musik 160 kbit/s.
- Kein Desktop-Strom, wenn die Option aus ist (kein Stumm-Flag nötig).

### Empfänger

- Je Person ein zweiter Platz (Art „Desktop“) mit eigenem Jitter-Puffer,
  Driftregelung, Decoder (stereo), Lautstärke und „Ton aus“. Die
  Teilnehmerliste zeigt unter der Stimme eine zweite Zeile „Desktop“ mit
  Pegel, Regler und Schalter, sobald etwas ankommt.
- Der Mixer wird stereo: Stimmen mono auf beide Kanäle, Desktop links/rechts.
  Plätze: 8 Personen × 2 = 16.

### Sender, im Fenster: Karte „Desktop-Audio“

```
DESKTOP-AUDIO
 ☐ Desktop-Audio senden
 Quelle   [ Alles ausser Holler ▾ ]    (Windows: dazu „Nur Programm …“ mit Liste)
 Qualität [ Spiel · Opus 96 ▾ ]        Erklärzeile: Spiel = Stereo, 96 kbit/s, Sprachtauglich. Musik = 160 kbit/s, Musikprofil.
 ▮▮▮▮▮▯▯▯▯▯  Pegel
 Lautstärke ────●──── 100 %             (Sendepegel, unabhängig von der Stimme)
```

Keine Rauschunterdrückung und keine Sperre auf diesem Kanal. Stumm (F9)
betrifft nur die Stimme; Desktop-Audio hat seinen eigenen Haken.

### Grenzen, vorab gesagt

- Discord-Ton würde unter „Alles ausser Holler“ mitgesendet; wer Discord parallel
  nutzt, nimmt „Nur Programm: Spiel“.
- Linux/macOS ohne Prozess-Loopback: Echo-Schutz nur durch getrennte Ausgabegeräte.
- Bandbreite je Desktop-Sender: 96–160 kbit/s je Empfänger im Internet, im LAN
  1,5 Mbit/s je Empfänger.

### Schritte

1. Protokoll, Plätze, Stereo-Mixer, Empfängerliste, Sender über beliebiges
   Aufnahmegerät (alle Plattformen) → funktioniert sofort unter Linux/macOS mit
   Monitor/BlackHole.
2. Windows: Prozess-Loopback über die Bibliothek `wasapi` (Alles ausser Holler,
   Nur Programm) mit Programmliste aus den Audiositzungen.
3. Release 1.4.0.

**Umgesetzt am 8. September 2026 als 1.4.0**: alle drei Schritte in einem Zug,
Plätze 16, Stereo-Mixer, `src/desktop.rs` mit Prozess-Loopback (`wasapi`) und
Gerätepfad (cpal), Programmliste aus den Audiositzungen. Zwei-Instanzen-Test
LAN (PCM stereo) und Relay über den Hub (Opus stereo) bestanden.
