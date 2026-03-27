[Setup]
AppName=Aurora
AppVersion=0.1.0
AppPublisher=choiceoh
AppPublisherURL=https://github.com/choiceoh/Aurora
DefaultDirName={autopf}\Aurora
DefaultGroupName=Aurora
UninstallDisplayIcon={app}\aurora.exe
OutputDir=Output
OutputBaseFilename=AuroraSetup
Compression=lzma2
SolidCompression=yes
SetupIconFile=..\..\assets\aurora.ico
WizardStyle=modern
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Languages]
Name: "korean"; MessagesFile: "compiler:Languages\Korean.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "바탕화면에 바로가기 만들기"; GroupDescription: "추가 옵션:"; Flags: checked

[Files]
Source: "..\..\target\release\aurora.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Aurora"; Filename: "{app}\aurora.exe"
Name: "{group}\Aurora 제거"; Filename: "{uninstallexe}"
Name: "{autodesktop}\Aurora"; Filename: "{app}\aurora.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\aurora.exe"; Description: "Aurora 실행"; Flags: nowait postinstall skipifsilent
