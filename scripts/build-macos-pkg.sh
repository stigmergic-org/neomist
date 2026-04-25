#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/load-dotenv.sh
. "${ROOT_DIR}/scripts/load-dotenv.sh"

APP_NAME="${NEOMIST_APP_NAME:-NeoMist}"
EXECUTABLE_NAME="${NEOMIST_EXECUTABLE_NAME:-neomist}"
PKG_IDENTIFIER="${NEOMIST_PKG_IDENTIFIER:-org.neomist.pkg}"
COMPONENT_PKG_IDENTIFIER="${NEOMIST_COMPONENT_PKG_IDENTIFIER:-${PKG_IDENTIFIER}.component}"
PROFILE="${NEOMIST_PROFILE:-release}"
DIST_DIR="${ROOT_DIR}/dist"
SCRIPT_TEMPLATE="${ROOT_DIR}/packaging/macos/pkg-postinstall.template"
PREINSTALL_TEMPLATE="${ROOT_DIR}/packaging/macos/pkg-preinstall.template"
DIST_TEMPLATE="${ROOT_DIR}/packaging/macos/product-distribution.xml.template"
RESOURCE_TEMPLATE_DIR="${ROOT_DIR}/packaging/macos/installer-resources"
SIGN_IDENTITY="${NEOMIST_INSTALLER_SIGN_IDENTITY:-}"
APP_SIGN_IDENTITY="${NEOMIST_APP_SIGN_IDENTITY:-}"
APP_SIGN_TIMESTAMP="${NEOMIST_APP_SIGN_TIMESTAMP:-1}"
APP_HARDENED_RUNTIME="${NEOMIST_APP_HARDENED_RUNTIME:-1}"
APP_ENTITLEMENTS="${NEOMIST_APP_ENTITLEMENTS:-}"
BUILD_APP=1
SIGN_PKG=0
OUTPUT_PKG=""

artifact_app_path() {
    local version=$1
    local arch=$2
    printf '%s/dist/neomist-%s-macos-%s.app' "$ROOT_DIR" "$version" "$arch"
}

stage_app_bundle() {
    local source_app_path=$1
    local staged_app_path=$2

    rm -rf "$staged_app_path"
    cp -R "$source_app_path" "$staged_app_path"
}

write_component_plist() {
    local output_path=$1

    cat > "$output_path" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<array>
  <dict>
    <key>RootRelativeBundlePath</key>
    <string>Applications/${APP_NAME}.app</string>
    <key>BundleIsRelocatable</key>
    <false/>
    <key>BundleIsVersionChecked</key>
    <false/>
    <key>BundleHasStrictIdentifier</key>
    <false/>
    <key>BundleOverwriteAction</key>
    <string>upgrade</string>
  </dict>
</array>
</plist>
EOF
}

