#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/load-dotenv.sh
. "${ROOT_DIR}/scripts/load-dotenv.sh"

APP_NAME="${NEOMIST_APP_NAME:-NeoMist}"
DIST_DIR="${ROOT_DIR}/dist"
NOTARY_PROFILE="${NEOMIST_NOTARY_PROFILE:-}"
NOTARY_KEYCHAIN="${NEOMIST_NOTARY_KEYCHAIN:-}"
SUBMISSION_TIMEOUT="${NEOMIST_NOTARY_TIMEOUT:-20m}"
PKG_PATH=""

usage() {
    cat <<'EOF'
Notarize and staple signed macOS installer package.

Usage:
  scripts/notarize-macos-pkg.sh [--pkg-path PATH]

Options:
  --pkg-path PATH  Override package path to notarize
  -h, --help       Show help

Environment:
  NEOMIST_APP_NAME       App name used for default pkg path lookup
  NEOMIST_NOTARY_PROFILE notarytool keychain profile name (required)
  NEOMIST_NOTARY_KEYCHAIN Optional custom keychain path for profile lookup
  NEOMIST_NOTARY_TIMEOUT Wait timeout for notarization (default: 20m)
  NEOMIST_ENV_FILE       Optional alternate env file path (default: .env)
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --pkg-path)
            if [[ $# -lt 2 ]]; then
                printf 'Missing value for %s\n' "$1" >&2
                exit 1
            fi
            PKG_PATH="$2"
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

if ! command -v xcrun >/dev/null 2>&1; then
    printf 'xcrun not found. Install Xcode command line tools.\n' >&2
    exit 1
fi

if [[ -z "$NOTARY_PROFILE" ]]; then
    printf 'NEOMIST_NOTARY_PROFILE is required.\n' >&2
    exit 1
fi

version="$({ awk -F '"' '/^version = "/ { print $2; exit }' "${ROOT_DIR}/Cargo.toml"; } || true)"
if [[ -z "$version" ]]; then
    printf 'Failed to resolve app version from Cargo.toml.\n' >&2
    exit 1
fi

ARCH="$(uname -m)"
if [[ -z "$PKG_PATH" ]]; then
    PKG_PATH="${DIST_DIR}/${APP_NAME}-${version}-${ARCH}.pkg"
fi

if [[ ! -f "$PKG_PATH" ]]; then
    printf 'Missing pkg artifact: %s\n' "$PKG_PATH" >&2
    exit 1
fi

NOTARY_ARGS=(notarytool submit "$PKG_PATH" --keychain-profile "$NOTARY_PROFILE" --wait --timeout "$SUBMISSION_TIMEOUT")
if [[ -n "$NOTARY_KEYCHAIN" ]]; then
    NOTARY_ARGS+=(--keychain "$NOTARY_KEYCHAIN")
fi

printf 'Submitting pkg for notarization: %s\n' "$PKG_PATH"
xcrun "${NOTARY_ARGS[@]}"

printf 'Stapling notarization ticket: %s\n' "$PKG_PATH"
xcrun stapler staple "$PKG_PATH"

printf 'Validating stapled ticket: %s\n' "$PKG_PATH"
xcrun stapler validate "$PKG_PATH"

printf 'Notarized pkg artifact: %s\n' "$PKG_PATH"
