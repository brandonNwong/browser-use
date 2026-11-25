<#
.SYNOPSIS
    Runs the full Windows verification suite:
      1. `cargo test` (all Rust unit + integration tests)
      2. `build.ps1 -Config Release`
      3. PE import audit on `browser_use.dll` (only Windows system DLLs allowed)
      4. `browser-use.exe --version` and `--help`
      5. `Browser Use.exe --dump-status-menu`
      6. Live end-to-end smoke test: `Browser Use.exe` <-> `browser-use.exe status`
#>
[CmdletBinding()]
param(
    [switch]$UseDevEnv
)

$ErrorActionPreference = 'Stop'

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$DevEnvScript = Join-Path $PSScriptRoot 'dev-env.ps1'

$NeedsDevEnv = $UseDevEnv -or (-not (Get-Command cargo -ErrorAction SilentlyContinue)) -or (-not (Get-Command dotnet -ErrorAction SilentlyContinue))
if ($NeedsDevEnv -and (Test-Path (Join-Path $RepoRoot '.devenv\cargo\bin\cargo.exe'))) {
    . $DevEnvScript -Quiet
}

Push-Location $RepoRoot
try {
    Write-Host '==> 1/6 Running cargo test...'
    & cargo test
    if ($LASTEXITCODE -ne 0) { throw "cargo test failed ($LASTEXITCODE)" }

    Write-Host '==> 2/6 Building Release bundle...'
    & (Join-Path $PSScriptRoot 'build.ps1') -Config Release
    if ($LASTEXITCODE -ne 0) { throw "build.ps1 failed ($LASTEXITCODE)" }

    $BundleDir = Join-Path $RepoRoot 'build\windows\Browser Use'
    $AppExe = Join-Path $BundleDir 'Browser Use.exe'
    $CliExe = Join-Path $BundleDir 'browser-use.exe'
    $HubDll = Join-Path $BundleDir 'browser_use.dll'
    $ExtManifest = Join-Path $BundleDir 'extension\manifest.json'
    $ExtXpi = Join-Path $BundleDir 'gecko-orange.xpi'
    if (-not (Test-Path $ExtManifest)) { throw "Missing bundled extension manifest: $ExtManifest" }
    if (-not (Test-Path $ExtXpi)) { throw "Missing bundled extension archive: $ExtXpi" }

    Write-Host '==> 3/6 Auditing browser_use.dll PE imports...'
    $PeAuditScript = @'
import struct, sys
from pathlib import Path

dll = Path(sys.argv[1]).read_bytes()
pe_off = struct.unpack_from("<I", dll, 0x3C)[0]
opt_off = pe_off + 24
magic = struct.unpack_from("<H", dll, opt_off)[0]
num_sections = struct.unpack_from("<H", dll, pe_off + 6)[0]
opt_size = struct.unpack_from("<H", dll, pe_off + 20)[0]
sec_off = opt_off + opt_size
dd_off = opt_off + (112 if magic == 0x20B else 96)
imp_rva, _ = struct.unpack_from("<II", dll, dd_off + 8)

sections = []
for i in range(num_sections):
    s = sec_off + i * 40
    vsize, va, rsize, roff = struct.unpack_from("<IIII", dll, s + 8)
    sections.append((va, max(vsize, rsize), roff))

def rva_to_off(rva):
    for va, span, roff in sections:
        if va <= rva < va + span:
            return roff + (rva - va)
    raise ValueError(f"unmapped rva {rva:#x}")

imports = []
off = rva_to_off(imp_rva)
while True:
    _, _, _, name_rva, _ = struct.unpack_from("<IIIII", dll, off)
    if name_rva == 0:
        break
    n_off = rva_to_off(name_rva)
    end = dll.index(b"\x00", n_off)
    imports.append(dll[n_off:end].decode("ascii", errors="replace").lower())
    off += 20

print("    DLL imports:", ", ".join(imports))
disallowed = [
    d for d in imports
    if not (
        d in {"kernel32.dll", "msvcrt.dll", "ws2_32.dll", "advapi32.dll", "userenv.dll",
              "bcrypt.dll", "bcryptprimitives.dll", "ntdll.dll", "user32.dll", "shell32.dll",
              "ole32.dll", "crypt32.dll", "iphlpapi.dll", "secur32.dll", "ucrtbase.dll"}
        or d.startswith("api-ms-win-")
    )
]
if disallowed:
    print(f"ERROR: non-system DLL dependencies found: {disallowed}", file=sys.stderr)
    sys.exit(1)
'@
    & python -c ($PeAuditScript -replace '"', "'") $HubDll
    if ($LASTEXITCODE -ne 0) { throw "PE import audit failed ($LASTEXITCODE)" }

    Write-Host '==> 4/6 Checking browser-use.exe CLI...'
    $VersionOut = & $CliExe --version
    if ($LASTEXITCODE -ne 0 -or ($VersionOut -notmatch 'browser-use 0\.1\.0')) {
        throw "Unexpected --version output: $VersionOut"
    }
    $HelpOut = & $CliExe --help
    if ($LASTEXITCODE -ne 0 -or (($HelpOut -join "`n") -notmatch 'Usage:')) {
        throw 'browser-use.exe --help failed'
    }

    Write-Host '==> 5/6 Checking Browser Use.exe --dump-status-menu...'
    $MenuOut = (& $AppExe --dump-status-menu | Out-String)
    if ($LASTEXITCODE -ne 0) { throw "--dump-status-menu failed ($LASTEXITCODE)" }
    foreach ($expected in @('Browser Use', 'Tabs (3)', 'Copy Shared Secret', 'Restart Hub', 'Quit Browser Use')) {
        if ($MenuOut -notlike "*$expected*") {
            throw "--dump-status-menu missing '$expected':`n$MenuOut"
        }
    }

    Write-Host '==> 6/6 Running live Browser Use.exe <-> browser-use.exe smoke test...'
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    $listener.Start()
    $Port = ([System.Net.IPEndPoint]$listener.LocalEndpoint).Port
    $listener.Stop()

    $Secret = '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef'
    $Psi = [System.Diagnostics.ProcessStartInfo]::new()
    $Psi.FileName = $AppExe
    $Psi.Arguments = '--allow-multiple'
    $Psi.WorkingDirectory = $BundleDir
    $Psi.UseShellExecute = $false
    $Psi.Environment['ORANGE_PORT'] = $Port.ToString()
    $Psi.Environment['ORANGE_SECRET'] = $Secret

    $AppProc = [System.Diagnostics.Process]::Start($Psi)
    try {
        $Healthy = $false
        for ($i = 0; $i -lt 50; $i++) {
            Start-Sleep -Milliseconds 150
            if ($AppProc.HasExited) {
                throw "Browser Use.exe exited prematurely with code $($AppProc.ExitCode)"
            }
            try {
                $Resp = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/health" -TimeoutSec 2
                if ($Resp.ok -eq $true -and $Resp.port -eq $Port) {
                    $Healthy = $true
                    break
                }
            }
            catch {
                # Retry until hub finishes binding.
            }
        }
        if (-not $Healthy) {
            throw "Hub inside Browser Use.exe did not answer /health on port $Port"
        }

        $PrevPort = $env:ORANGE_PORT
        $PrevSecret = $env:ORANGE_SECRET
        try {
            $env:ORANGE_PORT = $Port.ToString()
            $env:ORANGE_SECRET = $Secret
            $StatusLines = & $CliExe status
            $StatusExit = $LASTEXITCODE
            $StatusText = $StatusLines -join "`n"
            if ($StatusExit -ne 0 -or $StatusText -notmatch "Hub: running at ws://127\.0\.0\.1:$Port") {
                throw "Live status check failed (exit $StatusExit):`n$StatusText"
            }
            Write-Host "    Live smoke check passed on port $Port"
        }
        finally {
            if ($null -eq $PrevPort) { Remove-Item Env:ORANGE_PORT -ErrorAction SilentlyContinue } else { $env:ORANGE_PORT = $PrevPort }
            if ($null -eq $PrevSecret) { Remove-Item Env:ORANGE_SECRET -ErrorAction SilentlyContinue } else { $env:ORANGE_SECRET = $PrevSecret }
        }
    }
    finally {
        if (-not $AppProc.HasExited) {
            $AppProc.Kill()
            $AppProc.WaitForExit(3000) | Out-Null
        }
        $AppProc.Dispose()
    }

    Write-Host '==> All Windows checks passed!'
}
finally {
    Pop-Location
}
