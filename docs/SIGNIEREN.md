# Signieren: Wo das Zertifikat liegt und was damit zu tun ist

Angelegt am 6. September 2026 auf dem Rechner LUPUSMALUS.

## Wo ist das Zertifikat

| Was                              | Wo                                                        | Wofür |
|----------------------------------|-----------------------------------------------------------|-------|
| Das Zertifikat selbst (mit Schlüssel) | Windows-Zertifikatspeicher deines Benutzerkontos. Anschauen: `Win+R`, `certmgr.msc`, dann „Eigene Zertifikate“ → „Zertifikate“ → **Lupus Malus Deviant** | Signiert die Exe. Braucht kein Passwort, solange du an diesem Rechner angemeldet bist. |
| `dist\LupusMalusDeviant.cer`     | im Projektordner                                           | Öffentlicher Teil. Diese Datei auf den anderen Rechner kopieren, damit er dem Zertifikat vertraut. Darf jeder haben. |
| `dist\LupusMalusDeviant.pfx`     | im Projektordner                                           | Privater Teil mit dem Passwort, das du beim Anlegen vergeben hast. **Nicht weitergeben.** Nur nötig, wenn du auf einem neuen Rechner weiter signieren willst (dort per Doppelklick importieren). |

Fingerabdruck zur Kontrolle: `6FD35F81A08EFF9409E67355906FD1B793939A2D`

## Was du damit machst

### 1. Exe signieren (nach jedem Build, auf diesem Rechner)

```
powershell -ExecutionPolicy Bypass -File X:\CodingAdventures2\TeamVoideOverLan\tools\sign.ps1
```

Signiert `dist\holler.exe` mit Zeitstempel. Eine andere Datei: `-Exe <pfad>` anhängen.
Der Aufruf funktioniert aus jedem Ordner, der volle Pfad zum Skript ist entscheidend.

### 2. Vertrauen einrichten (einmalig, auf JEDEM Rechner, der die Exe nutzt)

Auch auf deinem eigenen. Entweder `tools\vertrauen.cmd` per Rechtsklick „Als Administrator ausführen“,
oder von Hand in einer Administrator-Eingabeaufforderung:

```
certutil -addstore Root X:\CodingAdventures2\TeamVoideOverLan\dist\LupusMalusDeviant.cer
certutil -addstore TrustedPublisher X:\CodingAdventures2\TeamVoideOverLan\dist\LupusMalusDeviant.cer
```

Auf dem Rechner deiner Partnerin: `LupusMalusDeviant.cer` und `tools\vertrauen.cmd` rüberkopieren
(beide in denselben Ordner), dann das `.cmd` als Administrator starten.

Danach zeigt Windows bei der Exe „Verifizierter Herausgeber: Lupus Malus Deviant“ und keinen
Hinweis „Unbekannter Herausgeber“ mehr.

### 3. Kontrolle

Rechtsklick auf `holler.exe` → Eigenschaften → Reiter „Digitale Signaturen“. Dort steht
Lupus Malus Deviant mit Zeitstempel. Solange Schritt 2 auf dem Rechner fehlt, meldet der
Reiter „Das Zertifikat ist nicht vertrauenswürdig“, die Signatur selbst ist trotzdem gültig.

## Wenn etwas schiefgeht

- **„signtool.exe nicht gefunden“**: Windows SDK fehlt. Visual Studio Installer → Ändern →
  Einzelne Komponenten → „Windows 11 SDK“.
- **„Zeitstempel fehlgeschlagen“**: kein Internet oder Zeitstempeldienst gestört. Das Skript
  signiert dann ohne Zeitstempel; die Signatur gilt dann bis 2036 statt unbegrenzt. Später
  einfach noch einmal signieren.
- **Neuer Rechner, weiter signieren**: `LupusMalusDeviant.pfx` per Doppelklick importieren
  (Speicherort „Aktueller Benutzer“, Passwort eingeben), danach funktioniert `sign.ps1` dort.
- **Zertifikat weg oder Passwort vergessen**: `sign.ps1 -Init` legt ein neues an. Dann muss
  Schritt 2 auf allen Rechnern wiederholt werden.

## Was das Zertifikat nicht kann

Es ist selbstsigniert. Fremde Rechner vertrauen ihm nicht, und SmartScreen kennt es nicht.
Für zwei Rechner zu Hause ist das egal. Soll die Exe an Fremde verteilt werden, braucht es
ein gekauftes Zertifikat, siehe README.
