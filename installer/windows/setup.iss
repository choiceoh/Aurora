[Setup]
AppName=Aurora
AppVersion=0.1.0
AppPublisher=choiceoh
AppPublisherURL=https://github.com/choiceoh/Aurora
DefaultDirName={autopf}\Aurora
DefaultGroupName=Aurora
OutputDir=Output
OutputBaseFilename=AuroraSetup
Compression=lzma2
SolidCompression=yes
SetupIconFile=..\..\assets\aurora.ico
UninstallDisplayIcon={app}\aurora.exe
PrivilegesRequired=lowest
WizardStyle=modern
DisableProgramGroupPage=yes

[Languages]
Name: "korean"; MessagesFile: "compiler:Languages\Korean.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Files]
Source: "..\..\target\release\aurora.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
; 바탕화면 아이콘
Name: "{userdesktop}\Aurora"; Filename: "{app}\aurora.exe"; Comment: "AI 코딩 어시스턴트"
; 시작 메뉴
Name: "{userprograms}\Aurora\Aurora"; Filename: "{app}\aurora.exe"
Name: "{userprograms}\Aurora\Aurora 제거"; Filename: "{uninstallexe}"

[Run]
Filename: "{app}\aurora.exe"; Description: "Aurora 실행"; Flags: nowait postinstall skipifsilent
