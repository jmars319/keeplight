; Per-user installer for Keeplight. Needs no administrator rights.
; Built by scripts/package.ps1, which passes AppVersion and SourceDir.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\dist\staging"
#endif
#ifndef OutputDir
  #define OutputDir "..\dist"
#endif

[Setup]
AppId={{A192D59D-D878-4270-9A23-7E44F8EB9BAA}
AppName=Keeplight
AppVersion={#AppVersion}
AppPublisher=Keeplight contributors
AppPublisherURL=https://github.com/jmars319/keeplight
AppSupportURL=https://github.com/jmars319/keeplight/issues
DefaultDirName={autopf}\Keeplight
DefaultGroupName=Keeplight
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile={#SourceDir}\LICENSE.txt
OutputDir={#OutputDir}
OutputBaseFilename=Keeplight-{#AppVersion}-setup
SetupIconFile=..\assets\app.ico
UninstallDisplayIcon={app}\keeplight.exe
UninstallDisplayName=Keeplight
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Shortcuts:"; Flags: unchecked

[Files]
Source: "{#SourceDir}\keeplight.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\keeplight-cli.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\THIRD-PARTY-LICENSES.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
; An explicit IconFilename, so Windows refreshes the icon when the exe changes.
Name: "{autoprograms}\Keeplight"; Filename: "{app}\keeplight.exe"; IconFilename: "{app}\keeplight.exe"
Name: "{autodesktop}\Keeplight"; Filename: "{app}\keeplight.exe"; IconFilename: "{app}\keeplight.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\keeplight.exe"; Description: "Open Keeplight"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Stops a keeper that is still putting lights back, so its exe can be removed.
Filename: "{sys}\taskkill.exe"; Parameters: "/f /im keeplight.exe"; Flags: runhidden; RunOnceId: "StopKeeplight"
Filename: "{sys}\taskkill.exe"; Parameters: "/f /im keeplight-cli.exe"; Flags: runhidden; RunOnceId: "StopKeeplightCli"

[Code]
// The app's "Put kept lights back when you sign in" switch writes a per-user
// startup entry. Uninstalling removes it, so nothing tries to start a missing
// exe at the next sign-in. The setup file and log in %LOCALAPPDATA%\Keeplight
// are left, so a reinstall keeps what you chose.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RegDeleteValue(HKEY_CURRENT_USER, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Keeplight');
end;
