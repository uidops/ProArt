; Inno Setup script for ProArt.  Build with:  iscc /DAppVersion=0.2.1 packaging\windows\proart.iss
#ifndef AppVersion
  #define AppVersion "0.2.1"
#endif
[Setup]
AppId={{6B1F4E2A-5C3D-4E8B-9A07-PROART000001}
AppName=ProArt
AppVersion={#AppVersion}
AppPublisher=ProArt contributors
DefaultDirName={autopf}\ProArt
DefaultGroupName=ProArt
OutputDir=..\..\dist
OutputBaseFilename=ProArt-Setup-{#AppVersion}
SetupIconFile=..\..\assets\icons\ProArt.ico
UninstallDisplayIcon={app}\ProArt.exe
Compression=lzma2
SolidCompression=yes
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=lowest

[Files]
Source: "..\..\dist\ProArt.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\dist\bin\proart.exe"; DestDir: "{app}\bin"; Flags: ignoreversion

[Icons]
Name: "{group}\ProArt"; Filename: "{app}\ProArt.exe"
Name: "{autodesktop}\ProArt"; Filename: "{app}\ProArt.exe"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "Create a desktop icon"; Flags: unchecked
Name: "startup"; Description: "Start ProArt in the tray when I sign in"; Flags: unchecked

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "ProArt"; ValueData: """{app}\ProArt.exe"" --menubar"; Tasks: startup; Flags: uninsdeletevalue

[Run]
Filename: "{app}\ProArt.exe"; Description: "Start ProArt"; Flags: nowait postinstall skipifsilent
