#!/usr/bin/env bash
#
# Builds the SwiftPM executable and wraps it in a real .app bundle, together
# with the `browser-use` CLI/MCP binary that talks to the in-process hub.
#
# Output (matching the root justfile and README):
#   <repo-root>/build/<APP_NAME>.app/Contents/MacOS/<APP_NAME>
#   <repo-root>/build/<APP_NAME>.app/Contents/MacOS/browser-use
#   <repo-root>/build/<APP_NAME>.app/Contents/Resources/extension/
#   <repo-root>/build/<APP_NAME>.app/Contents/Resources/gecko-orange.xpi
# SwiftPM's own scratch directory stays inside the package at macos/.build/.
#
# The bundle matters for three reasons:
#   1. Info.plist LSUIElement is what makes the process an accessory app.
#   2. SwiftUI's Settings scene and UserDefaults both want a bundle identifier.
#   3. macOS will not keep a menu bar extra running for a bare executable.
#
# The app statically links `libbrowser_use.a`; build the Rust crate first:
#   cargo build                 # CONFIG=debug   -> target/debug
#   cargo build --release       # CONFIG=release -> target/release
#
# Configuration comes from the environment (set once in the root justfile):
#   APP_NAME      bundle name, display name, CFBundleExecutable   [Browser Use]
#   PRODUCT_NAME  SwiftPM executable to copy (constant)           [App]
#   BUNDLE_ID     CFBundleIdentifier                              [com.browser-use.menubar]
#   VERSION       CFBundleShortVersionString                      [1.0]
#   BUILD         CFBundleVersion                                 [1]
#   CONFIG        debug | release                                 [release]
#
# Usage:  CONFIG=debug ./macos/Scripts/build-app.sh [--universal]
#         (works from any CWD; the paths derive from this file's location)
#
# `--universal` additionally requires a universal (arm64 + x86_64)
# libbrowser_use.a, i.e. a Rust build for both targets.
#
set -euo pipefail

APP_NAME="${APP_NAME:-Browser Use}"
PRODUCT_NAME="${PRODUCT_NAME:-App}"
VERSION="${VERSION:-1.0}"
BUILD="${BUILD:-1}"
CONFIG="${CONFIG:-release}"

[[ -n "${APP_NAME}" ]] || { echo "APP_NAME must not be empty" >&2; exit 1; }

case "${CONFIG}" in
  debug|release) ;;
  *) echo "CONFIG must be 'debug' or 'release' (got '${CONFIG}')" >&2; exit 1 ;;
esac

# Package.swift reads this at manifest-evaluation time to pick the cargo
# target directory, so Swift always links the staticlib built for CONFIG.
export BROWSER_USE_CARGO_PROFILE="${CONFIG}"

