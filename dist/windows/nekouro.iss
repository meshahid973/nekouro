; NekoUro for Windows — per-user installer (Inno Setup 6).
;
; Built by scripts/package-windows.ps1, which passes the version, the package
; architecture, and the staged portable directory:
;   ISCC.exe /DAppVersion=0.2.97 /DArch=x86_64 /DPackageDir=<stage> /DOutputDir=<out> nekouro.iss
;
; Installs into %LOCALAPPDATA%\Programs\NekoUro without elevation, like VS
; Code's user setup: the directory stays writable by its user, so the in-app
; updater (crates/update/src/windows.rs) can replace nekouro.exe in place. The
; staged directory already carries nekouro-update.json, which marks the install
; as update-managed. Re-running a newer installer upgrades in place; user data
; lives in %LOCALAPPDATA%\NekoUro and is never touched here.

#ifndef AppVersion
  #error AppVersion must be defined (/DAppVersion=x.y.z)
#endif
#ifndef Arch
  #error Arch must be defined (/DArch=x86_64 or /DArch=aarch64)
#endif
#ifndef PackageDir
  #error PackageDir must be defined (/DPackageDir=<staged package directory>)
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif

#if Arch == "aarch64"
  #define ArchAllowed "arm64"
#else
  #define ArchAllowed "x64compatible"
#endif

[Setup]
; Never change AppId: it identifies the installation across upgrades, and
; crates/update/src/windows.rs refreshes DisplayVersion under this key after
; in-app updates.
AppId={{D2402C03-F657-4910-8FC3-33F7334881B7}
AppName=NekoUro
AppVersion={#AppVersion}
AppVerName=NekoUro {#AppVersion}
AppPublisher=NekoUro
AppPublisherURL=https://github.com/meshahid973/nekouro
AppSupportURL=https://github.com/meshahid973/nekouro/issues
AppUpdatesURL=https://github.com/meshahid973/nekouro/releases
VersionInfoVersion={#AppVersion}
PrivilegesRequired=lowest
DefaultDirName={autopf}\NekoUro
DisableProgramGroupPage=yes
DisableDirPage=auto
DisableReadyPage=yes
ArchitecturesAllowed={#ArchAllowed}
ArchitecturesInstallIn64BitMode={#ArchAllowed}
MinVersion=10.0
OutputDir={#OutputDir}
OutputBaseFilename=nekouro-{#AppVersion}-windows-{#Arch}-setup
SetupIconFile=nekouro.ico
UninstallDisplayIcon={app}\nekouro.exe
UninstallDisplayName=NekoUro
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
; A running NekoUro is closed through the Restart Manager before its files are
; replaced; the updated app starts again from the finish page.
CloseApplications=yes
RestartApplications=no

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#PackageDir}\nekouro.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PackageDir}\nekouro-update.json"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PackageDir}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PackageDir}\THIRD_PARTY_NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PackageDir}\licenses\*"; DestDir: "{app}\licenses"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\NekoUro"; Filename: "{app}\nekouro.exe"
Name: "{autodesktop}\NekoUro"; Filename: "{app}\nekouro.exe"; Tasks: desktopicon

[Registry]
; nekouro:// conversation links — the scheme macOS registers in Info.plist and
; Linux in nekouro.desktop.
Root: HKCU; Subkey: "Software\Classes\nekouro"; ValueType: string; ValueName: ""; ValueData: "URL:NekoUro"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\nekouro"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\nekouro\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\nekouro.exe"",0"
Root: HKCU; Subkey: "Software\Classes\nekouro\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\nekouro.exe"" ""%1"""

[Run]
Filename: "{app}\nekouro.exe"; Description: "{cm:LaunchProgram,NekoUro}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; Leftovers of in-app updates (crates/update/src/windows.rs).
Type: files; Name: "{app}\nekouro.exe.old"
Type: files; Name: "{app}\.nekouro-update-incoming.exe"
Type: filesandordirs; Name: "{app}\.nekouro-update-*"
