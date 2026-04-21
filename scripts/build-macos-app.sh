#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="${NEOMIST_APP_NAME:-NeoMist}"
EXECUTABLE_NAME="${NEOMIST_EXECUTABLE_NAME:-neomist}"
BUNDLE_ID="${NEOMIST_BUNDLE_ID:-org.neomist.app}"
PROFILE="${NEOMIST_PROFILE:-release}"
TARGET_DIR="${CARGO_TARGET_DIR:-${ROOT_DIR}/target}"
APP_DIR_DEFAULT="${ROOT_DIR}/dist/${APP_NAME}.app"
APP_DIR="$APP_DIR_DEFAULT"
ICON_SOURCE="${NEOMIST_ICON_SOURCE:-${ROOT_DIR}/assets/icon.png}"
PLIST_TEMPLATE="${ROOT_DIR}/packaging/macos/Info.plist.template"
BUILD_BINARY=1

usage() {
    cat <<'EOF'
Build NeoMist.app bundle.

Usage:
  scripts/build-macos-app.sh [--skip-build] [--app-dir PATH]

Options:
  --skip-build    Reuse existing Cargo binary instead of running cargo build
  --app-dir PATH  Override output app bundle path
  -h, --help      Show help

Environment:
  NEOMIST_APP_NAME          App bundle name (default: NeoMist)
  NEOMIST_EXECUTABLE_NAME   Binary name inside app (default: neomist)
  NEOMIST_BUNDLE_ID         Bundle identifier (default: org.neomist.app)
  NEOMIST_PROFILE           Cargo profile (default: release)
  NEOMIST_ICON_SOURCE       PNG source for generated AppIcon.icns
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-build)
            BUILD_BINARY=0
            ;;
        --app-dir)
            if [[ $# -lt 2 ]]; then
                printf 'Missing value for %s\n' "$1" >&2
                exit 1
            fi
            APP_DIR="$2"
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            printf 'Unknown option: %s\n' "$1" >&2
            usage >&2
            exit 1
            ;;
    esac
    shift
done

if [[ "$(uname -s)" != "Darwin" ]]; then
    printf 'This script must run on macOS.\n' >&2
    exit 1
fi

if [[ ! -f "$PLIST_TEMPLATE" ]]; then
    printf 'Missing plist template: %s\n' "$PLIST_TEMPLATE" >&2
    exit 1
fi

version="$({ awk -F '"' '/^version = "/ { print $2; exit }' "${ROOT_DIR}/Cargo.toml"; } || true)"
if [[ -z "$version" ]]; then
    printf 'Failed to resolve app version from Cargo.toml.\n' >&2
    exit 1
fi

if [[ "$BUILD_BINARY" -eq 1 ]]; then
    cargo build --manifest-path "${ROOT_DIR}/Cargo.toml" --profile "$PROFILE"
fi

BINARY_PATH="${TARGET_DIR}/${PROFILE}/${EXECUTABLE_NAME}"
if [[ ! -x "$BINARY_PATH" ]]; then
    printf 'Missing built binary: %s\n' "$BINARY_PATH" >&2
    exit 1
fi

CONTENTS_DIR="${APP_DIR}/Contents"
MACOS_DIR="${CONTENTS_DIR}/MacOS"
RESOURCES_DIR="${CONTENTS_DIR}/Resources"
INFO_PLIST_PATH="${CONTENTS_DIR}/Info.plist"

mkdir -p "$(dirname "$APP_DIR")"
rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"

cp "$BINARY_PATH" "${MACOS_DIR}/${EXECUTABLE_NAME}"
chmod 755 "${MACOS_DIR}/${EXECUTABLE_NAME}"

sed \
    -e "s|__APP_NAME__|${APP_NAME}|g" \
    -e "s|__EXECUTABLE_NAME__|${EXECUTABLE_NAME}|g" \
    -e "s|__BUNDLE_ID__|${BUNDLE_ID}|g" \
    -e "s|__APP_VERSION__|${version}|g" \
    "$PLIST_TEMPLATE" > "$INFO_PLIST_PATH"

generate_icns() {
    local iconset_root
    local iconset_dir
    iconset_root="$(mktemp -d "${TMPDIR:-/tmp}/neomist.iconset.XXXXXX")"
    iconset_dir="${iconset_root}/AppIcon.iconset"
    mkdir -p "$iconset_dir"

    local specs=(
        "16 icon_16x16.png"
        "32 icon_16x16@2x.png"
        "32 icon_32x32.png"
        "64 icon_32x32@2x.png"
        "128 icon_128x128.png"
        "256 icon_128x128@2x.png"
        "256 icon_256x256.png"
        "512 icon_256x256@2x.png"
        "512 icon_512x512.png"
        "1024 icon_512x512@2x.png"
    )

    local spec
    local size
    local name
    for spec in "${specs[@]}"; do
        size="${spec%% *}"
        name="${spec#* }"
        sips -z "$size" "$size" "$ICON_SOURCE" --out "${iconset_dir}/${name}" >/dev/null
    done

    iconutil -c icns "$iconset_dir" -o "${RESOURCES_DIR}/AppIcon.icns"
    rm -rf "$iconset_root"
}

if [[ -f "$ICON_SOURCE" ]]; then
    if command -v iconutil >/dev/null 2>&1 && command -v sips >/dev/null 2>&1; then
        generate_icns
    else
        printf 'Skipping AppIcon.icns generation: iconutil or sips missing.\n' >&2
    fi
else
    printf 'Skipping AppIcon.icns generation: missing icon source %s\n' "$ICON_SOURCE" >&2
fi

printf 'Built app bundle: %s\n' "$APP_DIR"
