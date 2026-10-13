; Resource Monitor installer (Inno Setup 6.7+). Build: .\release.ps1 -Installer
;
; A bootstrapper: it holds no app build. It downloads the exe from the repo's
; main branch, then runs `resource-monitor-rs.exe --update`, which pulls the
; rest (sensor helpers in vendor\) by dist/manifest.json. Releases therefore
; never need a new installer; the app updates itself from main afterwards.
;
; Test against a local server: ISCC /DBase=http://127.0.0.1:8765/ /DTest setup.iss
; (/DTest = per-user install without admin; also set RM_UPDATE_BASE for --update.)

#define AppName "Resource Monitor"
#define AppExe "resource-monitor-rs.exe"
#define Repo "https://github.com/smtmRadu/resource-monitor"
#ifndef Base
  #define Base "https://raw.githubusercontent.com/smtmRadu/resource-monitor/main/"
#endif

[Setup]
; Also in src/update.rs (UNINSTALL_KEY): the app writes DisplayVersion there.
AppId={{6F1C2E7A-3B9D-4C5E-8A21-9D4E7B0C5F13}
AppName={#AppName}
AppVerName={#AppName}
AppPublisher=smtmRadu
AppPublisherURL={#Repo}
AppSupportURL={#Repo}/issues
AppUpdatesURL={#Repo}
DefaultDirName={autopf}\{#AppName}
DisableDirPage=no
DisableProgramGroupPage=yes
#ifdef Test
PrivilegesRequired=lowest
#else
PrivilegesRequired=admin
#endif
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir=..\dist
OutputBaseFilename=ResourceMonitor-Setup
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
WizardStyle=modern
Compression=lzma2/ultra
SolidCompression=yes
UsedUserAreasWarning=no

[Tasks]
Name: startup; Description: "Enable on startup"
Name: desktopicon; Description: "{cm:CreateDesktopIcon}"; Flags: unchecked

[Files]
Source: "{#Base}dist/{#AppExe}"; DestName: "{#AppExe}"; DestDir: "{app}"; ExternalSize: 1_000_000; Flags: external download ignoreversion
Source: "..\setup_task.ps1"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: postinstall nowait skipifsilent; Tasks: not startup

[UninstallRun]
Filename: "{sys}\WindowsPowerShell\v1.0\powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{app}\setup_task.ps1"" -Remove"; Flags: runhidden; RunOnceId: "RemoveTask"

[UninstallDelete]
; Downloaded by the app, so the uninstaller does not know them.
Type: filesandordirs; Name: "{app}\vendor"
Type: files; Name: "{app}\*.old"
Type: filesandordirs; Name: "{localappdata}\ResourceMonitor"

[Code]
function PowerShell(const Params: String): Integer;
begin
  if not Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
    '-NoProfile -ExecutionPolicy Bypass ' + Params, '', SW_HIDE, ewWaitUntilTerminated, Result) then
    Result := -1;
end;

{ Close a running monitor so its files can be replaced. A graceful close also
  ends its FPS capture session; force it after 8 s. }
procedure CloseApp;
var
  Wnd: HWND;
begin
  Wnd := FindWindowByClassName('ResourceMonitorRs');
  if Wnd <> 0 then begin
    PostMessage(Wnd, 16 { WM_CLOSE }, 0, 0);
    PowerShell('-Command "Wait-Process -Name resource-monitor-rs -Timeout 8 -ErrorAction SilentlyContinue; ' +
      'Stop-Process -Name resource-monitor-rs -Force -ErrorAction SilentlyContinue"');
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  CloseApp;
  Result := '';
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  Code: Integer;
begin
  if CurStep <> ssPostInstall then
    Exit;
  WizardForm.StatusLabel.Caption := 'Downloading the latest version from GitHub...';
  WizardForm.ProgressGauge.Style := npbstMarquee;
  if not Exec(ExpandConstant('{app}\{#AppExe}'), '--update', '', SW_SHOWNORMAL, ewWaitUntilTerminated, Code) or (Code <> 0) then
    SuppressibleMsgBox('Some components (FPS and exact CPU sensors) could not be downloaded. ' +
      'Resource Monitor still works without them. To retry, right-click its tray icon and click the version.',
      mbInformation, MB_OK, IDOK);
  if WizardIsTaskSelected('startup') then begin
    { Registers the elevated logon task and starts the monitor through it. }
    WizardForm.StatusLabel.Caption := 'Enabling start on logon...';
    PowerShell(ExpandConstant('-File "{app}\setup_task.ps1" -Exe "{app}\{#AppExe}"'));
  end else
    PowerShell(ExpandConstant('-File "{app}\setup_task.ps1" -Remove'));
  WizardForm.ProgressGauge.Style := npbstNormal;
end;

function InitializeUninstall: Boolean;
begin
  CloseApp;
  Result := True;
end;
