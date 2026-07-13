#!/usr/bin/env bash
set -euo pipefail

CACHE_DIR="${1:-.bench-cache}"
TOOLS_DIR="${2:-tools}"
MINOTARI_REV="c2b8d7b65a3b4320d85b7ba118145d190c264777"
TARI_CONSOLE_WALLET_REV="9f5adb7183dc2ec285f5c8fae05f4be9735d9749"
TARI_NODE_REV="v5.4.0"
MINOTARI_DIR="${CACHE_DIR}/minotari-cli"
TARI_DIR="${CACHE_DIR}/tari"

mkdir -p "${CACHE_DIR}" "${TOOLS_DIR}"

if [ ! -d "${MINOTARI_DIR}/.git" ]; then
  git clone https://github.com/tzmWW/minotari-cli.git "${MINOTARI_DIR}"
fi

if ! git -C "${MINOTARI_DIR}" remote get-url fork >/dev/null 2>&1; then
  git -C "${MINOTARI_DIR}" remote add fork https://github.com/tzmWW/minotari-cli.git
fi
git -C "${MINOTARI_DIR}" fetch --tags fork
git -C "${MINOTARI_DIR}" checkout "${MINOTARI_REV}"
if [ -n "$(git -C "${MINOTARI_DIR}" status --porcelain --untracked-files=all)" ]; then
  printf 'minotari-cli source tree is dirty; use a fresh cache directory\n' >&2
  exit 1
fi

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
if [ -n "$(git -C "${TARI_DIR}" status --porcelain --untracked-files=all)" ]; then
  printf 'Tari source tree is dirty; use a fresh cache directory\n' >&2
  exit 1
fi

(
  cd "${TARI_DIR}"
  cargo build --release --bin minotari_console_wallet
)

cp "${TARI_DIR}/target/release/minotari_console_wallet" "${TOOLS_DIR}/minotari_console_wallet"

git -C "${TARI_DIR}" checkout "${TARI_NODE_REV}"
if [ -n "$(git -C "${TARI_DIR}" status --porcelain --untracked-files=all)" ]; then
  printf 'Tari source tree became dirty before node build\n' >&2
  exit 1
fi

(
  cd "${TARI_DIR}"
  cargo build --release --bin minotari_node
)

cp "${TARI_DIR}/target/release/minotari_node" "${TOOLS_DIR}/minotari_node"

printf 'installed minotari at %s, minotari_console_wallet at %s, minotari_node at %s in %s\n' \
  "${MINOTARI_REV}" "${TARI_CONSOLE_WALLET_REV}" "${TARI_NODE_REV}" "${TOOLS_DIR}"
