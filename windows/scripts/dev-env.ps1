<#
.SYNOPSIS
    Activates (and optionally installs) a 100% isolated Rust + .NET 8 SDK
    toolchain inside `<repo>\.devenv\` without modifying the Windows registry,
    user profile directories, or global PATH.

.EXAMPLE
    . .\windows\scripts\dev-env.ps1 -Install
    . .\windows\scripts\dev-env.ps1
#>
[CmdletBinding()]
param(
    [switch]$Install,
    [switch]$Quiet
)

$ErrorActionPreference = 'Stop'

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$DevEnvRoot = Join-Path $RepoRoot '.devenv'
$RustupHome = Join-Path $DevEnvRoot 'rustup'
$CargoHome = Join-Path $DevEnvRoot 'cargo'
$DotnetRoot = Join-Path $DevEnvRoot 'dotnet'
$DotnetCliHome = Join-Path $DevEnvRoot 'dotnet-home'
$NugetPackages = Join-Path $DevEnvRoot 'nuget'
$LlvmMingwDir = Join-Path $DevEnvRoot 'llvm-mingw'
$LlvmMingwBin = Join-Path $LlvmMingwDir 'bin'
$TmpDir = Join-Path $DevEnvRoot 'tmp'

$env:RUSTUP_HOME = $RustupHome
$env:CARGO_HOME = $CargoHome
$env:DOTNET_ROOT = $DotnetRoot
$env:DOTNET_CLI_HOME = $DotnetCliHome
$env:NUGET_PACKAGES = $NugetPackages
$env:DOTNET_SKIP_FIRST_TIME_EXPERIENCE = '1'
$env:DOTNET_CLI_TELEMETRY_OPTOUT = '1'
$env:DOTNET_MULTILEVEL_LOOKUP = '0'

$CargoBin = Join-Path $CargoHome 'bin'
$PathEntries = @($CargoBin, $DotnetRoot)
if (Test-Path $LlvmMingwBin) {
    $PathEntries += $LlvmMingwBin
}
$ExistingParts = ($env:PATH -split ';') | Where-Object { $_ -and ($_ -notin $PathEntries) }
$env:PATH = (($PathEntries + $ExistingParts) -join ';')

$CargoExe = Join-Path $CargoBin 'cargo.exe'
$RustcExe = Join-Path $CargoBin 'rustc.exe'
$DotnetExe = Join-Path $DotnetRoot 'dotnet.exe'

function Test-Dotnet8Sdk {
    if (-not (Test-Path $DotnetExe)) { return $false }
    $sdks = & $DotnetExe --list-sdks 2>$null
    return [bool]($sdks | Where-Object { $_ -match '^8\.' })
}

if ($Install) {
    foreach ($dir in @($DevEnvRoot, $RustupHome, $CargoHome, $DotnetRoot, $DotnetCliHome, $NugetPackages, $TmpDir)) {
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
    }

    if (-not (Test-Path $RustcExe)) {
        if (-not $Quiet) { Write-Host '==> Installing isolated Rust toolchain (x86_64-pc-windows-gnullvm) into .devenv\...' }
        $rustupInit = Join-Path $TmpDir 'rustup-init.exe'
        Invoke-WebRequest -Uri 'https://win.rustup.rs/x86_64' -OutFile $rustupInit
        & $rustupInit --no-modify-path -y --default-host x86_64-pc-windows-gnullvm --default-toolchain stable --profile minimal
        if ($LASTEXITCODE -ne 0) {
            throw "rustup-init.exe failed with exit code $LASTEXITCODE"
        }
    }

    if (-not (Test-Path (Join-Path $LlvmMingwBin 'dlltool.exe'))) {
        if (-not $Quiet) { Write-Host '==> Installing portable llvm-mingw (x86_64) into .devenv\llvm-mingw...' }
        $release = Invoke-RestMethod -Uri 'https://api.github.com/repos/mstorsjo/llvm-mingw/releases/latest'
        $asset = $release.assets | Where-Object { $_.name -like '*ucrt-x86_64.zip' } | Select-Object -First 1
        if (-not $asset) {
            throw 'Unable to locate portable llvm-mingw ucrt-x86_64.zip release asset'
        }
        $mingwZip = Join-Path $TmpDir $asset.name
        Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $mingwZip
        $extractDir = Join-Path $TmpDir 'llvm-mingw-extract'
        if (Test-Path $extractDir) { Remove-Item -Recurse -Force $extractDir }
        Expand-Archive -Path $mingwZip -DestinationPath $extractDir -Force
        $inner = Get-ChildItem $extractDir -Directory | Select-Object -First 1
        if (-not $inner) { throw 'Unexpected llvm-mingw archive layout' }
        if (Test-Path $LlvmMingwDir) { Remove-Item -Recurse -Force $LlvmMingwDir }
        Move-Item -Path $inner.FullName -Destination $LlvmMingwDir
        Remove-Item -Recurse -Force $extractDir
        Remove-Item -Force $mingwZip
    }

    if (-not (Test-Dotnet8Sdk)) {
        if (-not $Quiet) { Write-Host '==> Installing isolated .NET 8 SDK into .devenv\dotnet...' }
        $dotnetInstall = Join-Path $TmpDir 'dotnet-install.ps1'
        Invoke-WebRequest -Uri 'https://dot.net/v1/dotnet-install.ps1' -OutFile $dotnetInstall
        & $dotnetInstall -Channel '8.0' -InstallDir $DotnetRoot -NoPath
        if ($LASTEXITCODE -ne 0) {
            throw "dotnet-install.ps1 failed with exit code $LASTEXITCODE"
        }
    }
}

if (-not (Test-Path $CargoExe) -or -not (Test-Path $RustcExe)) {
    throw "Isolated Rust toolchain not found in $CargoBin. Run: . .\windows\scripts\dev-env.ps1 -Install"
}
if (-not (Test-Path (Join-Path $LlvmMingwBin 'dlltool.exe'))) {
    throw "Portable llvm-mingw not found in $LlvmMingwBin. Run: . .\windows\scripts\dev-env.ps1 -Install"
}
if (($env:PATH -split ';') -notcontains $LlvmMingwBin) {
    $env:PATH = "$LlvmMingwBin;$env:PATH"
}
if (-not (Test-Dotnet8Sdk)) {
    throw "Isolated .NET 8 SDK not found in $DotnetRoot. Run: . .\windows\scripts\dev-env.ps1 -Install"
}

if (-not $Quiet) {
    Write-Host "==> Isolated dev-env active ($DevEnvRoot)"
    Write-Host "    rustc : $(& $RustcExe --version)"
    Write-Host "    cargo : $(& $CargoExe --version)"
    Write-Host "    dotnet: $(& $DotnetExe --version)"
}
