; Inno Setup script for ratex (TeX engine & toolchain) on Windows
#define MyAppName "ratex"
#define MyAppVersion "0.2.0"
#define MyAppPublisher "Leo Liu"
#define MyAppURL "https://github.com/leoliu0/ratex"
#define MyAppExeName "texmk.exe"

[Setup]
AppId={{D82496E3-4E86-4F58-A81E-2B60773E92B1}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
AppUpdatesURL={#MyAppURL}
DefaultDirName={autopf}\ratex
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
LicenseFile=..\..\LICENSE-MIT
OutputBaseFilename=ratex-setup-v{#MyAppVersion}-windows-x64
OutputDir=..\..\dist
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
ArchitecturesInstallIn64BitMode=x64compatible
ArchitecturesAllowed=x64compatible
ChangesEnvironment=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "envPath"; Description: "Add ratex to environment PATH (recommended)"; GroupDescription: "System Integration:"
[Files]

; Core binary
Source: "..\..\dist\tex-suite-windows-x86_64\bin\ratex.exe"; DestDir: "{app}\bin"; Flags: ignoreversion

; Runtime assets
Source: "..\..\dist\tex-suite-windows-x86_64\share\tex-suite\*"; DestDir: "{app}\share\tex-suite"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "..\..\README.md"; DestDir: "{app}"; DestName: "README.txt"; Flags: ignoreversion
Source: "..\..\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion

[Registry]
; Add {app}\bin to user PATH
Root: HKCU; Subkey: "Environment"; \
    ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}\bin"; \
    Check: NeedsAddPath(ExpandConstant('{app}\bin')); Tasks: envPath

[Code]
function NeedsAddPath(Param: string): boolean;
var
  OrigPath: string;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', OrigPath)
  then begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Param + ';', ';' + OrigPath + ';') = 0;
end;
