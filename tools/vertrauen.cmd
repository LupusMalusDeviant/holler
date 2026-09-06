@echo off
rem Traegt das Holler-Zertifikat (Lupus Malus Deviant) als vertrauenswuerdig ein.
rem Rechtsklick -> "Als Administrator ausfuehren". Erwartet LupusMalusDeviant.cer
rem im selben Ordner wie diese Datei oder im Ordner dist\ des Projekts.

setlocal
set CER=%~dp0LupusMalusDeviant.cer
if not exist "%CER%" set CER=%~dp0..\dist\LupusMalusDeviant.cer
if not exist "%CER%" (
    echo LupusMalusDeviant.cer nicht gefunden. Neben diese Datei legen.
    pause
    exit /b 1
)

net session >nul 2>&1
if errorlevel 1 (
    echo Bitte per Rechtsklick "Als Administrator ausfuehren" starten.
    pause
    exit /b 1
)

echo Zertifikat: %CER%
certutil -addstore Root "%CER%"
certutil -addstore TrustedPublisher "%CER%"
echo.
echo Fertig. Windows vertraut jetzt Programmen, die mit "Lupus Malus Deviant" signiert sind.
pause