usage() {
    cat <<'EOF'
Build macOS installer package.

Usage:
  scripts/build-macos-pkg.sh [--skip-build] [--sign] [--pkg-path PATH]

Options:
  --skip-build    Reuse existing app bundle in dist/
  --sign          Sign app and installer package using configured identities
  --pkg-path PATH Override output package path
  -h, --help      Show help

Environment:
  NEOMIST_APP_NAME                 App bundle name (default: NeoMist)
  NEOMIST_EXECUTABLE_NAME          Binary name inside app (default: neomist)
  NEOMIST_PKG_IDENTIFIER           Installer package identifier
  NEOMIST_COMPONENT_PKG_IDENTIFIER Component package identifier
  NEOMIST_PROFILE                  Cargo profile used by app builder
  NEOMIST_APP_SIGN_IDENTITY        Optional Developer ID Application identity
  NEOMIST_INSTALLER_SIGN_IDENTITY  Optional pkg signing identity
  NEOMIST_ENV_FILE                 Optional alternate env file path (default: .env)
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

validate_app_bundle_version() {
    local app_path=$1
    local expected_version=$2
    local binary_path="${app_path}/Contents/MacOS/${EXECUTABLE_NAME}"
    local actual_version

    actual_version="$(resolve_binary_version "$binary_path")"
    if [[ "$actual_version" != "$expected_version" ]]; then
        printf 'App bundle version mismatch: expected %s, got %s from %s\n' \
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
    local app_path=$1
    local executable_path="${app_path}/Contents/MacOS/${EXECUTABLE_NAME}"
    local -a exec_args bundle_args

    if [[ ! -x "$executable_path" ]]; then
        printf 'Missing app executable for signing: %s\n' "$executable_path" >&2
        exit 1
    fi

    codesign_args exec_args
    if [[ -n "$APP_ENTITLEMENTS" ]]; then
        exec_args+=(--entitlements "$APP_ENTITLEMENTS")
    fi
    codesign "${exec_args[@]}" "$executable_path"

    codesign_args bundle_args
    codesign "${bundle_args[@]}" "$app_path"
    codesign --verify --strict --verbose=2 "$app_path" >/dev/null
}

app_bundle_is_signed() {
    local app_path=$1
    codesign --verify --strict --verbose=2 "$app_path" >/dev/null 2>&1
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-build)
            BUILD_APP=0
            ;;
        --sign)
            SIGN_PKG=1
            ;;
        --pkg-path)
            if [[ $# -lt 2 ]]; then
                printf 'Missing value for %s\n' "$1" >&2
                exit 1
            fi
            OUTPUT_PKG="$2"
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

if ! command -v pkgbuild >/dev/null 2>&1; then
    printf 'pkgbuild not found. Install Xcode command line tools.\n' >&2
    exit 1
fi

if ! command -v productbuild >/dev/null 2>&1; then
    printf 'productbuild not found. Install Xcode command line tools.\n' >&2
    exit 1
fi

if [[ -n "$APP_ENTITLEMENTS" && ! -f "$APP_ENTITLEMENTS" ]]; then
    printf 'Missing app entitlements file: %s\n' "$APP_ENTITLEMENTS" >&2
    exit 1
fi

if [[ "$SIGN_PKG" == "1" && -z "$SIGN_IDENTITY" ]]; then
    printf 'Pkg signing requested but NEOMIST_INSTALLER_SIGN_IDENTITY is not set.\n' >&2
    exit 1
fi

if [[ ! -f "$SCRIPT_TEMPLATE" || ! -f "$PREINSTALL_TEMPLATE" || ! -f "$DIST_TEMPLATE" ]]; then
    printf 'Missing pkg packaging template.\n' >&2
    exit 1
fi

if [[ ! -d "$RESOURCE_TEMPLATE_DIR" ]]; then
    printf 'Missing installer resource templates: %s\n' "$RESOURCE_TEMPLATE_DIR" >&2
    exit 1
fi

version="$({ awk -F '"' '/^version = "/ { print $2; exit }' "${ROOT_DIR}/Cargo.toml"; } || true)"
if [[ -z "$version" ]]; then
    printf 'Failed to resolve app version from Cargo.toml.\n' >&2
    exit 1
fi

ARCH="$(uname -m)"
APP_PATH="$(artifact_app_path "$version" "$ARCH")"
if [[ -z "$OUTPUT_PKG" ]]; then
    OUTPUT_PKG="${DIST_DIR}/neomist-${version}-macos-${ARCH}.pkg"
fi

SCRIPT_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/neomist-pkg-scripts.XXXXXX")"
BUILD_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/neomist-pkg-build.XXXXXX")"
RESOURCE_ROOT="${BUILD_ROOT}/resources"
PAYLOAD_ROOT="${BUILD_ROOT}/payload"
STAGED_APP_PATH="${BUILD_ROOT}/${APP_NAME}.app"
COMPONENT_PKG_PATH="${BUILD_ROOT}/${APP_NAME}-component.pkg"
DIST_PATH="${BUILD_ROOT}/distribution.xml"
COMPONENT_PLIST_PATH="${BUILD_ROOT}/components.plist"

cleanup() {
    rm -rf "$SCRIPT_ROOT" "$BUILD_ROOT"
}
trap cleanup EXIT

if [[ "$BUILD_APP" -eq 1 ]]; then
    if [[ "$SIGN_PKG" == "1" ]]; then
        "${ROOT_DIR}/scripts/build-macos-app.sh" --sign --app-dir "$STAGED_APP_PATH"
    else
        "${ROOT_DIR}/scripts/build-macos-app.sh" --app-dir "$STAGED_APP_PATH"
    fi
else
    if [[ ! -d "$APP_PATH" ]]; then
        printf 'Missing app bundle: %s\n' "$APP_PATH" >&2
        exit 1
    fi
    stage_app_bundle "$APP_PATH" "$STAGED_APP_PATH"
fi

validate_app_bundle_version "$STAGED_APP_PATH" "$version"

mkdir -p "$(dirname "$OUTPUT_PKG")"

PAYLOAD_APP_PATH="$STAGED_APP_PATH"
if [[ "$SIGN_PKG" == "1" && -n "$APP_SIGN_IDENTITY" ]]; then
    if ! command -v codesign >/dev/null 2>&1; then
        printf 'codesign not found. Install Xcode command line tools.\n' >&2
        exit 1
    fi
    if app_bundle_is_signed "$PAYLOAD_APP_PATH"; then
        printf 'Using existing signed app bundle: %s\n' "$PAYLOAD_APP_PATH"
    else
        sign_app_bundle "$PAYLOAD_APP_PATH"
        printf 'Signed payload app bundle: %s\n' "$PAYLOAD_APP_PATH"
    fi
elif [[ "$SIGN_PKG" == "1" ]]; then
    if ! app_bundle_is_signed "$PAYLOAD_APP_PATH"; then
        printf 'Signed pkg requires signed app. Set NEOMIST_APP_SIGN_IDENTITY or provide pre-signed app.\n' >&2
        exit 1
    fi
fi

mkdir -p "${PAYLOAD_ROOT}/Applications"
stage_app_bundle "$PAYLOAD_APP_PATH" "${PAYLOAD_ROOT}/Applications/${APP_NAME}.app"
write_component_plist "$COMPONENT_PLIST_PATH"

sed \
    -e "s|__APP_NAME__|${APP_NAME}|g" \
    "$PREINSTALL_TEMPLATE" > "${SCRIPT_ROOT}/preinstall"
chmod 755 "${SCRIPT_ROOT}/preinstall"

sed \
    -e "s|__APP_NAME__|${APP_NAME}|g" \
    -e "s|__EXECUTABLE_NAME__|${EXECUTABLE_NAME}|g" \
    "$SCRIPT_TEMPLATE" > "${SCRIPT_ROOT}/postinstall"
chmod 755 "${SCRIPT_ROOT}/postinstall"

mkdir -p "${RESOURCE_ROOT}/English.lproj"
for template in "${RESOURCE_TEMPLATE_DIR}/English.lproj"/*.html.template; do
    output_name="$(basename "$template" .template)"
    sed \
        -e "s|__APP_NAME__|${APP_NAME}|g" \
        "$template" > "${RESOURCE_ROOT}/English.lproj/${output_name}"
done

sed \
    -e "s|__APP_NAME__|${APP_NAME}|g" \
    -e "s|__APP_VERSION__|${version}|g" \
    -e "s|__PKG_IDENTIFIER__|${COMPONENT_PKG_IDENTIFIER}|g" \
    -e "s|__COMPONENT_PKG_FILENAME__|$(basename "$COMPONENT_PKG_PATH")|g" \
    "$DIST_TEMPLATE" > "$DIST_PATH"

PKGBUILD_ARGS=(
    --root "$PAYLOAD_ROOT"
    --component-plist "$COMPONENT_PLIST_PATH"
    --identifier "$COMPONENT_PKG_IDENTIFIER"
    --version "$version"
    --install-location "/"
    --scripts "$SCRIPT_ROOT"
)

rm -f "$OUTPUT_PKG"
pkgbuild "${PKGBUILD_ARGS[@]}" "$COMPONENT_PKG_PATH" >/dev/null

PRODUCTBUILD_ARGS=(
    --distribution "$DIST_PATH"
    --package-path "$BUILD_ROOT"
    --resources "$RESOURCE_ROOT"
    --identifier "$PKG_IDENTIFIER"
    --version "$version"
)

if [[ "$SIGN_PKG" == "1" ]]; then
    PRODUCTBUILD_ARGS+=(--sign "$SIGN_IDENTITY")
fi

productbuild "${PRODUCTBUILD_ARGS[@]}" "$OUTPUT_PKG" >/dev/null

printf 'Built pkg artifact: %s\n' "$OUTPUT_PKG"
