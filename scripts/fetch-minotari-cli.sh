#!/usr/bin/env bash
set -euo pipefail

CACHE_DIR="${1:-.bench-cache}"
TOOLS_DIR="${2:-tools}"
MINOTARI_REV="360c4848a54d65fd710266233cc9277b0f785e74"
TARI_CONSOLE_WALLET_REV="9f5adb7183dc2ec285f5c8fae05f4be9735d9749"
MINOTARI_DIR="${CACHE_DIR}/minotari-cli"
TARI_DIR="${CACHE_DIR}/tari"

mkdir -p "${CACHE_DIR}" "${TOOLS_DIR}"

if [ ! -d "${MINOTARI_DIR}/.git" ]; then
  git clone https://github.com/tari-project/minotari-cli.git "${MINOTARI_DIR}"
fi

git -C "${MINOTARI_DIR}" fetch --tags origin
git -C "${MINOTARI_DIR}" checkout "${MINOTARI_REV}"

(
  cd "${MINOTARI_DIR}"
  cargo build --release --bin minotari
)

cp "${MINOTARI_DIR}/target/release/minotari" "${TOOLS_DIR}/minotari"

if [ ! -d "${TARI_DIR}/.git" ]; then
  git clone https://github.com/tari-project/tari.git "${TARI_DIR}"
fi

git -C "${TARI_DIR}" fetch --tags origin
git -C "${TARI_DIR}" checkout "${TARI_CONSOLE_WALLET_REV}"

(
  cd "${TARI_DIR}"
  cargo build --release --bin minotari_console_wallet
)

cp "${TARI_DIR}/target/release/minotari_console_wallet" "${TOOLS_DIR}/minotari_console_wallet"

printf 'installed minotari at %s and minotari_console_wallet at %s in %s\n' \
  "${MINOTARI_REV}" "${TARI_CONSOLE_WALLET_REV}" "${TOOLS_DIR}"
