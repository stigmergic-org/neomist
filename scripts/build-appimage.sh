#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="${NEOMIST_APP_NAME:-NeoMist}"
EXECUTABLE_NAME="${NEOMIST_EXECUTABLE_NAME:-neomist}"
PROFILE="${NEOMIST_PROFILE:-release}"
TARGET_DIR="${CARGO_TARGET_DIR:-${ROOT_DIR}/target}"
APPDIR_PATH="${NEOMIST_APPDIR_PATH:-${ROOT_DIR}/dist/AppDir}"
OUTPUT_DIR="${NEOMIST_OUTPUT_DIR:-${ROOT_DIR}/dist}"
TOOLS_DIR="${NEOMIST_TOOLS_DIR:-${ROOT_DIR}/dist/tools}"
DESKTOP_FILE="${ROOT_DIR}/packaging/linux/eth.neomist.app.desktop"
METAINFO_FILE="${ROOT_DIR}/packaging/linux/eth.neomist.app.metainfo.xml"
ICON_SVG="${ROOT_DIR}/assets/icon.svg"
ICON_PNG="${ROOT_DIR}/assets/icon.png"
BUILD_BINARY=1

usage() {
    cat <<'EOF'
Build NeoMist AppImage.

Usage:
  scripts/build-appimage.sh [--skip-build]

Options:
  --skip-build  Reuse existing release binary and UI build artifacts
  -h, --help    Show help

Environment:
  NEOMIST_APP_NAME          App name for output naming (default: NeoMist)
  NEOMIST_EXECUTABLE_NAME   Cargo binary name (default: neomist)
  NEOMIST_PROFILE           Cargo profile (default: release)
  NEOMIST_APPDIR_PATH       AppDir output path (default: dist/AppDir)
  NEOMIST_OUTPUT_DIR        AppImage output directory (default: dist)
  NEOMIST_TOOLS_DIR         linuxdeploy tool cache directory (default: dist/tools)
  LINUXDEPLOY               Optional explicit linuxdeploy executable path
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

detect_arch() {
    case "$(uname -m)" in
        x86_64|amd64)
            printf 'x86_64'
            ;;
        aarch64|arm64)
            printf 'aarch64'
            ;;
        i386|i686)
            printf 'i386'
            ;;
        *)
            return 1
            ;;
    esac
}

ensure_linuxdeploy_tooling() {
    mkdir -p "$TOOLS_DIR"

    if [[ -n "${LINUXDEPLOY:-}" ]]; then
        printf '%s' "$LINUXDEPLOY"
        return 0
    fi

    local arch
    arch="$(detect_arch)" || {
        printf 'Unsupported architecture for automatic linuxdeploy download: %s\n' "$(uname -m)" >&2
        printf 'Set LINUXDEPLOY to a custom executable to continue.\n' >&2
        exit 1
    }

    local linuxdeploy_path="${TOOLS_DIR}/linuxdeploy-${arch}.AppImage"
    local appimage_plugin_path="${TOOLS_DIR}/linuxdeploy-plugin-appimage-${arch}.AppImage"
    local gtk_plugin_path="${TOOLS_DIR}/linuxdeploy-plugin-gtk.sh"

    if [[ ! -x "$linuxdeploy_path" ]]; then
        curl -L "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-${arch}.AppImage" -o "$linuxdeploy_path"
        chmod +x "$linuxdeploy_path"
    fi

    if [[ ! -x "$appimage_plugin_path" ]]; then
        curl -L "https://github.com/linuxdeploy/linuxdeploy-plugin-appimage/releases/download/continuous/linuxdeploy-plugin-appimage-${arch}.AppImage" -o "$appimage_plugin_path"
        chmod +x "$appimage_plugin_path"
    fi

    if [[ ! -x "$gtk_plugin_path" ]]; then
        curl -L "https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh" -o "$gtk_plugin_path"
        chmod +x "$gtk_plugin_path"
    fi

    printf '%s' "$linuxdeploy_path"
}

