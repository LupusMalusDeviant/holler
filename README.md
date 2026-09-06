<p align="center"><img src="assets/holler-256.png" width="128" alt="Holler"></p>

# Holler

LAN-Funk ohne Umwege: Sprachverbindung zwischen zwei Rechnern im selben Netz,
ohne Server, ohne Codec, ohne Browser. Rohes PCM per UDP, ein kleines Fenster, eine Exe.
Gedacht für zwei Leute, die nebeneinander mit Headsets spielen und sich
trotzdem hören wollen.

Der Entwurf mit Latenzbudget, Fensterbeschreibung und Protokoll steht in
[docs/INTERFACE.md](docs/INTERFACE.md).

## Download und Installation

Fertige Version unter **[Releases](https://github.com/LupusMalusDeviant/holler/releases/latest)**.

- **Installer (empfohlen)**: `Holler-Setup-x.y.z.exe`. Installiert pro Benutzer nach
  `%LOCALAPPDATA%\Programs\Holler`, ohne Adminrechte, mit Startmenü-Eintrag, optional
  Desktop-Verknüpfung und Autostart im Tray. Deinstallation über Windows-Apps.
- **Portabel**: `holler.exe` aus dem Zip irgendwohin kopieren und starten.

**Updates**: Holler fragt beim Start die GitHub-Releases ab. Gibt es eine neuere
Version, erscheint oben im Fenster ein blauer Hinweis. Ein Klick auf „Jetzt
aktualisieren“ lädt den Installer, prüft seine Prüfsumme gegen die Angabe von
GitHub, startet ihn still und Holler neu. Die portable Exe bekommt stattdessen
einen Link zur Download-Seite. Abschalten: `--no-update-check` oder
`update_check = false` in der Konfiguration. Es zählen nur veröffentlichte
Releases, nie der Entwicklungsstand auf `main`.

Wer den Hinweis „Unbekannter Herausgeber“ loswerden will, startet im
Installationsordner `vertrauen.cmd` als Administrator (liegt neben der `.cer`).
SmartScreen meldet sich bei Downloads einmal: „Weitere Informationen“, „Trotzdem
ausführen“.

Neue Versionen entstehen automatisch: Version in `Cargo.toml` erhöhen, committen,
Tag `v0.2.0` pushen. GitHub baut, signiert Exe und Installer und veröffentlicht
(siehe `.github/workflows/release.yml`).

## Bauen

```
powershell -ExecutionPolicy Bypass -File toolsuild.ps1
```

Baut, kopiert nach `dist\holler.exe` und signiert in einem Rutsch (siehe
docs/SIGNIEREN.md). Nur bauen ohne Signatur: `cargo build --release`, Ergebnis
`target/release/holler.exe` mit eingebettetem Icon. Die Datei auf
beide Rechner kopieren, mehr braucht es nicht.

Zu dritt mit einem Mate im Discord: beide im Discord bleiben, sich dort
gegenseitig stumm stellen. Man hört den Partner über Holler, den Mate über
Discord, und der Mate hört beide über Discord.

## Starten

Auf beiden Rechnern doppelklicken. Die beiden Instanzen finden sich per
Broadcast von selbst. Findet sich nichts nach ein paar Sekunden:

1. **Windows-Firewall.** Beim ersten Start fragt Windows, ob `holler` ins
   Netz darf. **Private Netzwerke** erlauben, auf beiden Rechnern. Wurde der
   Dialog weggeklickt: Windows-Sicherheit → Firewall → App durch Firewall
   zulassen → Holler, Haken bei Privat.
2. **IP eintragen.** Im Fenster unter „IP manuell“ die Adresse des anderen
   Rechners eingeben und „Verbinden“ klicken. Die IP steht drüben in
   `ipconfig` unter IPv4-Adresse. Eine Seite reicht, die andere übernimmt den
   Absender.
3. **Netzwerkprofil.** Ist das WLAN als „Öffentlich“ eingestuft, blockt
   Windows Broadcasts. Unter Einstellungen → Netzwerk → das WLAN → auf
   „Privat“ stellen.

Stumm schalten: Taste **F9** (auch wenn das Spiel im Vordergrund ist), der
Knopf im Fenster oder das Tray-Menü. Solange stumm, hat das Fenster einen
roten Rahmen und das Tray-Symbol ist rot.

## Tray

Schliessen (×) legt das Fenster ins Tray, das Programm läuft weiter. Windows
11 versteckt neue Tray-Symbole zunächst hinter dem Pfeil „^“ neben der Uhr;
per Ziehen auf die Leiste bleibt es dauerhaft sichtbar. Das
Symbol zeigt den Zustand: grün verbunden, gelb wartet auf Antwort, grau
sucht, rot stumm. Linksklick oder Doppelklick holt das Fenster zurück,
Rechtsklick öffnet das Menü mit Anzeigen, Stumm und Beenden. Mit `--hidden`
startet das Programm direkt im Tray.

## Optionen

```
holler [--peer <ip>] [--port 4711] [--in <name|index>] [--out <name|index>]
         [--frame 5|10] [--jitter auto|1..8] [--volume 0..300] [--name <text>]
         [--mic-gain 0..30] [--gate 0.05..0.95|off] [--denoise on|off] [--hotkey F9] [--headset-ms 40]
         [--hidden] [--headless] [--list-devices]
```

Alles, was im Fenster verändert wird (Geräte, Lautstärke, Puffer, Peer-IP),
landet beim Beenden in `%APPDATA%\holler\config.toml` und gilt beim
nächsten Start wieder.

`--list-devices` zeigt die Geräte mit Index. `--headless` läuft ohne Fenster
mit einer Statuszeile in der Konsole. `--peer 127.0.0.1` ist ein
Schleifentest auf dem eigenen Rechner: man hört sich selbst mit der
Software-Verzögerung.

## Lautstärke, Rauschunterdrückung, Sprechsperre

Alles sitzt auf der Senderseite, jeder stellt also sein eigenes Mikrofon ein:

- **Verstärkung** 0 bis 30 dB (Vorgabe 12 dB). Hochdrehen, bis der Pegel
  beim Sprechen um -20 dB liegt und der Balken nicht rot wird.
- **Rauschunterdrückung** (Vorgabe an) ist RNNoise, ein kleines neuronales
  Netz, das Grundrauschen, Lüfter und Tastatur aus der Stimme rechnet. Es
  kostet etwa 10 ms Verzögerung und wenig CPU. Bei Bedarf abschaltbar.
- **Sprechsperre** (Vorgabe an) sendet nur, wenn RNNoise Sprache erkennt.
  Öffnet sofort, hält 400 ms, blendet weich aus. Kein Schwellwert nötig; die
  Zeile unter dem Pegel zeigt, ob gerade gesendet wird. Feinjustage per
  `--gate 0.3` (empfindlicher) bis `--gate 0.8` (strenger), Vorgabe 0.5.
- **Lautstärke** unter „Partner hören“ 0 bis 300 % regelt zusätzlich auf der
  Empfängerseite.

## Signieren, damit Windows nicht warnt

Die Exe trägt in den Dateieigenschaften unter „Details“ Lupus Malus Deviant
als Firma und Copyright. Gegen „Unbekannter Herausgeber“ braucht es
zusätzlich eine Signatur. Für zwei Rechner zu Hause reicht ein selbstsigniertes
Zertifikat, das beide Rechner einmal als vertrauenswürdig eintragen:

```
powershell -ExecutionPolicy Bypass -File tools\sign.ps1 -Init   # einmalig: Zertifikat erzeugen
powershell -ExecutionPolicy Bypass -File tools\sign.ps1         # nach jedem Build: dist\holler.exe signieren
```

Danach auf jedem Rechner einmal als Administrator:

```
certutil -addstore Root dist\LupusMalusDeviant.cer
certutil -addstore TrustedPublisher dist\LupusMalusDeviant.cer
```

Die `.cer` ist öffentlich und darf auf den anderen Rechner. Die `.pfx` enthält den
privaten Schlüssel und bleibt, wo sie ist: wer sie hat, kann Programme signieren,
denen diese beiden Rechner vertrauen.

SmartScreen (der blaue „Der Computer wurde geschützt“-Bildschirm) meldet sich nur
bei Dateien mit Internet-Markierung, also nach Download per Browser oder Discord.
Per Netzwerkfreigabe oder USB kopierte Dateien haben sie nicht. Falls doch:
Rechtsklick, Eigenschaften, Haken „Zulassen“. Ein weltweit gültiges Zertifikat
(OV auf Hardware-Token, 200 bis 400 Euro im Jahr, oder Microsoft Trusted Signing
für rund 10 Dollar im Monat mit Ausweisprüfung) lohnt sich erst, wenn andere
Leute die Exe herunterladen sollen.

## Was die Anzeige bedeutet

| Wert       | Bedeutung                                                                   |
|------------|-----------------------------------------------------------------------------|
| Laufzeit   | Halbe Rundlaufzeit im Netz, gemessen über das eigene Protokoll               |
| Jitter     | 95. Perzentil der Schwankung der Paketankunft in den letzten 5 s             |
| Verlust    | Anteil fehlender Pakete in den letzten 5 s                                   |
| Software   | Aufnahmeperiode + Pufferfüllstand + Wiedergabeperiode, live                  |
| Headset-Funk | Fester Schätzwert für beide 2,4-GHz-Strecken zusammen, per `--headset-ms` anpassbar |
| Puffer     | Auto regelt den Jitter-Puffer selbst: sofort grösser bei Aussetzern, langsam kleiner in Sprechpausen |

## Grenzen

- Nur IPv4, nur zwei Teilnehmer, keine Verschlüsselung. Es ist ein Heimnetz.
- WASAPI Shared Mode mit 10-ms-Perioden. Kleinere Perioden über
  `IAudioClient3` sind der nächste Schritt, wenn die Messung zeigt, dass es
  sich lohnt.
- Der Hotkey funktioniert nur mit Fenster, nicht im `--headless`-Modus.
