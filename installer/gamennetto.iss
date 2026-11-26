; Gamennetto installer (Inno Setup 6). Built by .github/workflows/release.yml:
;   iscc /DAppVersion=0.2.0 /DSourceDir=..\target\release installer\gamennetto.iss
;
; Per-user install: no administrator rights needed, so `gamennetto update`
; can install new versions silently. The virtual monitor driver is separate
; (`gamennetto display install` asks for administrator rights itself).

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\target\release"
#endif

[Setup]
; Never change AppId: updates find the existing install through it.
AppId={{6C1F4E57-2B7A-4E0B-9D35-7A54C6E2A1F3}
AppName=Gamennetto
AppVersion={#AppVersion}
AppVerName=Gamennetto {#AppVersion}
AppPublisher=Wilder Trujillo
AppPublisherURL=https://github.com/wilder00/gamennetto
AppSupportURL=https://github.com/wilder00/gamennetto/issues
AppUpdatesURL=https://github.com/wilder00/gamennetto/releases
DefaultDirName={autopf}\Gamennetto
DefaultGroupName=Gamennetto
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile=..\LICENSE
OutputDir=..\target\installer
OutputBaseFilename=Gamennetto-{#AppVersion}-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ChangesEnvironment=yes
; Updates close a running Gamennetto instead of asking to restart Windows.
CloseApplications=force
RestartApplications=no
UninstallDisplayName=Gamennetto

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "spanish"; MessagesFile: "compiler:Languages\Spanish.isl"

[Tasks]
Name: "addtopath"; Description: "Add gamennetto to PATH (use it from any terminal)"

[Files]
Source: "{#SourceDir}\gamennetto.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\THIRD_PARTY.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
; Each opens a console window, where Gamennetto asks its questions.
; --ask: step-by-step questions (mirror or extend, size, quality, control...).
Name: "{group}\Share this screen"; Filename: "{app}\gamennetto.exe"; Parameters: "host --ask"; WorkingDir: "{app}"
Name: "{group}\View another PC"; Filename: "{app}\gamennetto.exe"; Parameters: "view --ask"; WorkingDir: "{app}"
Name: "{group}\Gamennetto terminal"; Filename: "{cmd}"; Parameters: "/k ""{app}\gamennetto.exe"" --help"; WorkingDir: "{%USERPROFILE}"
Name: "{group}\Uninstall Gamennetto"; Filename: "{uninstallexe}"

[UninstallRun]
; Removes the virtual monitor driver if it was installed (asks for
; administrator permission; does nothing if there is no driver).
Filename: "{app}\gamennetto.exe"; Parameters: "display uninstall"; Flags: runhidden waituntilterminated; RunOnceId: "RemoveDriver"

[Registry]
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Tasks: addtopath; Check: NeedsAddPath(ExpandConstant('{app}'))

[Code]
function NeedsAddPath(Dir: string): Boolean;
var
  Path: string;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Path) then
  begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Path) + ';') = 0;
end;

procedure RemoveFromPath(Dir: string);
var
  Path: string;
  P: Integer;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Path) then
    exit;
  P := Pos(';' + Uppercase(Dir), Uppercase(Path));
  if P > 0 then
  begin
    Delete(Path, P, Length(Dir) + 1);
    RegWriteExpandStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Path);
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath(ExpandConstant('{app}'));
end;
