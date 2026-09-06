; Holler-Installer (Inno Setup 6.3+). Installation pro Benutzer, ohne Adminrechte.
; Aufruf: ISCC /DAppVersion=0.2.0 /DExeDir=target\release installer\holler.iss
; Signieren: zusaetzlich /DSignSetup /Sholler="<wrapper.cmd> $f"

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef ExeDir
  #define ExeDir "target\release"
#endif

[Setup]
AppId={{A3F2C9D1-7B4E-4C2A-9E1D-5F6A7B8C9D01}
AppName=Holler
AppVersion={#AppVersion}
AppVerName=Holler {#AppVersion}
AppPublisher=Lupus Malus Deviant
AppPublisherURL=https://github.com/LupusMalusDeviant/holler
AppSupportURL=https://github.com/LupusMalusDeviant/holler/issues
AppUpdatesURL=https://github.com/LupusMalusDeviant/holler/releases
VersionInfoVersion={#AppVersion}
DefaultDirName={localappdata}\Programs\Holler
DisableDirPage=auto
DefaultGroupName=Holler
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=..\dist
OutputBaseFilename=Holler-Setup-{#AppVersion}
SetupIconFile=..\assets\holler.ico
UninstallDisplayIcon={app}\holler.exe
UninstallDisplayName=Holler
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
#ifdef SignSetup
SignTool=holler $f
SignedUninstaller=yes
#endif

[Languages]
Name: "de"; MessagesFile: "compiler:Languages\German.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Verknüpfung auf dem Desktop"; Flags: unchecked
Name: "autostart"; Description: "Holler beim Anmelden starten (im Tray)"; Flags: unchecked

[Files]
Source: "..\{#ExeDir}\holler.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\dist\LupusMalusDeviant.cer"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\tools\vertrauen.cmd"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Holler"; Filename: "{app}\holler.exe"
Name: "{group}\Holler deinstallieren"; Filename: "{uninstallexe}"
Name: "{autodesktop}\Holler"; Filename: "{app}\holler.exe"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Holler"; ValueData: """{app}\holler.exe"" --hidden"; Tasks: autostart; Flags: uninsdeletevalue

[Run]
; Laeuft auch bei /VERYSILENT (Updater), weil kein "unchecked" gesetzt ist.
Filename: "{app}\holler.exe"; Description: "Holler jetzt starten"; Flags: nowait postinstall
