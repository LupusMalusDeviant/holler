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
| **Raum / Passwort** | Freier Name, beliebiges Passwort. Beide zusammen ergeben Raum-ID und Schlüssel (Abschnitt 4.3). Wer beides kennt, ist drin. „Beitreten“ merkt sich beides; beim nächsten Start wird automatisch wieder beigetreten. Leer = LAN-Modus wie bisher, ohne Server. |
| **Vermittler** | Vorbelegt mit deinem Server, änderbar in der Konfiguration. Zeigt Zustand und Laufzeit zum Server. Fällt der Server aus, bleiben bestehende Direktverbindungen offen; Relay-Verbindungen brechen ab, das Fenster sagt es. |
| **Qualität für Ferne** | Auswahl mit einer Erklärzeile darunter, siehe Abschnitt 5. Gilt nur für das eigene Senden an Nicht-LAN-Peers. LAN-Peers bekommen immer PCM, automatisch. |
| **Teilnehmerzeile** | Punkt (grün = Audio kommt, gelb = verbindet, rot = weg), Name, Weg (LAN direkt / direkt IPv6 / direkt IPv4 / Relay), Laufzeit, Codec, Pegel, Lautstärkeregler 0–300 %, lokaler Stumm-Schalter (nur ich höre die Person nicht). Reihenfolge: LAN zuerst, dann nach Beitritt. |
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

## 9. Offene Fragen an dich

1. **Welcher Server** für den Hub: BurgCloud, Rabenhof oder Zirkuswagen? Und
   gibt es eine Domain, die darauf zeigen soll (z. B. `holler.<deine-domain>`)?
   Sonst nimmt der Client die IP.
2. **Raumpasswort-Weitergabe**: per Hand (Discord, WhatsApp) reicht? Der
   Einladungslink aus Phase 4 wäre bequemer, kostet aber einen URL-Handler in
   der Installation.
3. **Namen** der Teilnehmer: Rechnername wie heute, oder frei wählbar im Fenster?
   (Empfehlung: frei wählbar, Vorgabe Rechnername.)
4. **Beim Start automatisch dem letzten Raum beitreten** — ja? (Empfehlung: ja,
   mit sichtbarem „Verlassen“.)

Nach deinem Okay zu Entwurf und Antworten beginne ich mit Phase 1.
