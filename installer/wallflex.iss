; Wallflex installer (Inno Setup 6). Built by tools\package.ps1, which
; passes the version from Cargo.toml:
;
;   ISCC /DAppVersion=0.1.0 installer\wallflex.iss
;
; Installs for the current user only (no admin) into
; %LOCALAPPDATA%\Wallflex, the place SETUP.md has always suggested, so
; an existing copy there is upgraded in place. Settings stay in
; %APPDATA%\Wallflex and are only removed if the user says so.

#ifndef AppVersion
  #error Pass the version: ISCC /DAppVersion=X.Y.Z installer\wallflex.iss
#endif

#define AppName "Wallflex"
#define AppExe "wallflex.exe"
#define SettingsExe "wallflex-settings.exe"
#define RunKey "Software\Microsoft\Windows\CurrentVersion\Run"

[Setup]
; Never change the AppId: upgrades and the uninstaller find it by this.
AppId={{C36C0B17-8B45-49D2-914E-EB709E532B39}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Graham Burkett
AppPublisherURL=https://github.com/BonelessPig/Wallflex
AppSupportURL=https://github.com/BonelessPig/Wallflex/issues
AppUpdatesURL=https://github.com/BonelessPig/Wallflex/releases
AppCopyright=Graham Burkett. MIT OR Apache-2.0.
VersionInfoVersion={#AppVersion}
PrivilegesRequired=lowest
DefaultDirName={localappdata}\{#AppName}
DisableDirPage=auto
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
WizardStyle=modern
Compression=lzma2/ultra64
SolidCompression=yes
; Fallback when the running copy didn't close after --quit (see [Code]).
CloseApplications=yes
RestartApplications=no
UninstallDisplayName={#AppName}
UninstallDisplayIcon={app}\{#AppExe}
SetupIconFile=..\wallflex.ico
OutputDir=..\dist
OutputBaseFilename=wallflex-{#AppVersion}-setup

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Types]
Name: "full"; Description: "Wallflex and the settings app"
Name: "compact"; Description: "Wallflex only (edit config.toml by hand)"
Name: "custom"; Description: "Custom"; Flags: iscustom

[Components]
Name: "main"; Description: "Wallflex"; Types: full compact custom; \
  Flags: fixed
; Optional: a window for arranging the widgets and changing settings.
; It only runs while open, so it costs nothing otherwise.
Name: "settings"; \
  Description: "Settings app: drag widgets into place, change settings in a window"; \
  Types: full

[Tasks]
Name: "startup"; Description: "Start Wallflex when I sign in"

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; \
  Flags: ignoreversion
Source: "..\target\release\{#SettingsExe}"; DestDir: "{app}"; \
  Components: settings; Flags: ignoreversion
Source: "..\docs\SETUP.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\config.default.toml"; DestDir: "{app}"; \
  DestName: "config.example.toml"; Flags: ignoreversion
Source: "..\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autoprograms}\{#AppName} Settings"; \
  Filename: "{app}\{#SettingsExe}"; Components: settings

[InstallDelete]
; Reinstalling without the settings app removes an earlier copy of it.
Type: files; Name: "{app}\{#SettingsExe}"; Components: not settings
Type: files; Name: "{autoprograms}\{#AppName} Settings.lnk"; \
  Components: not settings

[Registry]
; The same value the tray menu's "Start with Windows" writes.
Root: HKCU; Subkey: "{#RunKey}"; ValueType: string; \
  ValueName: "{#AppName}"; ValueData: """{app}\{#AppExe}"""; \
  Tasks: startup; Flags: uninsdeletevalue

[Run]
Filename: "{app}\{#AppExe}"; Description: "Start Wallflex now"; \
  Flags: nowait postinstall skipifsilent

[Code]
const
  { How often to ask a running copy to quit, and how long between. }
  QuitTries = 50;
  QuitPollMs = 100;
  { Lets the process finish exiting after its window has gone. }
  QuitSettleMs = 500;

{ Asks a running Wallflex to quit and waits until it has. `--quit`
  exits with 0 while it finds a running copy, so it is repeated until
  it reports none. Returns False if the copy is still there. }
function QuitRunningCopy(const Exe: String): Boolean;
var
  Code: Integer;
  Tries: Integer;
begin
  Result := True;
  if not FileExists(Exe) then
  begin
    Exit;
  end;
  Tries := 0;
  while Exec(Exe, '--quit', '', SW_HIDE, ewWaitUntilTerminated, Code)
    and (Code = 0) do
  begin
    Tries := Tries + 1;
    if Tries > QuitTries then
    begin
      Result := False;
      Exit;
    end;
    Sleep(QuitPollMs);
  end;
  if Tries > 0 then
  begin
    Sleep(QuitSettleMs);
  end;
end;

{ Before files are copied: close the copy being upgraded. }
function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  if not QuitRunningCopy(ExpandConstant('{app}\{#AppExe}')) then
  begin
    Result := 'Wallflex is still running. Quit it from its tray icon, ' +
      'then try again.';
  end;
end;

{ Removes the sign-in entry if it starts this copy, including one the
  tray menu wrote rather than the installer. }
procedure RemoveStartup();
var
  Command: String;
begin
  if RegQueryStringValue(HKCU, '{#RunKey}', '{#AppName}', Command) and
    (Pos(Lowercase(ExpandConstant('{app}')), Lowercase(Command)) > 0) then
  begin
    RegDeleteValue(HKCU, '{#RunKey}', '{#AppName}');
  end;
end;

{ Offers to delete the settings, Discord sign-in and log. Kept by
  default, and always kept by a silent uninstall. }
procedure OfferToRemoveSettings();
var
  Settings: String;
begin
  Settings := ExpandConstant('{userappdata}\{#AppName}');
  if UninstallSilent() or not DirExists(Settings) then
  begin
    Exit;
  end;
  if MsgBox('Also delete your Wallflex settings, Discord sign-in and ' +
    'log?' + #13#10#13#10 + Settings, mbConfirmation,
    MB_YESNO or MB_DEFBUTTON2) = IDYES then
  begin
    if not DelTree(Settings, True, True, True) then
    begin
      MsgBox('Some settings files could not be deleted:' + #13#10 +
        Settings, mbError, MB_OK);
    end;
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
  begin
    { Best effort: if it is still running, its exe stays behind. }
    QuitRunningCopy(ExpandConstant('{app}\{#AppExe}'));
    RemoveStartup();
  end
  else if CurUninstallStep = usPostUninstall then
  begin
    OfferToRemoveSettings();
  end;
end;
