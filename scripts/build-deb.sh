#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PACKAGE_NAME="${NEOMIST_DEB_PACKAGE_NAME:-neomist}"
EXECUTABLE_NAME="${NEOMIST_EXECUTABLE_NAME:-neomist}"
APP_ID="${NEOMIST_APP_ID:-eth.neomist.app}"
PROFILE="${NEOMIST_PROFILE:-release}"
TARGET_DIR="${CARGO_TARGET_DIR:-${ROOT_DIR}/target}"
OUTPUT_DIR="${NEOMIST_OUTPUT_DIR:-${ROOT_DIR}/dist}"
PACKAGE_ROOT="${NEOMIST_DEB_ROOT:-${ROOT_DIR}/dist/deb/pkgroot}"
MAINTAINER="${NEOMIST_DEB_MAINTAINER:-NeoMist <packaging@localhost>}"
HOMEPAGE="${NEOMIST_DEB_HOMEPAGE:-https://neomist.eth.link}"
BUILD_BINARY=1

release_arch() {
    case "$1" in
        amd64)
            printf 'x86_64'
            ;;
        arm64)
            printf 'arm64'
            ;;
        *)
            printf '%s' "$1"
            ;;
    esac
}

usage() {
    cat <<'EOF'
Build NeoMist Debian package.

Usage:
  scripts/build-deb.sh [--skip-build]

Options:
  --skip-build  Reuse existing release binary and UI build artifacts
  -h, --help    Show help

Environment:
  NEOMIST_DEB_PACKAGE_NAME  Debian package name (default: neomist)
  NEOMIST_EXECUTABLE_NAME   Cargo binary name (default: neomist)
  NEOMIST_APP_ID            Desktop/metainfo ID (default: eth.neomist.app)
  NEOMIST_PROFILE           Cargo profile (default: release)
  NEOMIST_OUTPUT_DIR        Output directory (default: dist)
  NEOMIST_DEB_ROOT          Package staging directory (default: dist/deb/pkgroot)
  NEOMIST_DEB_MAINTAINER    Debian Maintainer field
  NEOMIST_DEB_HOMEPAGE      Debian Homepage field
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-build)
            BUILD_BINARY=0
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

for command_name in awk cargo npm dpkg-deb dpkg-shlibdeps dpkg; do
    if ! command -v "$command_name" >/dev/null 2>&1; then
        printf 'Missing required command: %s\n' "$command_name" >&2
        exit 1
    fi
done

DESKTOP_FILE="${ROOT_DIR}/packaging/linux/${APP_ID}.desktop"
METAINFO_FILE="${ROOT_DIR}/packaging/linux/${APP_ID}.metainfo.xml"
ICON_SVG="${ROOT_DIR}/assets/icon.svg"
ICON_PNG="${ROOT_DIR}/assets/icon.png"

for required_path in "$DESKTOP_FILE" "$METAINFO_FILE" "$ICON_SVG" "$ICON_PNG"; do
    if [[ ! -f "$required_path" ]]; then
        printf 'Missing required packaging file: %s\n' "$required_path" >&2
        exit 1
    fi
done

version="$({ awk -F '"' '/^version = "/ { print $2; exit }' "${ROOT_DIR}/Cargo.toml"; } || true)"
if [[ -z "$version" ]]; then
    printf 'Failed to resolve package version from Cargo.toml.\n' >&2
    exit 1
fi

if [[ "$BUILD_BINARY" -eq 1 ]]; then
    npm ci --prefix "${ROOT_DIR}/ui"
    npm run --prefix "${ROOT_DIR}/ui" build
    cargo build --manifest-path "${ROOT_DIR}/Cargo.toml" --profile "$PROFILE" --locked
fi

BINARY_PATH="${TARGET_DIR}/${PROFILE}/${EXECUTABLE_NAME}"
if [[ ! -x "$BINARY_PATH" ]]; then
    printf 'Missing built binary: %s\n' "$BINARY_PATH" >&2
    exit 1
fi

architecture="$(dpkg --print-architecture)"
artifact_arch="$(release_arch "$architecture")"
deb_output_path="${OUTPUT_DIR}/${PACKAGE_NAME}-${version}-linux-${artifact_arch}.deb"

rm -rf "$PACKAGE_ROOT"
mkdir -p "$PACKAGE_ROOT/DEBIAN"
install -Dm0755 "$BINARY_PATH" "$PACKAGE_ROOT/usr/bin/${EXECUTABLE_NAME}"
install -Dm0644 "$DESKTOP_FILE" "$PACKAGE_ROOT/usr/share/applications/${APP_ID}.desktop"
install -Dm0644 "$METAINFO_FILE" "$PACKAGE_ROOT/usr/share/metainfo/${APP_ID}.metainfo.xml"
install -Dm0644 "$ICON_SVG" "$PACKAGE_ROOT/usr/share/icons/hicolor/scalable/apps/${APP_ID}.svg"
install -Dm0644 "$ICON_PNG" "$PACKAGE_ROOT/usr/share/icons/hicolor/512x512/apps/${APP_ID}.png"

shlibdeps_workdir="$(mktemp -d "${TMPDIR:-/tmp}/neomist-shlibdeps.XXXXXX")"
mkdir -p "$shlibdeps_workdir/debian"
cat > "$shlibdeps_workdir/debian/control" <<EOF
Source: ${PACKAGE_NAME}
Section: net
Priority: optional
Maintainer: ${MAINTAINER}
Standards-Version: 4.7.0

Package: ${PACKAGE_NAME}
Architecture: ${architecture}
Description: temporary control file for shlibdeps
 temporary
EOF

shlibs_output="$(
    cd "$shlibdeps_workdir" &&
    dpkg-shlibdeps -O "$PACKAGE_ROOT/usr/bin/${EXECUTABLE_NAME}"
)"
rm -rf "$shlibdeps_workdir"
shlibs_depends="${shlibs_output#shlibs:Depends=}"

depends_parts=("libayatana-appindicator3-1" "policykit-1" "libcap2-bin" "ca-certificates" "xdg-utils" "systemd-resolved")
if [[ -n "$shlibs_depends" ]]; then
    depends_parts=("$shlibs_depends" "${depends_parts[@]}")
fi

depends_field="$(printf '%s, ' "${depends_parts[@]}")"
depends_field="${depends_field%, }"

read -r installed_size _ < <(du -sk "$PACKAGE_ROOT")

cat > "$PACKAGE_ROOT/DEBIAN/control" <<EOF
Package: ${PACKAGE_NAME}
Version: ${version}
Section: net
Priority: optional
Architecture: ${architecture}
Maintainer: ${MAINTAINER}
Installed-Size: ${installed_size}
Depends: ${depends_field}
Homepage: ${HOMEPAGE}
Description: Local-first desktop app for .eth and .wei sites
 NeoMist runs local DNS, HTTPS, Ethereum light-client, and IPFS services
 so users can browse .eth and .wei sites without centralized gateways.
 .
 Package installs app in stable system path so NeoMist's existing Linux
 DNS, CA trust, autostart, and privileged-port setup can run natively.
EOF

mkdir -p "$OUTPUT_DIR"
dpkg-deb --build --root-owner-group "$PACKAGE_ROOT" "$deb_output_path"

printf 'Built Debian package: %s\n' "$deb_output_path"
