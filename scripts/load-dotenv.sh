#!/usr/bin/env bash

if [[ -z "${ROOT_DIR:-}" ]]; then
    ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fi

ENV_FILE="${NEOMIST_ENV_FILE:-${ROOT_DIR}/.env}"

if [[ -f "$ENV_FILE" ]]; then
    set -a
    # shellcheck disable=SC1090
    . "$ENV_FILE"
    set +a
fi
