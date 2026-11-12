<#
.SYNOPSIS
    Builds the Libertum installer: dist\Libertum-Setup-<version>.exe, one file that carries
    the program, install.ps1 and the README.

.DESCRIPTION
    Builds libertum.exe and the installer stub in release mode, then packs the three files
    into a copy of the stub (see crates/libertum-installer). Run from the repository root:

        powershell -ExecutionPolicy Bypass -File packaging\windows\build-installer.ps1

    An existing installer with the same name is never overwritten; pass -Suffix to make a
    new name (for example -Suffix r4).
#>
[CmdletBinding()]
param(
    [string]$Suffix = ''
)

$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..\..')
Set-Location $root
$cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
if (-not (Test-Path $cargo)) { $cargo = 'cargo' }

& $cargo build --release -p libertum-app -p libertum-installer
if ($LASTEXITCODE -ne 0) { throw 'The release build failed.' }

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches.Groups[1].Value
$name = "Libertum-Setup-$version"
if ($Suffix) { $name = "$name-$Suffix" }
New-Item -ItemType Directory -Force -Path dist | Out-Null
$out = Join-Path $root "dist\$name.exe"

# The installer is a windowed program: PowerShell only waits for it when told to.
$pack = Start-Process -FilePath (Join-Path $root 'target\release\libertum-installer.exe') -Wait -NoNewWindow -PassThru `
    -ArgumentList 'pack', '--app', '"target\release\libertum.exe"', '--script', '"packaging\windows\install.ps1"', `
        '--readme', '"README.md"', '--out', ('"' + $out + '"')
if ($pack.ExitCode -ne 0 -or -not (Test-Path $out)) { throw 'Packing the installer failed.' }
Get-Item $out | Select-Object Name, Length, LastWriteTime