resolve_appindicator_library() {
    local candidates=(
        /usr/lib/libayatana-appindicator3.so.1*
        /usr/lib/*/libayatana-appindicator3.so.1*
        /lib/libayatana-appindicator3.so.1*
        /lib/*/libayatana-appindicator3.so.1*
    )
    local candidate
    for candidate in "${candidates[@]}"; do
        if [[ -f "$candidate" ]]; then
            printf '%s' "$candidate"
            return 0
        fi
    done

    printf 'Missing libayatana-appindicator3 runtime library on build host.\n' >&2
    printf 'Install libayatana-appindicator3 or equivalent first.\n' >&2
    exit 1
}

if [[ ! -f "$DESKTOP_FILE" ]]; then
    printf 'Missing desktop file: %s\n' "$DESKTOP_FILE" >&2
    exit 1
fi

if [[ ! -f "$METAINFO_FILE" ]]; then
    printf 'Missing metainfo file: %s\n' "$METAINFO_FILE" >&2
    exit 1
fi

if [[ ! -f "$ICON_SVG" || ! -f "$ICON_PNG" ]]; then
    printf 'Missing Linux icon assets.\n' >&2
    exit 1
fi

LINUXDEPLOY_BIN="$(ensure_linuxdeploy_tooling)"
export PATH="$(dirname "$LINUXDEPLOY_BIN"):${PATH}"
export APPIMAGE_EXTRACT_AND_RUN=1

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

APPINDICATOR_LIB="$(resolve_appindicator_library)"
APPIMAGE_NAME="${APP_NAME}-$(detect_arch 2>/dev/null || uname -m).AppImage"

rm -rf "$APPDIR_PATH"
mkdir -p "$APPDIR_PATH/usr/bin" "$APPDIR_PATH/usr/share/applications" "$APPDIR_PATH/usr/share/metainfo"
install -Dm0755 "$BINARY_PATH" "$APPDIR_PATH/usr/bin/${EXECUTABLE_NAME}"
install -Dm0644 "$DESKTOP_FILE" "$APPDIR_PATH/usr/share/applications/eth.neomist.app.desktop"
install -Dm0644 "$METAINFO_FILE" "$APPDIR_PATH/usr/share/metainfo/eth.neomist.app.metainfo.xml"
install -Dm0644 "$METAINFO_FILE" "$APPDIR_PATH/usr/share/metainfo/eth.neomist.app.appdata.xml"
install -Dm0644 "$ICON_SVG" "$APPDIR_PATH/usr/share/icons/hicolor/scalable/apps/eth.neomist.app.svg"
install -Dm0644 "$ICON_PNG" "$APPDIR_PATH/usr/share/icons/hicolor/512x512/apps/eth.neomist.app.png"

"$LINUXDEPLOY_BIN" --appdir "$APPDIR_PATH" -e "$BINARY_PATH" -d "$DESKTOP_FILE" -i "$ICON_SVG" -l "$APPINDICATOR_LIB" --plugin gtk

for bundled_lib_dir in "$APPDIR_PATH/usr/lib" "$APPDIR_PATH"/usr/lib/*; do
    if [[ -d "$bundled_lib_dir" && -f "$bundled_lib_dir/$(basename "$APPINDICATOR_LIB")" && ! -e "$bundled_lib_dir/libayatana-appindicator3.so.1" ]]; then
        ln -sf "$(basename "$APPINDICATOR_LIB")" "$bundled_lib_dir/libayatana-appindicator3.so.1"
    fi
done

mkdir -p "$OUTPUT_DIR"
(
    cd "$OUTPUT_DIR"
    OUTPUT="$APPIMAGE_NAME" "$LINUXDEPLOY_BIN" --appdir "$APPDIR_PATH" --output appimage
)

printf 'Built AppImage: %s\n' "$OUTPUT_DIR/$APPIMAGE_NAME"
