#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/load-dotenv.sh
. "${ROOT_DIR}/scripts/load-dotenv.sh"

APP_NAME="${NEOMIST_APP_NAME:-NeoMist}"
EXECUTABLE_NAME="${NEOMIST_EXECUTABLE_NAME:-neomist}"
BUNDLE_ID="${NEOMIST_BUNDLE_ID:-eth.neomist.app}"
PROFILE="${NEOMIST_PROFILE:-release}"
TARGET_DIR="${CARGO_TARGET_DIR:-${ROOT_DIR}/target}"
APP_DIR=""
ICON_SOURCE="${NEOMIST_ICON_SOURCE:-${ROOT_DIR}/assets/icon.png}"
PLIST_TEMPLATE="${ROOT_DIR}/packaging/macos/Info.plist.template"
APP_SIGN_IDENTITY="${NEOMIST_APP_SIGN_IDENTITY:-}"
APP_SIGN_TIMESTAMP="${NEOMIST_APP_SIGN_TIMESTAMP:-1}"
APP_HARDENED_RUNTIME="${NEOMIST_APP_HARDENED_RUNTIME:-1}"
APP_ENTITLEMENTS="${NEOMIST_APP_ENTITLEMENTS:-}"
BUILD_BINARY=1
SIGN_APP=0

usage() {
    cat <<'EOF'
Build NeoMist.app bundle.

Usage:
  scripts/build-macos-app.sh [--skip-build] [--sign] [--app-dir PATH]

Options:
  --skip-build    Reuse existing Cargo binary instead of running cargo build
  --sign          Sign app bundle using configured identity
  --app-dir PATH  Override output app bundle path
  -h, --help      Show help

Environment:
  NEOMIST_APP_NAME          App bundle name (default: NeoMist)
  NEOMIST_EXECUTABLE_NAME   Binary name inside app (default: neomist)
  NEOMIST_BUNDLE_ID         Bundle identifier (default: eth.neomist.app)
  NEOMIST_PROFILE           Cargo profile (default: release)
  NEOMIST_ICON_SOURCE       PNG source for generated AppIcon.icns
  NEOMIST_APP_SIGN_IDENTITY Optional Developer ID Application identity
  NEOMIST_APP_SIGN_TIMESTAMP Add codesign timestamp when set to 1 (default: 1)
  NEOMIST_APP_HARDENED_RUNTIME Enable hardened runtime when set to 1 (default: 1)
  NEOMIST_APP_ENTITLEMENTS  Optional entitlements plist for app executable
  NEOMIST_ENV_FILE          Optional alternate env file path (default: .env)
EOF
}

resolve_binary_version() {
    local binary_path=$1
    local version_output

    if [[ ! -x "$binary_path" ]]; then
        printf 'Missing executable for version check: %s\n' "$binary_path" >&2
        exit 1
    fi

    version_output="$("$binary_path" --version 2>/dev/null || true)"
    if [[ -z "$version_output" ]]; then
        printf 'Failed to read binary version from: %s\n' "$binary_path" >&2
        exit 1
    fi

    printf '%s\n' "${version_output##* }"
}

validate_binary_version() {
    local binary_path=$1
    local expected_version=$2
    local actual_version

    actual_version="$(resolve_binary_version "$binary_path")"
    if [[ "$actual_version" != "$expected_version" ]]; then
        printf 'Binary version mismatch: expected %s, got %s from %s\n' \
            "$expected_version" "$actual_version" "$binary_path" >&2
        exit 1
    fi
}

codesign_args() {
    local -n args_ref=$1
    args_ref=(--force --sign "$APP_SIGN_IDENTITY")
    if [[ "$APP_HARDENED_RUNTIME" == "1" ]]; then
        args_ref+=(--options runtime)
    fi
    if [[ "$APP_SIGN_TIMESTAMP" == "1" ]]; then
        args_ref+=(--timestamp)
    fi
}

sign_app_bundle() {
    local executable_path=$1
    local app_path=$2
    local -a exec_args bundle_args

    codesign_args exec_args
    if [[ -n "$APP_ENTITLEMENTS" ]]; then
        exec_args+=(--entitlements "$APP_ENTITLEMENTS")
    fi
    codesign "${exec_args[@]}" "$executable_path"

    codesign_args bundle_args
    codesign "${bundle_args[@]}" "$app_path"
    codesign --verify --strict --verbose=2 "$app_path" >/dev/null
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-build)
            BUILD_BINARY=0
            ;;
        --sign)
            SIGN_APP=1
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

if [[ -n "$APP_ENTITLEMENTS" && ! -f "$APP_ENTITLEMENTS" ]]; then
    printf 'Missing app entitlements file: %s\n' "$APP_ENTITLEMENTS" >&2
    exit 1
fi

if [[ "$SIGN_APP" == "1" && -z "$APP_SIGN_IDENTITY" ]]; then
    printf 'App signing requested but NEOMIST_APP_SIGN_IDENTITY is not set.\n' >&2
    exit 1
fi

version="$({ awk -F '"' '/^version = "/ { print $2; exit }' "${ROOT_DIR}/Cargo.toml"; } || true)"
if [[ -z "$version" ]]; then
    printf 'Failed to resolve app version from Cargo.toml.\n' >&2
    exit 1
fi

ARCH="$(uname -m)"
if [[ -z "$APP_DIR" ]]; then
    APP_DIR="${ROOT_DIR}/dist/neomist-${version}-macos-${ARCH}.app"
fi

if [[ "$BUILD_BINARY" -eq 1 ]]; then
    cargo build --manifest-path "${ROOT_DIR}/Cargo.toml" --profile "$PROFILE"
fi

BINARY_PATH="${TARGET_DIR}/${PROFILE}/${EXECUTABLE_NAME}"
if [[ ! -x "$BINARY_PATH" ]]; then
    printf 'Missing built binary: %s\n' "$BINARY_PATH" >&2
    exit 1
fi

validate_binary_version "$BINARY_PATH" "$version"

CONTENTS_DIR="${APP_DIR}/Contents"
MACOS_DIR="${CONTENTS_DIR}/MacOS"
RESOURCES_DIR="${CONTENTS_DIR}/Resources"
INFO_PLIST_PATH="${CONTENTS_DIR}/Info.plist"

mkdir -p "$(dirname "$APP_DIR")"
if [[ -e "$APP_DIR" ]]; then
    if ! rm -rf "$APP_DIR"; then
        printf 'Failed to replace existing app bundle: %s\n' "$APP_DIR" >&2
        printf 'Existing bundle is likely owned by another user. Fix ownership or remove it manually, then rerun.\n' >&2
        printf 'Suggested fix: sudo chown -R %s "%s"\n' "$(id -un)" "$APP_DIR" >&2
        exit 1
    fi
fi
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

if [[ "$SIGN_APP" == "1" ]]; then
    if ! command -v codesign >/dev/null 2>&1; then
        printf 'codesign not found. Install Xcode command line tools.\n' >&2
        exit 1
    fi
    sign_app_bundle "${MACOS_DIR}/${EXECUTABLE_NAME}" "$APP_DIR"
    printf 'Signed app bundle: %s\n' "$APP_DIR"
fi

printf 'Built app bundle: %s\n' "$APP_DIR"
