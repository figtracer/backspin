; The Windows installer: Backspin.vst3 and Backspin.clap into the system plugin folders
; (Common Files\VST3 and Common Files\CLAP). Built by .github/workflows/windows.yml with
; iscc /DVersion=<version> /DBundled=<target\bundled> /DOutput=<dir> scripts\backspin.iss.

[Setup]
AppId={{4A3D7067-EAF1-43CD-A17F-69F41525030B}
AppName=Backspin
AppVersion={#Version}
AppPublisher=figtracer
DefaultDirName={commoncf64}\VST3
DisableDirPage=yes
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=admin
OutputDir={#Output}
OutputBaseFilename=Backspin-{#Version}-windows-x64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayName=Backspin

[Files]
Source: "{#Bundled}\Backspin.vst3\*"; DestDir: "{commoncf64}\VST3\Backspin.vst3"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#Bundled}\Backspin.clap"; DestDir: "{commoncf64}\CLAP"; Flags: ignoreversion
