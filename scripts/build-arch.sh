#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PACKAGE_NAME="${NEOMIST_ARCH_PACKAGE_NAME:-neomist}"
OUTPUT_DIR="${NEOMIST_OUTPUT_DIR:-${ROOT_DIR}/dist}"
WORK_DIR="${NEOMIST_ARCH_WORK_DIR:-${ROOT_DIR}/dist/arch}"
PKGBUILD_SOURCE="${ROOT_DIR}/packaging/arch/PKGBUILD"
BUILD_LOCAL=0
SYNC_DEPS=1

usage() {
    cat <<'EOF'
Build NeoMist Arch Linux package.

Usage:
  scripts/build-arch.sh [--local] [--skip-syncdeps]

Options:
  --local          Build from the current checkout instead of the release source in PKGBUILD
  --skip-syncdeps  Do not pass --syncdeps to makepkg
  -h, --help      Show help

Environment:
  NEOMIST_ARCH_PACKAGE_NAME  Arch package name (default: neomist)
  NEOMIST_OUTPUT_DIR         Output directory (default: dist)
  NEOMIST_ARCH_WORK_DIR      Working directory (default: dist/arch)
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --local)
            BUILD_LOCAL=1
            ;;
        --skip-syncdeps)
            SYNC_DEPS=0
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

for command_name in awk makepkg sed sha256sum tar; do
    if ! command -v "$command_name" >/dev/null 2>&1; then
        printf 'Missing required command: %s\n' "$command_name" >&2
        exit 1
    fi
done

if [[ ! -f "$PKGBUILD_SOURCE" ]]; then
    printf 'Missing PKGBUILD: %s\n' "$PKGBUILD_SOURCE" >&2
    exit 1
fi

version="$({ awk -F '"' '/^version = "/ { print $2; exit }' "${ROOT_DIR}/Cargo.toml"; } || true)"
if [[ -z "$version" ]]; then
    printf 'Failed to resolve package version from Cargo.toml.\n' >&2
    exit 1
fi

detect_arch() {
    case "$(uname -m)" in
        x86_64|amd64)
            printf 'x86_64'
            ;;
        *)
            printf 'Unsupported architecture for Arch package build: %s\n' "$(uname -m)" >&2
            exit 1
            ;;
    esac
}

archive_current_checkout() {
    local output_path=$1
    local prefix="${PACKAGE_NAME}-${version}/"

    if ! command -v git >/dev/null 2>&1; then
        printf 'Missing required command for --local: git\n' >&2
        exit 1
    fi

    (
        cd "$ROOT_DIR"
        git ls-files -z --cached --others --exclude-standard |
            tar --null --files-from - --transform "s#^#${prefix}#" -czf "$output_path"
    )
}

prepare_pkgbuild() {
    local build_dir=$1
    local pkgbuild_path="${build_dir}/PKGBUILD"

    mkdir -p "$build_dir"
    cp "$PKGBUILD_SOURCE" "$pkgbuild_path"
    sed -i -e "s/^pkgver=.*/pkgver=${version}/" "$pkgbuild_path"

    if [[ "$BUILD_LOCAL" -eq 1 ]]; then
        local source_name="${PACKAGE_NAME}-${version}.tar.gz"
        local source_path="${build_dir}/${source_name}"
        archive_current_checkout "$source_path"
        cp "$source_path" "${SRCDEST_DIR}/${source_name}"
        local checksum
        checksum="$(sha256sum "$source_path" | awk '{ print $1 }')"

        sed -i \
            -e "s#^source=.*#source=('${source_name}')#" \
            -e "s/^sha256sums=.*/sha256sums=('${checksum}')/" \
            "$pkgbuild_path"
    fi
}

arch="$(detect_arch)"
BUILD_DIR="${WORK_DIR}/makepkg"
PKGDEST_DIR="${OUTPUT_DIR}"
SRCDEST_DIR="${WORK_DIR}/sources"
BUILDDIR="${WORK_DIR}/build"
release_pkg="${OUTPUT_DIR}/${PACKAGE_NAME}-${version}-linux-${arch}.pkg.tar.zst"

rm -rf "$BUILD_DIR" "${BUILDDIR}/${PACKAGE_NAME}"
mkdir -p "$BUILD_DIR" "$PKGDEST_DIR" "$SRCDEST_DIR" "$BUILDDIR"
prepare_pkgbuild "$BUILD_DIR"

makepkg_args=(--clean --cleanbuild --force --noconfirm)
if [[ "$SYNC_DEPS" -eq 1 ]]; then
    makepkg_args+=(--syncdeps)
fi

(
    cd "$BUILD_DIR"
    PKGDEST="$PKGDEST_DIR" SRCDEST="$SRCDEST_DIR" BUILDDIR="$BUILDDIR" makepkg "${makepkg_args[@]}"
)

shopt -s nullglob
built_packages=("${OUTPUT_DIR}/${PACKAGE_NAME}-${version}-"*-"${arch}.pkg.tar.zst")
shopt -u nullglob

if [[ "${#built_packages[@]}" -eq 0 ]]; then
    printf 'Arch package build finished, but no package artifact was found.\n' >&2
    exit 1
fi

cp "${built_packages[0]}" "$release_pkg"
printf 'Built Arch package: %s\n' "$release_pkg"
