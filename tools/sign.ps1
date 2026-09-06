# Holler signieren mit einem selbstsignierten Code-Signing-Zertifikat.
#
# Einmalig (erzeugt Zertifikat im Speicher des angemeldeten Benutzers und exportiert es):
#   powershell -ExecutionPolicy Bypass -File tools\sign.ps1 -Init
# Bei jedem Build (signiert dist\holler.exe mit Zeitstempel):
#   powershell -ExecutionPolicy Bypass -File tools\sign.ps1
#
# Damit Windows dem Zertifikat vertraut, auf JEDEM Rechner einmal als Administrator:
#   certutil -addstore Root dist\LupusMalusDeviant.cer
#   certutil -addstore TrustedPublisher dist\LupusMalusDeviant.cer
# Danach zeigt Windows "Lupus Malus Deviant" als Herausgeber und keinen Warnhinweis.
#
# Die Datei dist\LupusMalusDeviant.pfx enthaelt den privaten Schluessel: NICHT weitergeben,
# nur die .cer wird auf den anderen Rechner kopiert.

param(
    [switch]$Init,
    [string]$Exe = "dist\holler.exe",
    [string]$Subject = "CN=Lupus Malus Deviant",
    [string]$Timestamp = "http://timestamp.digicert.com"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location $root
New-Item -ItemType Directory -Force dist | Out-Null

function Find-SignTool {
    $kits = "C:\Program Files (x86)\Windows Kits\10\bin"
    if (Test-Path $kits) {
        $c = Get-ChildItem $kits -Directory | Sort-Object Name -Descending |
             ForEach-Object { Join-Path $_.FullName "x64\signtool.exe" } | Where-Object { Test-Path $_ } | Select-Object -First 1
        if ($c) { return $c }
    }
    $cmd = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    throw "signtool.exe nicht gefunden. Windows SDK installieren (Visual Studio Installer -> Einzelne Komponenten -> Windows SDK)."
}

$cert = Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert | Where-Object { $_.Subject -eq $Subject } |
        Sort-Object NotAfter -Descending | Select-Object -First 1

if ($Init -or -not $cert) {
    if ($cert) { Write-Host "Zertifikat existiert bereits: $($cert.Thumbprint)" }
    else {
        Write-Host "Erzeuge selbstsigniertes Code-Signing-Zertifikat $Subject (gueltig 10 Jahre)"
        $cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject $Subject `
            -CertStoreLocation Cert:\CurrentUser\My -KeyUsage DigitalSignature `
            -KeyAlgorithm RSA -KeyLength 3072 -HashAlgorithm SHA256 `
            -NotAfter (Get-Date).AddYears(10) -FriendlyName "Holler Code Signing"
    }
    Export-Certificate -Cert $cert -FilePath "dist\LupusMalusDeviant.cer" -Force | Out-Null
    $pw = Read-Host -AsSecureString "Passwort fuer die private Sicherung (dist\LupusMalusDeviant.pfx)"
    Export-PfxCertificate -Cert $cert -FilePath "dist\LupusMalusDeviant.pfx" -Password $pw | Out-Null
    Write-Host "Exportiert: dist\LupusMalusDeviant.cer (oeffentlich, auf den anderen Rechner kopieren)"
    Write-Host "            dist\LupusMalusDeviant.pfx (privat, sicher aufbewahren)"
    if ($Init) { return }
}

if (-not (Test-Path $Exe)) { throw "Datei nicht gefunden: $Exe" }
$signtool = Find-SignTool
Write-Host "Signiere $Exe mit $($cert.Subject) ($($cert.Thumbprint))"
& $signtool sign /fd SHA256 /td SHA256 /tr $Timestamp /sha1 $cert.Thumbprint /d "Holler" /du "https://github.com" $Exe
if ($LASTEXITCODE -ne 0) {
    Write-Warning "Zeitstempel fehlgeschlagen, signiere ohne Zeitstempel (Signatur laeuft dann mit dem Zertifikat ab)."
    & $signtool sign /fd SHA256 /sha1 $cert.Thumbprint /d "Holler" $Exe
    if ($LASTEXITCODE -ne 0) { throw "Signieren fehlgeschlagen" }
}
& $signtool verify /pa /v $Exe | Select-String -Pattern "Issued to|Successfully|Error" | ForEach-Object { Write-Host $_ }
Write-Host "Hinweis: 'verify' meldet solange einen Fehler, bis das Zertifikat auf diesem Rechner als vertrauenswuerdig eingetragen ist (siehe Kopf der Datei)."
