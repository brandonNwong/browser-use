<#
.SYNOPSIS
    Installs `build/windows/Browser Use/` into `%LOCALAPPDATA%\Programs\Browser Use\`.
#>
[CmdletBinding()]
param(
    [switch]$AddToPath,
    [switch]$StartMenu,
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$SourceDir = Join-Path $RepoRoot 'build\windows\Browser Use'
$InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\Browser Use'

if (-not $SkipBuild -or -not (Test-Path (Join-Path $SourceDir 'Browser Use.exe'))) {
    & (Join-Path $PSScriptRoot 'build.ps1') -Config Release
}

Write-Host '==> Stopping any running Browser Use...'
Get-Process -Name 'Browser Use' -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 300

if (Test-Path $InstallDir) {
    Remove-Item -Recurse -Force $InstallDir
}
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
Copy-Item -Path (Join-Path $SourceDir '*') -Destination $InstallDir -Recurse -Force
Write-Host "==> Installed to $InstallDir"

if ($AddToPath) {
    $UserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ($null -eq $UserPath) { $UserPath = '' }
    $Parts = ($UserPath -split ';') | Where-Object { $_ }
    if ($Parts -notcontains $InstallDir) {
        $NewPath = (@($Parts + $InstallDir) -join ';')
        [Environment]::SetEnvironmentVariable('Path', $NewPath, 'User')
        Write-Host "==> Added $InstallDir to user PATH"
    }
}

if ($StartMenu) {
    $StartMenuDir = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
    $ShortcutPath = Join-Path $StartMenuDir 'Browser Use.lnk'
    $Shell = New-Object -ComObject WScript.Shell
    $Shortcut = $Shell.CreateShortcut($ShortcutPath)
    $Shortcut.TargetPath = Join-Path $InstallDir 'Browser Use.exe'
    $Shortcut.WorkingDirectory = $InstallDir
    $Shortcut.IconLocation = Join-Path $InstallDir 'Browser Use.exe,0'
    $Shortcut.Save()
    Write-Host "==> Created Start Menu shortcut: $ShortcutPath"
}
