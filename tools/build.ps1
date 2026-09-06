# Holler bauen, nach dist\ kopieren und signieren. Ein Aufruf, aus jedem Ordner:
#   powershell -ExecutionPolicy Bypass -File X:\CodingAdventures2\TeamVoideOverLan\tools\build.ps1
#
# Baut in target-gen3, damit eine gerade laufende holler.exe aus target\release
# nicht angefasst wird. -NoSign laesst die Signatur weg.

param(
    [switch]$NoSign,
    [string]$TargetDir = "target-gen3"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location $root

# Laeuft die alte Exe gerade, kann sie nicht ueberschrieben, aber umbenannt werden.
$relDir = Join-Path $TargetDir "release"
$exe = Join-Path $relDir "holler.exe"
Get-ChildItem $relDir -Filter "holler-old-*.exe" -ErrorAction SilentlyContinue | ForEach-Object {
    try { Remove-Item $_.FullName -Force -ErrorAction Stop } catch { }
}
if (Test-Path $exe) {
    try {
        $fs = [System.IO.File]::Open($exe, 'Open', 'ReadWrite', 'None'); $fs.Close()
    } catch {
        $old = Join-Path $relDir ("holler-old-" + (Get-Date -Format "yyyyMMdd-HHmmss") + ".exe")
        Rename-Item $exe $old
        Write-Host "== Laufende Exe umbenannt nach $old (wird beim naechsten Bau geloescht)"
    }
}

Write-Host "== Bauen ($TargetDir)"
cargo build --release --target-dir $TargetDir
if ($LASTEXITCODE -ne 0) { throw "cargo build fehlgeschlagen" }

New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item $exe "dist\holler.exe" -Force
Write-Host "== Kopiert nach dist\holler.exe"

if (-not $NoSign) {
    Write-Host "== Signieren"
    & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root "tools\sign.ps1") -Exe "dist\holler.exe"
    if ($LASTEXITCODE -ne 0) { throw "Signieren fehlgeschlagen" }
}

$sig = Get-AuthenticodeSignature "dist\holler.exe"
$v = (Get-Item "dist\holler.exe").VersionInfo
Write-Host ""
Write-Host ("Fertig: dist\holler.exe  Version {0}  Signatur: {1}  ({2})" -f $v.ProductVersion, $sig.Status, $sig.SignerCertificate.Subject)
if ($sig.Status -eq "UnknownError") {
    Write-Host "Signatur vorhanden, Zertifikat auf diesem Rechner noch nicht als vertrauenswuerdig eingetragen: tools\vertrauen.cmd als Administrator."
}
