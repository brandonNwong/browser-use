<#
.SYNOPSIS
    Builds `browser-use.exe`, `browser_use.dll`, and the native Windows WPF
    tray application (`Browser Use.exe`), assembling `build/windows/Browser Use/`.
#>
[CmdletBinding()]
param(
    [ValidateSet('Debug', 'Release')]
    [string]$Config = 'Release',
    [switch]$UseDevEnv
)

$ErrorActionPreference = 'Stop'

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$DevEnvScript = Join-Path $PSScriptRoot 'dev-env.ps1'

$NeedsDevEnv = $UseDevEnv -or (-not (Get-Command cargo -ErrorAction SilentlyContinue)) -or (-not (Get-Command dotnet -ErrorAction SilentlyContinue))
if ($NeedsDevEnv) {
    if (Test-Path (Join-Path $RepoRoot '.devenv\cargo\bin\cargo.exe')) {
        . $DevEnvScript -Quiet
    }
    elseif ($UseDevEnv) {
        . $DevEnvScript -Install -Quiet
    }
}

Push-Location $RepoRoot
try {
    $CargoArgs = @()
    if ($Config -eq 'Release') {
        $CargoArgs += '--release'
    }
    $CargoProfileDir = if ($Config -eq 'Release') { 'release' } else { 'debug' }

    Write-Host "==> Building browser-use.exe ($Config)..."
    & cargo build @CargoArgs --bin browser-use
    if ($LASTEXITCODE -ne 0) { throw "cargo build --bin browser-use failed ($LASTEXITCODE)" }

    Write-Host "==> Building browser_use.dll ($Config)..."
    & cargo rustc @CargoArgs --lib --crate-type cdylib
    if ($LASTEXITCODE -ne 0) { throw "cargo rustc --crate-type cdylib failed ($LASTEXITCODE)" }

    $CliExe = Join-Path $RepoRoot "target\$CargoProfileDir\browser-use.exe"
    $HubDll = Join-Path $RepoRoot "target\$CargoProfileDir\browser_use.dll"
    if (-not (Test-Path $CliExe)) { throw "Missing $CliExe" }
    if (-not (Test-Path $HubDll)) { throw "Missing $HubDll" }

    Write-Host "==> Building Windows WPF app ($Config)..."
    $Csproj = Join-Path $RepoRoot 'windows\BrowserUse\BrowserUse.csproj'
    & dotnet build $Csproj -c $Config
    if ($LASTEXITCODE -ne 0) { throw "dotnet build failed ($LASTEXITCODE)" }

    $WpfOutDir = Join-Path $RepoRoot "windows\BrowserUse\bin\$Config\net8.0-windows"
    if (-not (Test-Path (Join-Path $WpfOutDir 'Browser Use.exe'))) {
        throw "Missing WPF output in $WpfOutDir"
    }

    $StageDir = Join-Path $RepoRoot 'build\windows\Browser Use'
    if (Test-Path $StageDir) {
        Remove-Item -Recurse -Force $StageDir
    }
    New-Item -ItemType Directory -Force -Path $StageDir | Out-Null

    Write-Host "==> Assembling $StageDir..."
    Copy-Item -Path (Join-Path $WpfOutDir '*') -Destination $StageDir -Recurse -Force
    Copy-Item -Path $HubDll -Destination (Join-Path $StageDir 'browser_use.dll') -Force
    Copy-Item -Path $CliExe -Destination (Join-Path $StageDir 'browser-use.exe') -Force
    Copy-Item -Path (Join-Path $RepoRoot 'README.md') -Destination (Join-Path $StageDir 'README.md') -Force

    $ExtensionSrc = Join-Path $RepoRoot 'extension'
    if (Test-Path (Join-Path $ExtensionSrc 'manifest.json')) {
        $ExtensionDst = Join-Path $StageDir 'extension'
        Copy-Item -Path $ExtensionSrc -Destination $ExtensionDst -Recurse -Force

        # Also package an unsigned .xpi archive for Firefox Developer/Nightly or Mozilla AMO signing.
        $XpiPath = Join-Path $StageDir 'gecko-orange.xpi'
        $TmpZip = Join-Path $StageDir 'gecko-orange.zip'
        if (Test-Path $XpiPath) { Remove-Item -Force $XpiPath }
        if (Test-Path $TmpZip) { Remove-Item -Force $TmpZip }
        Compress-Archive -Path (Join-Path $ExtensionSrc '*') -DestinationPath $TmpZip -Force
        Move-Item -Path $TmpZip -Destination $XpiPath -Force
    }

    Write-Host "==> Done: $StageDir"
}
finally {
    Pop-Location
}
