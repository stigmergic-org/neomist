#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="${NEOMIST_APP_NAME:-NeoMist}"
EXECUTABLE_NAME="${NEOMIST_EXECUTABLE_NAME:-neomist}"
APP_DIR_NAME="${NEOMIST_APP_DIR_NAME:-neomist}"
PKG_IDENTIFIER="${NEOMIST_PKG_IDENTIFIER:-org.neomist.pkg}"
COMPONENT_PKG_IDENTIFIER="${NEOMIST_COMPONENT_PKG_IDENTIFIER:-${PKG_IDENTIFIER}.component}"
PROFILE="${NEOMIST_PROFILE:-release}"
DIST_DIR="${ROOT_DIR}/dist"
SCRIPT_TEMPLATE="${ROOT_DIR}/packaging/macos/pkg-postinstall.template"
DIST_TEMPLATE="${ROOT_DIR}/packaging/macos/product-distribution.xml.template"
RESOURCE_TEMPLATE_DIR="${ROOT_DIR}/packaging/macos/installer-resources"
SIGN_IDENTITY="${NEOMIST_INSTALLER_SIGN_IDENTITY:-}"
BUILD_APP=1
OUTPUT_PKG=""

usage() {
    cat <<'EOF'
Build macOS installer package.

Usage:
  scripts/build-macos-pkg.sh [--skip-build] [--pkg-path PATH]

Options:
  --skip-build    Reuse existing app bundle in dist/
  --pkg-path PATH Override output package path
  -h, --help      Show help

Environment:
  NEOMIST_APP_NAME                 App bundle name (default: NeoMist)
  NEOMIST_EXECUTABLE_NAME          Binary name inside app (default: neomist)
  NEOMIST_APP_DIR_NAME             User data dir name (default: neomist)
  NEOMIST_PKG_IDENTIFIER           Installer package identifier
  NEOMIST_COMPONENT_PKG_IDENTIFIER Component package identifier
  NEOMIST_PROFILE                  Cargo profile used by app builder
  NEOMIST_INSTALLER_SIGN_IDENTITY  Optional pkg signing identity
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-build)
            BUILD_APP=0
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

if [[ ! -f "$SCRIPT_TEMPLATE" || ! -f "$DIST_TEMPLATE" ]]; then
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
APP_PATH="${DIST_DIR}/${APP_NAME}.app"
if [[ -z "$OUTPUT_PKG" ]]; then
    OUTPUT_PKG="${DIST_DIR}/${APP_NAME}-${version}-${ARCH}.pkg"
fi

PAYLOAD_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/neomist-pkg-root.XXXXXX")"
SCRIPT_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/neomist-pkg-scripts.XXXXXX")"
BUILD_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/neomist-pkg-build.XXXXXX")"
RESOURCE_ROOT="${BUILD_ROOT}/resources"
COMPONENT_PKG_PATH="${BUILD_ROOT}/${APP_NAME}-component.pkg"
DIST_PATH="${BUILD_ROOT}/distribution.xml"

cleanup() {
    rm -rf "$PAYLOAD_ROOT" "$SCRIPT_ROOT" "$BUILD_ROOT"
}
trap cleanup EXIT

if [[ "$BUILD_APP" -eq 1 ]]; then
    "${ROOT_DIR}/scripts/build-macos-app.sh"
fi

if [[ ! -d "$APP_PATH" ]]; then
    printf 'Missing app bundle: %s\n' "$APP_PATH" >&2
    exit 1
fi

mkdir -p "${PAYLOAD_ROOT}/Applications" "$(dirname "$OUTPUT_PKG")"
cp -R "$APP_PATH" "${PAYLOAD_ROOT}/Applications/${APP_NAME}.app"

sed \
    -e "s|__APP_NAME__|${APP_NAME}|g" \
    -e "s|__EXECUTABLE_NAME__|${EXECUTABLE_NAME}|g" \
    -e "s|__APP_DIR_NAME__|${APP_DIR_NAME}|g" \
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

if [[ -n "$SIGN_IDENTITY" ]]; then
    PRODUCTBUILD_ARGS+=(--sign "$SIGN_IDENTITY")
fi

productbuild "${PRODUCTBUILD_ARGS[@]}" "$OUTPUT_PKG" >/dev/null

printf 'Built pkg artifact: %s\n' "$OUTPUT_PKG"
