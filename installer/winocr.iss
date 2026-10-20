; WinOCR v1.1.0 安装器脚本（Inno Setup 6）
; 前置：python tools/pack_release.py 组装 target/installer/staging/
; 编译：iscc installer/winocr.iss → installer 输出 WinOCR-Setup-v1.1.0.exe

#define MyAppName "WinOCR"
#define MyAppVersion "1.1.0"
#define MyAppPublisher "beyondaguan"
#define MyAppExeName "winocr.exe"

[Setup]
AppId={{7C1E9A52-6B0D-4F2E-9A31-A1B2C3D4E5F6}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
OutputDir=target
OutputBaseFilename=WinOCR-Setup-v{#MyAppVersion}
SetupIconFile=..\assets\tray.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=admin
MinVersion=10.0

[Tasks]
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "附加任务："
Name: "registerhost"; Description: "注册浏览器扩展接口（Chrome/Edge 划词采集）"; GroupDescription: "附加任务："; Flags: checkedonce

[Files]
Source: "..\target\installer\staging\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\installer\staging\winocr-host.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\installer\staging\winocr-argos.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\installer\staging\assets\*"; DestDir: "{app}\assets"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "..\target\installer\staging\browser-extension\*"; DestDir: "{app}\browser-extension"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\{#MyAppName} 卸载"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "立即启动 {#MyAppName}"; Flags: nowait postinstall skipifsilent
; 注册 Native Messaging Host（HKCU）+ 生成含钉死扩展 ID 白名单的 host manifest
Filename: "powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{app}\browser-extension\install_host.ps1"""; Tasks: registerhost; Flags: runhidden

[UninstallRun]
Filename: "powershell.exe"; Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{app}\browser-extension\uninstall_host.ps1"""; Flags: runhidden

[UninstallDelete]
; 说明：用户数据（生词本 winocr.db / 词典同步 / ECDICT）在 %APPDATA%\WinOCR，
; 卸载时特意保留——重装不丢生词本。如需彻底清除请手动删除该目录。
Type: files; Name: "{app}\*.log"
