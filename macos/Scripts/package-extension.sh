#!/usr/bin/env bash
#
# Stages the unpacked Gecko Orange WebExtension (`extension/`) and builds a
# clean, AMO-ready `gecko-orange.xpi` archive into a target directory.
#
# Usage:
#   ./macos/Scripts/package-extension.sh              # -> <repo>/build/{extension/,gecko-orange.xpi}
#   ./macos/Scripts/package-extension.sh --install    # -> ~/Library/Application Support/browser-use/{extension/,gecko-orange.xpi}
#   ./macos/Scripts/package-extension.sh --out <dir>  # -> <dir>/{extension/,gecko-orange.xpi}
#   ./macos/Scripts/package-extension.sh <dir>        # -> <dir>/{extension/,gecko-orange.xpi}
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
EXTENSION_SRC="${REPO_ROOT}/extension"

usage() {
  cat >&2 <<'EOF'
Usage: package-extension.sh [--install | --out <dir> | <dir>]
EOF
}

if [[ ! -f "${EXTENSION_SRC}/manifest.json" ]]; then
  echo "error: missing ${EXTENSION_SRC}/manifest.json" >&2
  exit 1
fi

OUT_DIR="${REPO_ROOT}/build"
if [[ $# -eq 1 ]]; then
  case "$1" in
    --install)
      OUT_DIR="${HOME}/Library/Application Support/browser-use"
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    -*)
      echo "error: unknown option: $1" >&2
      usage
      exit 1
      ;;
    *)
      OUT_DIR="$1"
      ;;
  esac
elif [[ $# -eq 2 ]]; then
  if [[ "$1" == "--out" && -n "$2" ]]; then
    OUT_DIR="$2"
  else
    echo "error: invalid arguments" >&2
    usage
    exit 1
  fi
elif [[ $# -gt 2 ]]; then
  echo "error: too many arguments" >&2
  usage
  exit 1
fi

mkdir -p "${OUT_DIR}"
OUT_DIR_ABS="$(cd "${OUT_DIR}" && pwd)"

EXT_STAGE="${OUT_DIR_ABS}/extension"
XPI_PATH="${OUT_DIR_ABS}/gecko-orange.xpi"

rm -rf "${EXT_STAGE}"
mkdir -p "${EXT_STAGE}"

EXTENSION_FILES=(
  manifest.json
  background.js
  content.js
  popup.html
  popup.js
  icon.svg
)

for file in "${EXTENSION_FILES[@]}"; do
  if [[ ! -f "${EXTENSION_SRC}/${file}" ]]; then
    echo "error: missing ${EXTENSION_SRC}/${file}" >&2
    exit 1
  fi
  cp "${EXTENSION_SRC}/${file}" "${EXT_STAGE}/${file}"
done

if [[ -f "${EXTENSION_SRC}/README.md" ]]; then
  cp "${EXTENSION_SRC}/README.md" "${EXT_STAGE}/README.md"
fi

rm -f "${XPI_PATH}"
(
  cd "${EXTENSION_SRC}"
  zip -q -X "${XPI_PATH}" "${EXTENSION_FILES[@]}"
)

echo "==> Extension staged: ${EXT_STAGE}/manifest.json"
echo "==> XPI packaged:     ${XPI_PATH}"
