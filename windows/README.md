# browser-use on Windows

Native Windows system-tray app, localhost WebSocket hub, CLI, and stdio MCP server for driving Gecko-based browsers (Firefox / Zen).

## Overview & Architecture

```text
Browser Use.exe ── in-process hub ── ws://127.0.0.1:19455 ── Firefox / Zen
      │                   ▲
      │ loads             │ ws /peer (lazy reconnect)
      │ browser_use.dll   │
      ▼                   │
 browser-use.exe ─────────┘
  (CLI + stdio MCP server)
```

- **`Browser Use.exe`** is a native .NET 8 WPF system-tray application that loads `browser_use.dll` and hosts the loopback HTTP/WebSocket hub in-process. App running = hub on; quit = hub off.
- **`browser-use.exe`** is the bundled terminal CLI and stdio JSON-RPC 2.0 MCP server. It connects lazily to the running hub over `/peer` and automatically reconnects across app restarts.

---

## Requirements

### To Run Prebuilt Binaries
- **Windows 10 or Windows 11 (x64)**
- **[.NET 8 Desktop Runtime (x64)](https://dotnet.microsoft.com/en-us/download/dotnet/8.0)**
- **Firefox or Zen** configured with the Gecko Orange extension (protocol `3.0.0`)

### To Build From Source
Choose either:
- **Isolated local toolchain (recommended):** Use `windows\scripts\dev-env.ps1 -Install` to install Rust (`stable-x86_64-pc-windows-gnullvm`), portable `llvm-mingw`, and the .NET 8 SDK strictly inside `<repo>\.devenv\` without modifying your global `PATH`, user profile, or Windows registry.
- **System toolchain:** Rust 1.80+ (`cargo`) and the .NET 8 SDK installed on `PATH`.

---

## Installation

### Option A: Build & Install From Source (Isolated `.devenv\`)

Open PowerShell in the repository root:

```powershell
# 1. One-time isolated toolchain setup (installs into .devenv\ only)
. .\windows\scripts\dev-env.ps1 -Install

# 2. Build Release binaries and install into %LOCALAPPDATA%\Programs\Browser Use\
.\windows\scripts\install.ps1 -AddToPath -StartMenu
```

> **Execution policy note:** If Windows PowerShell blocks local scripts, run with:
> `powershell -ExecutionPolicy Bypass -File .\windows\scripts\install.ps1 -AddToPath -StartMenu`
> (or `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned` once).

Open a new terminal after `-AddToPath` so the updated user `PATH` takes effect.

### Option B: Build & Install Using System Toolchains

If `cargo` and `dotnet` (.NET 8 SDK) are already on your `PATH`:

```powershell
.\windows\scripts\build.ps1
.\windows\scripts\install.ps1 -AddToPath -StartMenu -SkipBuild
```

### Option C: Install From a Release Zip (`BrowserUse-windows-x64.zip`)

1. Extract `BrowserUse-windows-x64.zip` into a permanent folder such as `%LOCALAPPDATA%\Programs\Browser Use\`.
2. Ensure all files (`Browser Use.exe`, `Browser Use.dll`, `Browser Use.deps.json`, `Browser Use.runtimeconfig.json`, `browser_use.dll`, `browser-use.exe`, `extension\`, `gecko-orange.xpi`) stay together in the same directory.
3. Optionally add that directory to your user `PATH` so `browser-use` can be run from any terminal.

---

## First-Time Setup (Installing the Extension & Pairing Firefox / Zen)

`Browser Use.exe` hosts the local WebSocket hub, and the bundled **Gecko Orange** browser extension (`extension\`) connects Firefox / Zen to it.

### Step 1: Launch Browser Use

1. Launch **Browser Use** from the Start Menu (or run `%LOCALAPPDATA%\Programs\Browser Use\Browser Use.exe`).
2. It starts silently in the **system tray** (notification area near the clock). Double-click the tray icon or right-click → **Open Dashboard…** to open the Dashboard.

### Step 2: Install the Gecko Orange Extension in Zen / Firefox

Choose one of the installation methods below:

#### Option 1: Immediate Load (Zen & Firefox Release — No Account Needed)
1. In **Zen / Firefox**, type `about:debugging` in the address bar and press **Enter**.
2. In the left sidebar, click **This Zen** (or **This Firefox**).
3. Click **Load Temporary Add-on…** at the top right.
4. Select `extension\manifest.json`:
   - Installed location: `%LOCALAPPDATA%\Programs\Browser Use\extension\manifest.json`
   - Or click **Open Extension Folder** from the Browser Use tray menu / Dashboard to reveal `manifest.json` in File Explorer.
   - From a source checkout: `<repo>\extension\manifest.json`
5. **Gecko Orange** appears under **Temporary Extensions**.

> **Why "Temporary"?** Zen and standard Firefox Release enforce Mozilla add-on signing for permanent installs, so extensions loaded via `about:debugging` unload when you close the browser. Simply click **Load Temporary Add-on…** and pick `manifest.json` again after restarting Zen—your saved secret persists automatically in browser storage.

#### Option 2: Permanent Install in Zen / Firefox Release (Free Mozilla Self-Signing)
If you want Gecko Orange to stay installed across Zen restarts:
1. Create a free developer account at [Mozilla Add-ons Developer Hub](https://addons.mozilla.org/developers/).
2. Submit the bundled `gecko-orange.xpi` (`%LOCALAPPDATA%\Programs\Browser Use\gecko-orange.xpi`) choosing **On your own** (unlisted self-distribution). Automated signing typically finishes in 1–2 minutes.
3. Download the signed `.xpi` and drag-and-drop it into Zen (or open `about:addons` → gear icon → **Install Add-on From File…**).

#### Option 3: Permanent Unsigned Install (Firefox Developer Edition / Nightly / ESR)
1. Open `about:config` and set `xpinstall.signatures.required` to `false`.
2. Open `about:addons` → gear icon → **Install Add-on From File…** and select `gecko-orange.xpi`.

### Step 3: Pair Zen / Firefox With Browser Use

You can pair using either the **toolbar popup** (easiest) or the **extension console**:

#### Method A: Extension Toolbar Popup (Recommended)
1. Copy your secret or setup command from **Browser Use**:
   - Right-click the tray icon → **Copy Shared Secret** (or **Copy Zen Setup Command**)
   - Or in the **Dashboard** → **Copy Setup Command**
   - Or in PowerShell → `browser-use secret`
2. Click the **Gecko Orange** icon in Zen's toolbar (open the puzzle-piece **Extensions** button if it isn't pinned).
3. Paste what you copied into **Shared Secret (or Zen Setup Command)**—both the raw 64-character secret and the full `await OrangeBridge.setSecret("...")` line work.
4. Click **Save & Connect**. The extension badge turns green (**ON**).

#### Method B: Extension Console (`about:debugging`)
1. Copy the setup command via tray icon → **Copy Zen Setup Command** (or run `browser-use secret`).
2. In Zen, open `about:debugging` → **This Zen** (or **This Firefox**).
3. Find **Gecko Orange** under **Temporary Extensions** (or click **`Extensions (N) >`** to expand permanently installed add-ons) and click **Inspect**.
4. Switch to the **Console** tab, paste the command, and press **Enter**:
   ```js
   await OrangeBridge.setSecret("<your-64-char-hex-secret>")
   ```

### Step 4: Verify the Connection

The tray icon menu and Dashboard will update to **LIVE** (`Zen extension connected`) and display your open tabs. You can also verify from PowerShell:

```powershell
browser-use status
```

---

## Using the Windows Desktop App (`Browser Use.exe`)

### System Tray Menu
- **Right-click** the Browser Use tray icon to view:
  - Connection status header (`LIVE`, `WAITING`, `STARTING`, `SECRET`, `OFFLINE`, `ERROR`)
  - Extension version, hub URL (`ws://127.0.0.1:19455`), and connected peer count
  - **Tabs** submenu (hover to list open browser tabs, click any tab to switch to it, or open Dashboard)
  - Quick actions: **Copy Shared Secret**, **Copy Zen Setup Command**, **Open Extension Folder**, **Restart Hub**, **Reveal Hub Logs**
  - **Open Dashboard…**, **Settings…**, **Quit Browser Use**
- **Double-click** the tray icon (or launch `Browser Use.exe` again) to focus/open the Dashboard window.

### Dashboard Window
- Shows live phase badge, selectable hub URL, extension version, last update time, and **Tabs / Peers / Uptime** counters.
- Displays the **Finish connecting Zen** setup card whenever the extension is disconnected.
- Provides live tab search, one-click tab switching, and a right-click tab context menu (**Switch to Tab**, **Copy URL**).
- Closing the Dashboard window hides it to the tray without stopping the hub; use **Quit Browser Use** in the tray menu to stop the hub and exit.

### Settings Window
- **General:** Inspect hub state, restart the hub, change the hub port (updates settings and restarts the hub; must match Zen's `orange_bridge.url`), toggle/limit tabs in the tray menu (`3`–`20`), adjust refresh cadence (`1s`–`10s`), toggle **Start Browser Use at login**, or **Reset Preferences**.
- **Connection:** Inspect hub address/state/extension/protocol/peers/uptime, **Reveal / Hide** the shared secret, copy the secret or Zen setup command, and trigger **Restart Hub** / **Refresh Now**.
- **About:** App version, hub summary, shortcuts to **README**, **Reveal Hub Logs** (selects `hub.log` in File Explorer), and **Secret Location**.

### Utility Flags on `Browser Use.exe`
- `--show-dashboard`: Launch and immediately open the Dashboard window (or focus an existing instance).
- `--allow-multiple`: Bypass the single-instance mutex (useful for isolated tests on a custom `ORANGE_PORT`).
- `--register-login` / `--unregister-login` / `--login-status`: Manage or query the Windows startup entry (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`).
- `--dump-status-menu`: Print a preview tray menu structure to stdout and exit.

---

## Using the CLI (`browser-use.exe`)

Running `browser-use.exe` with no arguments (or `browser-use mcp`) starts the stdio MCP server. Passing a subcommand runs the terminal CLI:

```powershell
browser-use status                         # hub, extension connection, peers, and open tabs
browser-use secret                         # print/initialize the shared secret + Zen setup command
browser-use snap [-i] [-c]                 # accessibility snapshot (interactive / compact)
browser-use snap --selector "#main" --urls # scoped snapshot with link URLs
browser-use click <@ref|selector>          # click an element (--double, --right, --force)
browser-use fill <@ref|selector> <text>    # fill a field (--slow, --delay <ms>, --enter)
browser-use read [url]                     # extract page Markdown (--outline, --llms, --filter)
browser-use --help                         # full flags & options
browser-use --version                      # print version
```

### Environment Variables
- `ORANGE_PORT`: Override hub port (default `19455`, valid range `1`–`65535`).
- `ORANGE_SECRET`: Supply the shared secret directly in the environment (overrides `%USERPROFILE%\.orange-mcp\secret` and never writes to disk).

---

## Connecting MCP Clients

Point your MCP client at the installed `browser-use.exe` binary. Running it with no arguments starts the stdio JSON-RPC 2.0 MCP server.

For **Google Antigravity**, edit `%USERPROFILE%\.gemini\config\mcp_config.json` (inspect active tools under **Additional Options (`...`) → MCP Servers**). The same `"mcpServers"` schema also works for Claude Desktop, Cursor, and other MCP hosts:

```json
{
  "mcpServers": {
    "browser-use": {
      "command": "C:\\Users\\<YourUsername>\\AppData\\Local\\Programs\\Browser Use\\browser-use.exe"
    }
  }
}
```

If you use a custom port or environment-supplied secret, include an `"env"` block:

```json
{
  "mcpServers": {
    "browser-use": {
      "command": "C:\\Users\\<YourUsername>\\AppData\\Local\\Programs\\Browser Use\\browser-use.exe",
      "env": {
        "ORANGE_PORT": "19455"
      }
    }
  }
}
```

Make sure `Browser Use.exe` is running before calling browser tools; if `Browser Use.exe` restarts, active MCP sessions reconnect automatically on the next tool call.

> **Tip (Google Antigravity):** Because Antigravity also includes a built-in browser subagent, mention *"using the `browser-use` MCP tools"* or *"in my Zen browser"* when prompting so requests route to your Gecko Orange extension (e.g., *"Using the browser-use MCP tools, list my currently open tabs in Zen."*).

---

## File Locations on Windows

| Item | Path |
|---|---|
| Installed app, CLI & extension | `%LOCALAPPDATA%\Programs\Browser Use\` (`extension\manifest.json`, `gecko-orange.xpi`) |
| Start Menu shortcut | `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Browser Use.lnk` |
| Shared secret | `%USERPROFILE%\.orange-mcp\secret` (or `$HOME\.orange-mcp\secret` if `HOME` is set) |
| App settings | `%APPDATA%\browser-use\settings.json` |
| Hub log | `%LOCALAPPDATA%\browser-use\logs\hub.log` |
| Launch at login | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` (`"Browser Use"`) |

---

## Development, Testing & Packaging

All PowerShell scripts under `windows\scripts\` automatically activate `.devenv\` if `cargo` or `dotnet` is not already on `PATH` (or when `-UseDevEnv` is passed):

```powershell
# Run Rust unit/integration tests, release build, browser_use.dll PE import audit,
# --dump-status-menu check, and live Browser Use.exe <-> browser-use.exe smoke test
.\windows\scripts\test.ps1

# Build Debug or Release bundle into build\windows\Browser Use\
.\windows\scripts\build.ps1 -Config Debug
.\windows\scripts\build.ps1 -Config Release

# Package BrowserUse-windows-x64.zip (runs test.ps1 first)
.\windows\scripts\release.ps1

# Re-render windows\BrowserUse\Resources\AppIcon.ico (pure Python, no pip deps)
python .\windows\scripts\render-icon.py
```

To remove the isolated `.devenv\` toolchain completely:

```powershell
Remove-Item -Recurse -Force .devenv
```

---

## Troubleshooting

- **`Browser Use is not running at ws://127.0.0.1:19455. Launch Browser Use first.`**
  Start `Browser Use.exe`. Confirm `http://127.0.0.1:19455/health` responds in a browser or via `Invoke-RestMethod http://127.0.0.1:19455/health`.
- **`Hub rejected the shared secret`**
  Run `browser-use secret` (or click **Copy Zen Setup Command** in the tray menu) and re-run `await OrangeBridge.setSecret(...)` in Zen's extension console.
- **`Hub: port 19455 is not a usable browser-use hub` / bind failure**
  Another process is using port `19455`. Change the port in **Settings → General → Hub port** (and update Zen's `orange_bridge.url`), or set `ORANGE_PORT`.
- **SmartScreen / PowerShell execution policy warnings**
  Local builds are unsigned. If PowerShell blocks script execution, run `powershell -ExecutionPolicy Bypass -File <script>`.

---

## Uninstalling

```powershell
# 1. Stop Browser Use if running
Get-Process -Name 'Browser Use' -ErrorAction SilentlyContinue | Stop-Process -Force

# 2. Remove launch-at-login entry (if enabled)
Remove-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'Browser Use' -ErrorAction SilentlyContinue

# 3. Remove installed binaries and Start Menu shortcut
Remove-Item -Recurse -Force "$env:LOCALAPPDATA\Programs\Browser Use" -ErrorAction SilentlyContinue
Remove-Item -Force "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\Browser Use.lnk" -ErrorAction SilentlyContinue

# 4. Optional: remove settings, logs, and shared secret
Remove-Item -Recurse -Force "$env:APPDATA\browser-use" -ErrorAction SilentlyContinue
Remove-Item -Recurse -Force "$env:LOCALAPPDATA\browser-use" -ErrorAction SilentlyContinue
Remove-Item -Recurse -Force "$env:USERPROFILE\.orange-mcp" -ErrorAction SilentlyContinue
```

If you passed `-AddToPath`, you can also remove `%LOCALAPPDATA%\Programs\Browser Use` from your user `PATH` in **System Properties → Environment Variables**.
