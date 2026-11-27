# Unified dev recipes for the consolidated browser-use repository.
#
# One runtime owner: `build/Browser Use.app` links `libbrowser_use.a` and hosts
# the WebSocket/HTTP hub in-process. `browser-use` (same Rust crate) is the pure
# follower CLI + stdio MCP server bundled inside the app.

set export

APP_NAME   := "Browser Use"
BUNDLE_ID  := "com.browser-use.menubar"
VERSION    := "0.1.0"
BUILD      := "1"

APP        := "build/" + APP_NAME + ".app"
BIN        := APP + "/Contents/MacOS/" + APP_NAME
CLI_IN_APP := APP + "/Contents/MacOS/browser-use"

default:
    @just --list

# Build release Rust static lib + CLI and assemble build/Browser Use.app.
build:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build --release
    CONFIG=release ./macos/Scripts/build-app.sh

# Fast dev loop: rebuild Rust + Swift, re-bundle, restart the app, relink the
# CLI symlink, and wait for the in-process hub to answer /health.
reload:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build
    CONFIG=debug ./macos/Scripts/build-app.sh

    echo "==> stopping any running Browser Use"
    osascript -e 'tell application "Browser Use" to quit' >/dev/null 2>&1 || true
    for _ in $(seq 1 30); do
        pgrep -x "Browser Use" >/dev/null 2>&1 || break
        sleep 0.1
    done
    pkill -x "Browser Use" 2>/dev/null || true

    mkdir -p "$HOME/.local/bin"
    ln -sfn "$(pwd)/{{CLI_IN_APP}}" "$HOME/.local/bin/browser-use"

    echo "==> launching {{APP}}"
    open "{{APP}}"

    port="${ORANGE_PORT:-19455}"
    echo "==> waiting for hub on ws://127.0.0.1:${port}"
    for _ in $(seq 1 150); do
        if curl -sf "http://127.0.0.1:${port}/health" >/dev/null 2>&1; then
            curl -s "http://127.0.0.1:${port}/health"; echo
            echo "==> ready. Running MCP stdio sessions keep their spawned binary"
            echo "    until the MCP client reconnects; hub/Swift changes are live."
            exit 0
        fi
        sleep 0.1
    done
    echo "error: hub did not become healthy within 15s" >&2
    echo "       see ~/Library/Logs/browser-use/hub.log" >&2
    exit 1

# Point ~/.local/bin/browser-use at the debug CLI for sub-second CLI/MCP
# iteration without rebuilding the Swift app.
dev-cli:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build
    mkdir -p "$HOME/.local/bin"
    ln -sfn "$(pwd)/target/debug/browser-use" "$HOME/.local/bin/browser-use"
    echo "==> ~/.local/bin/browser-use -> $(pwd)/target/debug/browser-use"

# Install the release bundle into /Applications and link the bundled CLI.
install: build
    #!/usr/bin/env bash
    set -euo pipefail
    echo "==> stopping any running Browser Use"
    osascript -e 'tell application "Browser Use" to quit' >/dev/null 2>&1 || true
    sleep 0.5
    pkill -x "Browser Use" 2>/dev/null || true

    rm -rf "/Applications/{{APP_NAME}}.app"
    ditto "{{APP}}" "/Applications/{{APP_NAME}}.app"
    mkdir -p "$HOME/.local/bin"
    ln -sfn "/Applications/{{APP_NAME}}.app/Contents/MacOS/browser-use" \
        "$HOME/.local/bin/browser-use"
    ./macos/Scripts/package-extension.sh --install
    echo "==> installed /Applications/{{APP_NAME}}.app"
    echo "==> ~/.local/bin/browser-use -> /Applications/{{APP_NAME}}.app/Contents/MacOS/browser-use"
    echo "==> extension -> $HOME/Library/Application Support/browser-use/extension/manifest.json"
    echo "==> xpi       -> $HOME/Library/Application Support/browser-use/gecko-orange.xpi"

# Package the Gecko Orange Zen/Firefox extension into build/extension/ and build/gecko-orange.xpi.
extension:
    ./macos/Scripts/package-extension.sh

# Install the Gecko Orange Zen/Firefox extension into ~/Library/Application Support/browser-use/.
install-extension:
    ./macos/Scripts/package-extension.sh --install

# Re-render the multi-resolution app icon from Resources/AppIcon.svg (the menu
# bar mark stays vector and is loaded from Resources/MenuBarIcon.svg).
icons:
    ./macos/Scripts/render-app-icon.sh

# Full test pass: Rust unit/integration tests + app build + dev-tool checks.
test:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo test
    cargo build --release
    CONFIG=release ./macos/Scripts/build-app.sh
    "{{BIN}}" --dump-windows
    "{{BIN}}" --dump-status-menu

# Activation-policy round trip (stages A-I).
verify: build
    "{{BIN}}" --dump-windows

# Assembled NSMenu structure (AppKit draws it, so it can only be printed).
verify-menu: build
    "{{BIN}}" --dump-status-menu

# Render the status-menu header, dashboard and Settings tabs into macos/docs/.
snapshot: build
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p macos/docs
    "{{BIN}}" --render-status-header "$(pwd)/macos/docs/status-menu-header.png"
    "{{BIN}}" --render-settings "$(pwd)/macos/docs/settings-general.png"
    "{{BIN}}" --render-settings-connection "$(pwd)/macos/docs/settings-connection.png"
    "{{BIN}}" --render-settings-about "$(pwd)/macos/docs/settings-about.png"
    "{{BIN}}" --render-dashboard "$(pwd)/macos/docs/dashboard.png"

# Stop the running app.
stop:
    -osascript -e 'tell application "Browser Use" to quit'
    -pkill -x "Browser Use"

# Remove build products (keeps Rust's dependency cache warm).
clean:
    swift package --package-path macos clean
    rm -rf build macos/.build
