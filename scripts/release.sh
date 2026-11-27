#!/usr/bin/env bash
set -euo pipefail

echo "==> Running tests and verifying build..."
just test

echo "==> Building release..."
just build

echo "==> Packaging release..."
# Remove old zip if it exists
rm -f BrowserUse.zip
(cd build && zip -r ../BrowserUse.zip "Browser Use.app" extension gecko-orange.xpi)

echo "==> Release artifact created: BrowserUse.zip"
