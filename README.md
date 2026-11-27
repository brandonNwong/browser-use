# browser-use

A localhost WebSocket bridge, stdio MCP server, and native macOS menu bar app
for driving Gecko-based browsers (Firefox / Zen).

```text
Browser Use.app ── in-process hub ── ws://127.0.0.1:19455 ── Firefox / Zen
      │                   ▲
      │ links             │ ws /peer (lazy reconnect)
      │ libbrowser_use.a  │
      ▼                   │
  browser-use  ───────────┘
  (CLI + stdio MCP server)
```

- **`Browser Use.app`** hosts the loopback HTTP/WebSocket hub in-process via
  `libbrowser_use.a`. App open = hub on; quit = hub off.
- **`browser-use`** is the bundled CLI and stdio MCP server (`Contents/MacOS/browser-use`),
  lazily reconnecting to the app across restarts.

## Requirements

- macOS 15+, Swift 6 (Xcode 16+)
- Rust 1.80+ (`cargo`)
- [`just`](https://github.com/casey/just)
- Firefox or Zen Browser (using the bundled `extension/` Gecko Orange add-on, protocol `3.0.0`)

## Build & Install

```bash
just install            # release build -> /Applications/Browser Use.app, ~/.local/bin/browser-use, and ~/Library/Application Support/browser-use/
just build              # release build only -> build/Browser Use.app + build/extension/ + build/gecko-orange.xpi
just extension          # package extension only -> build/extension/ + build/gecko-orange.xpi
just install-extension  # install extension only -> ~/Library/Application Support/browser-use/
just test               # cargo test + release bundle + app sanity checks
```

Dev loop:

```bash
just reload         # debug Rust + Swift build, restart Browser Use.app, wait for /health
just dev-cli        # point ~/.local/bin/browser-use at target/debug/browser-use
```

Other recipes: `just verify`, `just verify-menu`, `just snapshot`, `just icons`, `just stop`, `just clean`.

## Usage (macOS)

1. Launch the app (`open "/Applications/Browser Use.app"` or `just reload`).
2. Load `~/Library/Application Support/browser-use/extension/manifest.json` in Zen/Firefox (`about:debugging` → **This Zen** → **Load Temporary Add-on…**, or click **Open Extension Folder** in Browser Use to reveal it in Finder; for permanent installation, submit `gecko-orange.xpi` for free Mozilla self-signing).
3. Pair using the **Gecko Orange** toolbar icon (or `await OrangeBridge.setSecret(...)` in the extension console).
4. Point your MCP client at the bundled binary:

```bash
"/Applications/Browser Use.app/Contents/MacOS/browser-use"
```

See [macos/README.md](macos/README.md) for full macOS installation, Zen/Firefox pairing, MCP configuration, CLI usage, and troubleshooting instructions.

### CLI

```bash
browser-use status                      # hub, browser connection, and open tabs
browser-use secret                      # print/initialize the shared secret
browser-use snap [-i] [-c]              # accessibility snapshot
browser-use click <@ref|selector>       # click an element
browser-use fill <@ref|selector> <text> # fill a field
browser-use read [url]                  # read page as Markdown
browser-use --help                      # full flags & options
```

## Layout

- `rust/` — Core library (`secret`, `hub`, `peer`, `format`, `tools`, `mcp`, `cli`, `ffi`) + C-ABI header (`rust/include/browser_use.h`)
- `macos/` — SwiftPM menu bar app (`Shell/`, `Kit/`, `DevTools/`, `BrowserUse/`) linking `libbrowser_use.a`
- `windows/` — Native .NET 8 WPF system-tray app (`BrowserUse/`) linking `browser_use.dll` via `rust/include/browser_use.h`, plus PowerShell dev/build/test scripts (`scripts/`)
- `extension/` — Companion **Gecko Orange** Firefox/Zen WebExtension (`manifest.json`, `background.js`, `content.js`, popup UI)

## Windows

### Requirements

- Windows 10/11 (x64)
- Rust 1.80+ (`cargo`) and .NET 8 SDK (or use the isolated `.devenv\` bootstrap below)
- Firefox or Zen Browser (using the bundled `extension/` Gecko Orange add-on)

### Isolated Dev Environment & Build

To keep your global Windows environment untouched, `dev-env.ps1` installs Rust and the .NET 8 SDK strictly inside `<repo>\.devenv\`:

```powershell
. .\windows\scripts\dev-env.ps1 -Install   # one-time isolated toolchain setup
.\windows\scripts\build.ps1                # builds browser-use.exe, browser_use.dll, Browser Use.exe, and bundles extension/ -> build\windows\Browser Use\
.\windows\scripts\test.ps1                 # cargo test + release bundle + DLL import check + live smoke test
.\windows\scripts\install.ps1 -AddToPath -StartMenu  # installs to %LOCALAPPDATA%\Programs\Browser Use\
```

Launch `Browser Use.exe`, load `%LOCALAPPDATA%\Programs\Browser Use\extension\manifest.json` in Zen/Firefox (`about:debugging` → **This Zen** → **Load Temporary Add-on…**), pair using the Gecko Orange toolbar icon (or `await OrangeBridge.setSecret(...)`), and point your MCP client at the bundled CLI binary:

```text
%LOCALAPPDATA%\Programs\Browser Use\browser-use.exe
```

See [windows/README.md](windows/README.md) for full Windows installation, Firefox/Zen pairing, MCP configuration, CLI usage, and troubleshooting instructions.