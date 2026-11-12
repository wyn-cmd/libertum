<#
.SYNOPSIS
    Installs Libertum for the current user: the program, a Start Menu shortcut, and the file
    types it reads so Windows offers it in "Choose an app".

.DESCRIPTION
    No administrator rights and no installer framework are needed: everything is written under
    the user's own profile (HKCU and %LOCALAPPDATA%). The program is unsigned, so Windows
    SmartScreen may warn on first run; "More info" then "Run anyway" is the usual route.

    Run it from the folder that holds libertum.exe:

        powershell -ExecutionPolicy Bypass -File .\install.ps1

    Uninstall with the Libertum-Uninstall.ps1 that this script writes beside the program.
#>
[CmdletBinding()]
param(
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Libertum'),
    [switch]$NoDesktopShortcut
)

$ErrorActionPreference = 'Stop'
$source = Join-Path $PSScriptRoot 'libertum.exe'
if (-not (Test-Path $source)) {
    throw "libertum.exe must sit next to this script (looked in $PSScriptRoot)."
}

$exe = Join-Path $InstallDir 'libertum.exe'
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
# Installing over an earlier copy: Windows will not replace a program that is running.
$running = Get-Process -Name 'libertum' -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -eq $exe }
if ($running) {
    throw 'Libertum is running. Save your work, close it, then run this script again.'
}
Copy-Item -Force $source $exe
if (Test-Path (Join-Path $PSScriptRoot 'README.md')) {
    Copy-Item -Force (Join-Path $PSScriptRoot 'README.md') $InstallDir
}

# --- Start Menu and desktop shortcuts -------------------------------------------------------
$startMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
New-Item -ItemType Directory -Force -Path $startMenu | Out-Null
$shortcut = Join-Path $startMenu 'Libertum.lnk'
$shell = New-Object -ComObject WScript.Shell
$link = $shell.CreateShortcut($shortcut)
$link.TargetPath = $exe
$link.WorkingDirectory = $InstallDir
$link.Description = 'Libertum word processor'
$link.Save()
if (-not $NoDesktopShortcut) {
    $desktop = [Environment]::GetFolderPath('Desktop')
    $link = $shell.CreateShortcut((Join-Path $desktop 'Libertum.lnk'))
    $link.TargetPath = $exe
    $link.WorkingDirectory = $InstallDir
    $link.Save()
}

# --- file types ----------------------------------------------------------------------------
# One programmatic identifier per format, so Windows can offer Libertum in "Open with" and in
# Settings > Default apps without taking the association away from another program.
$formats = [ordered]@{
    'libertum' = @{ Ext = '.lbtd'; Name = 'Libertum document' }
    'odt'      = @{ Ext = '.odt';  Name = 'OpenDocument text' }
    'docx'     = @{ Ext = '.docx'; Name = 'Word document' }
    'rtf'      = @{ Ext = '.rtf';  Name = 'Rich Text Format document' }
    'markdown' = @{ Ext = '.md';   Name = 'Markdown document' }
}

foreach ($key in $formats.Keys) {
    $format = $formats[$key]
    $progId = "Libertum.$key"
    $classes = "HKCU:\Software\Classes\$progId"

    # Every key a value is written into has to exist first, including DefaultIcon: a program
    # identifier without one shows up in "Open with" with a blank icon.
    foreach ($sub in @('', '\DefaultIcon', '\shell', '\shell\open', '\shell\open\command')) {
        New-Item -Path ($classes + $sub) -Force | Out-Null
    }
    Set-ItemProperty -Path $classes -Name '(default)' -Value $format.Name
    Set-ItemProperty -Path $classes -Name 'FriendlyTypeName' -Value $format.Name
    Set-ItemProperty -Path "$classes\DefaultIcon" -Name '(default)' -Value "$exe,0"
    Set-ItemProperty -Path "$classes\shell\open\command" -Name '(default)' -Value "`"$exe`" `"%1`""

    # Offer Libertum for the extension without stealing it from another program.
    $ext = "HKCU:\Software\Classes\$($format.Ext)\OpenWithProgids"
    New-Item -Path $ext -Force | Out-Null
    Set-ItemProperty -Path $ext -Name $progId -Value ''
}

# Advertise the formats so Libertum appears in Settings > Apps > Default apps.
$capabilities = 'HKCU:\Software\Libertum\Capabilities'
New-Item -Path $capabilities -Force | Out-Null
New-Item -Path "$capabilities\FileAssociations" -Force | Out-Null
Set-ItemProperty -Path $capabilities -Name 'ApplicationName' -Value 'Libertum'
Set-ItemProperty -Path $capabilities -Name 'ApplicationDescription' -Value 'Word processor for long documents'
Set-ItemProperty -Path "$capabilities\FileAssociations" -Name '.lbtd' -Value 'Libertum.libertum'
Set-ItemProperty -Path "$capabilities\FileAssociations" -Name '.odt' -Value 'Libertum.odt'
Set-ItemProperty -Path "$capabilities\FileAssociations" -Name '.docx' -Value 'Libertum.docx'
Set-ItemProperty -Path "$capabilities\FileAssociations" -Name '.rtf' -Value 'Libertum.rtf'
Set-ItemProperty -Path "$capabilities\FileAssociations" -Name '.md' -Value 'Libertum.markdown'
New-Item -Path 'HKCU:\Software\RegisteredApplications' -Force | Out-Null
Set-ItemProperty -Path 'HKCU:\Software\RegisteredApplications' -Name 'Libertum' -Value 'Software\Libertum\Capabilities'

# --- uninstaller ---------------------------------------------------------------------------
$uninstall = @'
# Removes Libertum for the current user. Leaves documents alone.
$ErrorActionPreference = 'SilentlyContinue'
$installDir = Join-Path $env:LOCALAPPDATA 'Libertum'
$progIds = @('Libertum.libertum', 'Libertum.odt', 'Libertum.docx', 'Libertum.rtf', 'Libertum.markdown')
$extensions = @('.lbtd', '.odt', '.docx', '.rtf', '.md')
foreach ($progId in $progIds) {
    Remove-Item -Recurse -Force "HKCU:\Software\Classes\$progId"
}
foreach ($extension in $extensions) {
    foreach ($progId in $progIds) {
        Remove-ItemProperty -Path "HKCU:\Software\Classes\$extension\OpenWithProgids" -Name $progId
    }
}
Remove-ItemProperty -Path 'HKCU:\Software\RegisteredApplications' -Name 'Libertum'
Remove-Item -Recurse -Force 'HKCU:\Software\Libertum'
Remove-Item -Force (Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Libertum.lnk')
Remove-Item -Force (Join-Path ([Environment]::GetFolderPath('Desktop')) 'Libertum.lnk')
Remove-Item -Recurse -Force $installDir
Write-Host 'Libertum removed.'
'@
$uninstallPath = Join-Path $InstallDir 'Libertum-Uninstall.ps1'
Set-Content -Path $uninstallPath -Value $uninstall -Encoding UTF8

Write-Host ''
Write-Host "Libertum installed: $exe"
Write-Host "Start Menu: $shortcut"
Write-Host 'It is offered for .lbtd, .odt, .docx, .rtf and .md in Settings > Apps > Default apps.'
Write-Host "Uninstall: powershell -ExecutionPolicy Bypass -File `"$uninstallPath`""
