# browser-use on macOS

Native macOS menu bar app, localhost WebSocket hub, CLI, and stdio MCP server for driving Gecko-based browsers (Firefox / Zen).

## Overview & Architecture

```text
Browser Use.app ── in-process hub ── ws://127.0.0.1:19455 ── Firefox / Zen
      │                   ▲
      │ links             │ ws /peer (lazy reconnect)
      │ libbrowser_use.a  │
      ▼                   │
  browser-use  ───────────┘
  (CLI + stdio MCP server)
```

- **`Browser Use.app`** is a native Swift/AppKit menu bar application (`LSUIElement`) that statically links `libbrowser_use.a` and hosts the loopback HTTP/WebSocket hub in-process. App open = hub on; quit = hub off.
- **`browser-use`** is the bundled terminal CLI and stdio JSON-RPC 2.0 MCP server (`Contents/MacOS/browser-use`). It connects lazily to the running hub over `/peer` and automatically reconnects across app restarts.

---

## Requirements

- **macOS 15+**
- **Firefox or Zen Browser** configured with the bundled **Gecko Orange** extension (protocol `3.0.0`)

### Additional Requirements to Build From Source
- **Xcode 16+ / Swift 6**
- **Rust 1.80+** (`cargo`)
- **[`just`](https://github.com/casey/just)**

---

## Installation

### Option A: Build & Install From Source (Recommended)

From the repository root:

```bash
just install
```

This builds the release Rust static library, CLI, and `Browser Use.app`, and installs:
- `/Applications/Browser Use.app` (with `extension/` and `gecko-orange.xpi` bundled inside `Contents/Resources/`)
- `~/.local/bin/browser-use` (symlink to `/Applications/Browser Use.app/Contents/MacOS/browser-use`)
- `~/Library/Application Support/browser-use/extension/` and `~/Library/Application Support/browser-use/gecko-orange.xpi`

Ensure `~/.local/bin` is on your `PATH` (for example in `~/.zshrc`):

```zsh
export PATH="$HOME/.local/bin:$PATH"
```

If you only need to package or install the Zen / Firefox extension without rebuilding the macOS app:

```bash
just extension          # stages build/extension/ and build/gecko-orange.xpi
just install-extension  # installs into ~/Library/Application Support/browser-use/
```

(Or invoke `./macos/Scripts/package-extension.sh --install` directly without `just`.)

### Option B: Install From a Release Zip (`BrowserUse.zip`)

1. Unzip `BrowserUse.zip`. The archive contains `Browser Use.app`, `extension/`, and `gecko-orange.xpi`.
2. Drag **`Browser Use.app`** into `/Applications`.
3. Optionally symlink the bundled CLI onto your `PATH`:
   ```bash
   mkdir -p ~/.local/bin
   ln -sfn "/Applications/Browser Use.app/Contents/MacOS/browser-use" ~/.local/bin/browser-use
   ```
4. Keep the extracted `extension/` folder and `gecko-orange.xpi` handy, or click **Open Extension Folder** inside `Browser Use.app`—the app automatically stages the bundled extension into `~/Library/Application Support/browser-use/` and reveals `manifest.json` in Finder.

---

## First-Time Setup (Installing the Extension & Pairing Zen / Firefox)

`Browser Use.app` hosts the local WebSocket hub, and the bundled **Gecko Orange** browser extension (`extension/`) connects Zen / Firefox to it.

### Step 1: Launch Browser Use

1. Open **Browser Use** (`/Applications/Browser Use.app`, Spotlight, or `open "/Applications/Browser Use.app"`).
2. It runs in the **menu bar** (top-right status area). Click the Browser Use menu bar icon and choose **Open Dashboard…** (or **Settings…**).

### Step 2: Install the Gecko Orange Extension in Zen / Firefox

Choose one of the installation methods below:

#### Option 1: Immediate Load (Zen & Firefox Release — No Account Needed)
1. In **Zen** (or Firefox), type `about:debugging` in the address bar and press **Return**.
2. In the left sidebar, click **This Zen** (or **This Firefox**).
3. Click **Load Temporary Add-on…** at the top right.
4. Select `extension/manifest.json`:
   - **Installed location:** `~/Library/Application Support/browser-use/extension/manifest.json`
   - **Finder shortcut:** Click **Open Extension Folder** in the Browser Use menu bar menu, Dashboard, or Settings → Connection to reveal `manifest.json` in Finder. You can drag `manifest.json` directly onto Zen's file-open dialog, or press **`⌘⇧G`** inside the dialog and paste `~/Library/Application Support/browser-use/extension`.
   - **From a source checkout:** `<repo>/extension/manifest.json`
5. **Gecko Orange** appears under **Temporary Extensions**.

> **Why "Temporary"?** Zen and standard Firefox Release enforce Mozilla add-on signing for permanent installs, so extensions loaded via `about:debugging` unload when you quit the browser. Click **Load Temporary Add-on…** and pick `manifest.json` again after restarting Zen—your saved secret persists automatically in `browser.storage.local` thanks to the fixed Gecko ID (`gecko-orange@browser-use.local`).

#### Option 2: Permanent Install in Zen / Firefox Release (Free Mozilla Self-Signing)
If you want Gecko Orange to stay installed across Zen restarts:
1. Create a free developer account at [Mozilla Add-ons Developer Hub](https://addons.mozilla.org/developers/).
2. Submit the packaged `gecko-orange.xpi` (`~/Library/Application Support/browser-use/gecko-orange.xpi` or `build/gecko-orange.xpi`) choosing **On your own** (unlisted self-distribution). Automated signing typically finishes in 1–2 minutes.
3. Download the signed `.xpi` and drag-and-drop it into Zen (or open `about:addons` → gear icon → **Install Add-on From File…**).

#### Option 3: Permanent Unsigned Install (Firefox Developer Edition / Nightly / ESR)
1. Open `about:config` and set `xpinstall.signatures.required` to `false`.
2. Open `about:addons` → gear icon → **Install Add-on From File…** and select `gecko-orange.xpi`.

### Step 3: Pair Zen / Firefox With Browser Use

You can pair using either the **toolbar popup** (easiest) or the **extension console**:

#### Method A: Extension Toolbar Popup (Recommended)
1. Copy your secret or setup command from **Browser Use**:
   - Click the menu bar icon → **Copy Shared Secret** (or **Copy Zen Setup Command**)
   - Or in the **Dashboard** → **Copy Setup Command**
   - Or in Terminal → `browser-use secret`
2. Click the **Gecko Orange** icon in Zen's toolbar (open the puzzle-piece **Extensions** button if it isn't pinned).
3. Paste what you copied into **Shared Secret (or Zen Setup Command)**—both the raw 64-character secret and the full `await OrangeBridge.setSecret("...")` command work.
4. Click **Save & Connect**. The extension badge turns green (**ON**).

#### Method B: Extension Console (`about:debugging`)
1. Copy the setup command via menu bar icon → **Copy Zen Setup Command** (or run `browser-use secret`).
2. In Zen, open `about:debugging` → **This Zen** (or **This Firefox**).
3. Find **Gecko Orange** under **Temporary Extensions** (or click **`Extensions (N) >`** to expand permanently installed add-ons) and click **Inspect**.
4. Switch to the **Console** tab, paste the command, and press **Return**:
   ```js
   await OrangeBridge.setSecret("<your-64-char-hex-secret>")
   ```

### Step 4: Verify the Connection

The menu bar header and Dashboard update to **LIVE** (`Zen extension connected`) and list your open tabs. You can also verify from Terminal:

```bash
browser-use status
```

---

## Using the macOS App (`Browser Use.app`)

### Menu Bar Menu
- Click the **Browser Use** menu bar icon to view:
  - Connection status header (`LIVE`, `WAITING`, `STARTING`, `SECRET`, `OFFLINE`, `ERROR`)
  - Extension version, hub URL (`ws://127.0.0.1:19455`), and connected peer count
  - **Tabs** submenu (hover to list open browser tabs, click any tab to switch to it, or open Dashboard)
  - Quick actions: **Copy Shared Secret**, **Copy Zen Setup Command**, **Open Extension Folder**, **Restart Hub**, **Reveal Hub Logs**
  - **Open Dashboard…**, **Settings…**, **Quit Browser Use**

### Dashboard Window
- Shows live phase badge, selectable hub URL, extension version, last update time, and **Tabs / Peers / Uptime** counters.
- Displays the **Finish connecting Zen** setup card whenever the extension is disconnected, with one-click **Copy Setup Command**, **Open Extension Folder**, **Copy Shared Secret**, and **Reveal Hub Logs** buttons.
- Provides live tab search, one-click tab switching, and a right-click tab context menu (**Switch to Tab**, **Copy URL**).
- Closing the Dashboard window hides it without stopping the hub; use **Quit Browser Use** in the menu bar menu to stop the hub and exit.

### Settings Window
- **General:** Inspect hub state, restart the hub, change the hub port (updates settings and restarts the hub; must match Zen's `orange_bridge.url`), toggle/limit tabs in the menu bar (`3`–`20`), adjust refresh cadence (`1s`–`10s`), toggle Dock icon visibility, toggle **Start Browser Use at login**, or **Reset Preferences**.
- **Connection:** Inspect hub address/state/extension/protocol/peers/uptime, **Reveal / Hide** the shared secret, copy the secret or Zen setup command, **Open Extension Folder**, and trigger **Restart Hub** / **Refresh Now**.
- **About:** App version, hub summary, shortcuts to **README**, **Reveal Hub Logs** (`~/Library/Logs/browser-use/hub.log`), and **Secret Location**.

### Utility Flags on `Browser Use` Binary
- `--register-login` / `--unregister-login` / `--login-status`: Register, unregister, or query the macOS login item via `SMAppService`.
- `--dump-windows`: Run activation-policy diagnostics.
- `--dump-status-menu`: Print the assembled `NSMenu` structure to stdout and exit.

---

## Using the CLI (`browser-use`)

Running `browser-use` with no arguments (or `browser-use mcp`) starts the stdio MCP server. Passing a subcommand runs the terminal CLI:

```bash
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
- `ORANGE_SECRET`: Supply the shared secret directly in the environment (overrides `~/.orange-mcp/secret` and never writes to disk).

---

## Connecting MCP Clients

Point your MCP client at the installed `browser-use` binary inside `Browser Use.app`. Running it with no arguments starts the stdio JSON-RPC 2.0 MCP server.

For **Google Antigravity**, edit `~/.gemini/config/mcp_config.json` (inspect active tools under **Additional Options (`...`) → MCP Servers**). The same `"mcpServers"` schema also works for Claude Desktop, Cursor, and other MCP hosts:

```json
{
  "mcpServers": {
    "browser-use": {
      "command": "/Applications/Browser Use.app/Contents/MacOS/browser-use"
    }
  }
}
```

If you use a custom port or environment-supplied secret, include an `"env"` block:

```json
{
  "mcpServers": {
    "browser-use": {
      "command": "/Applications/Browser Use.app/Contents/MacOS/browser-use",
      "env": {
        "ORANGE_PORT": "19455"
      }
    }
  }
}
```

Make sure `Browser Use.app` is running before calling browser tools; if `Browser Use.app` restarts, active MCP sessions reconnect automatically on the next tool call.

> **Tip (Google Antigravity):** Because Antigravity also includes a built-in browser subagent, mention *"using the `browser-use` MCP tools"* or *"in my Zen browser"* when prompting so requests route to your Gecko Orange extension (e.g., *"Using the browser-use MCP tools, list my currently open tabs in Zen."*).

---

## File Locations on macOS

| Item | Path |
|---|---|
| Installed app & bundled CLI | `/Applications/Browser Use.app` (`Contents/MacOS/browser-use`) |
| Bundled extension inside `.app` | `/Applications/Browser Use.app/Contents/Resources/extension/` & `gecko-orange.xpi` |
| CLI symlink | `~/.local/bin/browser-use` |
| User-accessible extension & `.xpi` | `~/Library/Application Support/browser-use/extension/manifest.json`, `gecko-orange.xpi` |
| Shared secret | `~/.orange-mcp/secret` |
| Hub log | `~/Library/Logs/browser-use/hub.log` |
| App preferences | `com.browser-use.menubar` (`~/Library/Preferences/com.browser-use.menubar.plist`) |

---

## Development, Testing & Packaging

```bash
just reload             # debug Rust + Swift build, restart Browser Use.app, wait for /health
just dev-cli            # point ~/.local/bin/browser-use at target/debug/browser-use
just extension          # package extension into build/extension/ and build/gecko-orange.xpi
just test               # cargo test + release bundle + --dump-windows + --dump-status-menu
just snapshot           # render status header, dashboard, and settings screenshots into macos/docs/
./scripts/release.sh    # run just test + build and package BrowserUse.zip
```

---

## Troubleshooting

- **`Browser Use is not running at ws://127.0.0.1:19455. Launch Browser Use first.`**
  Start `Browser Use.app`. Confirm `http://127.0.0.1:19455/health` responds (`curl http://127.0.0.1:19455/health`).
- **`Hub rejected the shared secret`**
  Run `browser-use secret` (or click **Copy Zen Setup Command** in the menu bar menu) and paste the command or secret into the Gecko Orange popup or extension console.
- **Cannot navigate inside `.app` when picking `manifest.json` in Zen**
  macOS file-open dialogs treat `.app` bundles as single files. Load `~/Library/Application Support/browser-use/extension/manifest.json` instead (click **Open Extension Folder** in Browser Use to stage and reveal it in Finder).
- **`Hub: port 19455 is not a usable browser-use hub` / bind failure**
  Another process is using port `19455`. Change the port in **Settings → General → Hub port** (and update the Hub WebSocket URL in Gecko Orange), or set `ORANGE_PORT`.

---

## Uninstalling

```bash
# 1. Stop Browser Use if running
osascript -e 'tell application "Browser Use" to quit' >/dev/null 2>&1 || true
pkill -x "Browser Use" 2>/dev/null || true

# 2. Unregister login item (if enabled)
"/Applications/Browser Use.app/Contents/MacOS/Browser Use" --unregister-login >/dev/null 2>&1 || true

# 3. Remove installed app, CLI symlink, and staged extension
rm -rf "/Applications/Browser Use.app"
rm -f "$HOME/.local/bin/browser-use"
rm -rf "$HOME/Library/Application Support/browser-use"

# 4. Optional: remove logs, shared secret, and preferences
rm -rf "$HOME/Library/Logs/browser-use"
rm -rf "$HOME/.orange-mcp"
defaults delete com.browser-use.menubar >/dev/null 2>&1 || true
```
