<#
.SYNOPSIS
    Runs `test.ps1` and packages `build/windows/Browser Use` into `BrowserUse-windows-x64.zip`.
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
Push-Location $RepoRoot
try {
    & (Join-Path $PSScriptRoot 'test.ps1')
    $SourceDir = Join-Path $RepoRoot 'build\windows\Browser Use'
    $ZipPath = Join-Path $RepoRoot 'BrowserUse-windows-x64.zip'
    if (Test-Path $ZipPath) {
        Remove-Item -Force $ZipPath
    }
    Compress-Archive -Path "$SourceDir\*" -DestinationPath $ZipPath
    Write-Host "==> Release artifact created: $ZipPath"
}
finally {
    Pop-Location
}