# Guard the deletes below. What actually needs rejecting is an APP_NAME that
# could escape the repo-root build/ directory or a BUNDLE_ID that could break
# the plist.
case "${APP_NAME}" in
  */*)  echo "APP_NAME must not contain '/': ${APP_NAME}" >&2; exit 1 ;;
  *..*) echo "APP_NAME must not contain '..': ${APP_NAME}" >&2; exit 1 ;;
  .|..) echo "APP_NAME must not be '.' or '..'" >&2; exit 1 ;;
esac
[[ "${APP_NAME}" != *$'\n'* ]] || { echo "APP_NAME must not contain a newline" >&2; exit 1; }

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"     # macos/
REPO_ROOT="$(cd "${ROOT}/.." && pwd)"                        # browser-use/
# The bundle lives beside the Rust workspace, not inside the Swift package:
# <repo-root>/build/Browser Use.app (see the root justfile).
BUILD_DIR="${REPO_ROOT}/build"
APP="${BUILD_DIR}/${APP_NAME}.app"

BUNDLE_ID="${BUNDLE_ID:-com.browser-use.menubar}"

LIB="${REPO_ROOT}/target/${CONFIG}/libbrowser_use.a"
CLI="${REPO_ROOT}/target/${CONFIG}/browser-use"
if [[ ! -f "${LIB}" ]]; then
  echo "error: missing ${LIB}" >&2
  if [[ "${CONFIG}" == "release" ]]; then
    echo "       run 'cargo build --release' first" >&2
  else
    echo "       run 'cargo build' first" >&2
  fi
  exit 1
fi
if [[ ! -x "${CLI}" ]]; then
  echo "error: missing ${CLI}" >&2
  echo "       run 'cargo build' (or 'cargo build --release') first" >&2
  exit 1
fi

ARCH_FLAGS=()
if [[ "${1:-}" == "--universal" ]]; then
  ARCH_FLAGS=(--arch arm64 --arch x86_64)
fi
# bash 3.2 (the macOS default) treats an empty array expansion as unbound under `set -u`.
arch_args() { if (( ${#ARCH_FLAGS[@]} )); then printf '%s\n' "${ARCH_FLAGS[@]}"; fi; }

# Two different escapes are needed for a name like "Notes & Tasks":
#   1. XML — a bare & (or < >) makes the plist invalid.
#   2. sed — an unescaped & in the replacement means "the whole match", and \ or
#      the delimiter would break the expression.
xml_escape() {
  printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'
}
subst() {
  xml_escape "$1" | sed -e 's/[\\&|]/\\&/g'
}

echo "==> Building ${PRODUCT_NAME} (${CONFIG}, cargo profile ${BROWSER_USE_CARGO_PROFILE})"
swift build --package-path "${ROOT}" -c "${CONFIG}" $(arch_args)
BIN_DIR="$(swift build --package-path "${ROOT}" -c "${CONFIG}" $(arch_args) --show-bin-path)"

echo "==> Assembling ${APP}"
rm -rf "${APP}"
mkdir -p "${APP}/Contents/MacOS" "${APP}/Contents/Resources"
cp "${BIN_DIR}/${PRODUCT_NAME}" "${APP}/Contents/MacOS/${APP_NAME}"
cp "${CLI}" "${APP}/Contents/MacOS/browser-use"
chmod +x "${APP}/Contents/MacOS/browser-use"
# Multi-resolution app icon (source: Resources/AppIcon.svg, rendered by
# Scripts/render-app-icon.sh; Info.plist names it via CFBundleIconFile) and the
# vector menu bar template image loaded by StatusItemController.
cp "${ROOT}/Resources/AppIcon.icns" "${APP}/Contents/Resources/AppIcon.icns"
cp "${ROOT}/Resources/MenuBarIcon.svg" "${APP}/Contents/Resources/MenuBarIcon.svg"
if [[ -f "${REPO_ROOT}/README.md" ]]; then
  cp "${REPO_ROOT}/README.md" "${APP}/Contents/Resources/README.md"
fi
if [[ -f "${REPO_ROOT}/extension/manifest.json" ]]; then
  "${ROOT}/Scripts/package-extension.sh" "${APP}/Contents/Resources"
  "${ROOT}/Scripts/package-extension.sh" "${BUILD_DIR}"
fi
printf 'APPL????' > "${APP}/Contents/PkgInfo"

sed -e "s|__APP_NAME__|$(subst "${APP_NAME}")|g" \
    -e "s|__BUNDLE_ID__|$(subst "${BUNDLE_ID}")|g" \
    -e "s|__VERSION__|$(subst "${VERSION}")|g" \
    -e "s|__BUILD__|$(subst "${BUILD}")|g" \
    "${ROOT}/Resources/Info.plist" > "${APP}/Contents/Info.plist"

plutil -lint "${APP}/Contents/Info.plist" >/dev/null

# Ad-hoc signature: enough for local launches and for TCC/Accessibility prompts.
# The nested browser-use CLI binary lives in Contents/MacOS next to the app
# executable; codesign seals it as part of the bundle.
codesign --force --sign - --timestamp=none "${APP}" >/dev/null 2>&1 \
  || echo "note: ad-hoc codesign failed; the app will still run locally"

echo "==> Done: ${APP} (${BUNDLE_ID} ${VERSION} build ${BUILD})"
