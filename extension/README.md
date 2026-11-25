# Gecko Orange (Firefox / Zen Extension)

Companion Manifest V2 WebExtension for [browser-use](../README.md) implementing protocol `3.0.0` over a localhost WebSocket (`ws://127.0.0.1:19455/`).

## Files

- `manifest.json` — Extension manifest with fixed Gecko ID (`gecko-orange@browser-use.local`) so paired secrets persist in `browser.storage.local` across reloads.
- `background.js` — Persistent WebSocket client (`hello` / `hello_ok`, 15s `ping`/`pong` heartbeat, automatic reconnect), `OrangeBridge` console API, tab management, navigation, `webRequest` capture/routing, `llms.txt` / URL reading, and screenshot capture/cropping.
- `content.js` — On-demand page script providing durable `@eN` accessibility snapshots (full/delta/diff), DOM interactions (`click`, `fill`, `type`, `press`, `select`, `check`, `uncheck`, `hover`, `scroll`, `focus`), element locators, waits, live DOM Markdown extraction, Set-of-Mark screenshot overlays, and MAIN-world script evaluation.
- `popup.html` / `popup.js` — Toolbar popup for viewing live status, configuring the hub URL, and pasting either a raw shared secret or the full `await OrangeBridge.setSecret("...")` command.
- `icon.svg` — Browser Use brand icon.

## Installing in Zen / Firefox

### Quick Load (Zen & Firefox Release)
1. Open `about:debugging` → **This Zen** (or **This Firefox**).
2. Click **Load Temporary Add-on…** and choose `extension/manifest.json`.
3. Click the **Gecko Orange** toolbar icon, paste your Browser Use secret or setup command, and click **Save & Connect**.

### Permanent Install (Self-Signed `.xpi` for Zen / Firefox Release)
1. Package the files inside `extension/` as a `.xpi` zip (on Windows, `.\windows\scripts\build.ps1` generates `build\windows\Browser Use\gecko-orange.xpi` automatically).
2. Submit the `.xpi` to [Mozilla Add-ons Developer Hub](https://addons.mozilla.org/developers/) as **On your own** (unlisted self-distribution) for free automated signing.
3. Drag-and-drop the signed `.xpi` into Zen or install via `about:addons` → **Install Add-on From File…**.

## Console API

When inspecting the background page (`about:debugging` → **Gecko Orange** → **Inspect** → **Console**):

```js
await OrangeBridge.setSecret("<64-char-hex-secret>")
await OrangeBridge.setUrl("ws://127.0.0.1:19455/")
OrangeBridge.status()
await OrangeBridge.reconnect()
await OrangeBridge.clearSecret()
```
