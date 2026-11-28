; Inno Setup 6 script for the DentalClarity Windows installer (docs/DECISIONS.md E26).
; Build steps are in deploy/windows/README.md. Paths are relative to this file.

#define AppVersion "0.1.0"
#define Port "8443"

[Setup]
AppId={{6C1E7A52-3B7F-4D7E-9A51-2F3C8D0B9E41}
AppName=DentalClarity
AppVersion={#AppVersion}
AppPublisher=DentalClarity contributors
AppPublisherURL=https://github.com/arminkhaleghirod/DentalClarity
DefaultDirName={autopf}\DentalClarity
DisableProgramGroupPage=yes
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile=..\..\LICENSE
OutputDir=..\..\target\installer
OutputBaseFilename=DentalClarity-{#AppVersion}-setup
UninstallDisplayIcon={app}\dentalclarity.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Files]
Source: "..\..\target\release\dentalclarity.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\knowledge\*"; DestDir: "{app}\knowledge"; Flags: ignoreversion recursesubdirs
Source: "..\..\fixtures\synthetic\*"; DestDir: "{app}\fixtures\synthetic"; Flags: ignoreversion recursesubdirs
Source: "..\..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\NOTICE"; DestDir: "{app}"; Flags: ignoreversion
Source: "README.md"; DestDir: "{app}"; DestName: "WINDOWS-README.md"; Flags: ignoreversion
; The settings file is written once and kept on upgrade and uninstall.
Source: "dentalclarity.env.example"; DestDir: "{commonappdata}\DentalClarity"; DestName: "dentalclarity.env"; Flags: onlyifdoesntexist uninsneveruninstall

[Dirs]
Name: "{commonappdata}\DentalClarity"; Flags: uninsneveruninstall
Name: "{commonappdata}\DentalClarity\data"; Flags: uninsneveruninstall
Name: "{commonappdata}\DentalClarity\logs"; Flags: uninsneveruninstall

[Run]
; Register the service first, so its virtual account exists for the ACL.
Filename: "{app}\dentalclarity.exe"; Parameters: "service install"; Flags: runhidden waituntilterminated; StatusMsg: "Registering the DentalClarity service..."
; Only SYSTEM, Administrators and the service account may read the data,
; because DPAPI machine scope lets any local process unwrap the secrets.
Filename: "{sys}\icacls.exe"; Parameters: """{commonappdata}\DentalClarity"" /inheritance:r /grant:r *S-1-5-18:(OI)(CI)F *S-1-5-32-544:(OI)(CI)F ""NT SERVICE\DentalClarity:(OI)(CI)M"""; Flags: runhidden waituntilterminated; StatusMsg: "Restricting access to the data folder..."
; HTTPS from the local subnet only.
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall add rule name=""DentalClarity HTTPS"" dir=in action=allow protocol=TCP localport={#Port} remoteip=localsubnet program=""{app}\dentalclarity.exe"""; Flags: runhidden waituntilterminated; StatusMsg: "Adding the firewall rule..."
; The settings need the practice's address before the first start.
Filename: "{win}\notepad.exe"; Parameters: """{commonappdata}\DentalClarity\dentalclarity.env"""; Description: "Edit the settings file now (start the service afterwards)"; Flags: postinstall nowait skipifsilent

[UninstallRun]
Filename: "{app}\dentalclarity.exe"; Parameters: "service uninstall"; Flags: runhidden waituntilterminated; RunOnceId: "RemoveService"
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""DentalClarity HTTPS"""; Flags: runhidden waituntilterminated; RunOnceId: "RemoveFirewallRule"

[Messages]
FinishedLabel=DentalClarity is installed as a Windows service but not started. Edit %ProgramData%\DentalClarity\dentalclarity.env (at least DC_PUBLIC_ORIGIN), then run: sc.exe start DentalClarity
