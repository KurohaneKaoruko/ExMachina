; EXMACHINA 桌面端 Windows 安装包(Inno Setup 6)
; 用户自选安装目录;安装后数据与应用同目录(data/),卸载可保留数据
; 前置:CI 已在 bundle/ 目录组装好全部文件

[Setup]
AppName=EXMACHINA
AppVersion=1.0.0
AppPublisher=EXMACHINA
DefaultDirName={autopf}\EXMACHINA
DefaultGroupName=EXMACHINA
UninstallDisplayIcon={app}\exmachina-desktop.exe
OutputBaseFilename=exmachina-desktop-windows-x64-setup
OutputDir=.
Compression=lzma2/max
SolidCompression=yes
; 免管理员:安装到用户可选目录(默认 %LOCALAPPDATA%\Programs\EXMACHINA)
PrivilegesRequired=lowest
SetupIconFile=icon.ico
WizardStyle=modern

[Tasks]
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "附加任务:"

[Files]
Source: "bundle\*"; DestDir: "{app}"; Flags: recursesubdirs createallsubdirs

[Icons]
Name: "{group}\EXMACHINA"; Filename: "{app}\exmachina-desktop.exe"; IconFilename: "{app}\icon.ico"
Name: "{autodesktop}\EXMACHINA"; Filename: "{app}\exmachina-desktop.exe"; IconFilename: "{app}\icon.ico"; Tasks: desktopicon

[Run]
Filename: "{app}\exmachina-desktop.exe"; Description: "立即启动 EXMACHINA"; Flags: nowait postinstall skipifsilent
