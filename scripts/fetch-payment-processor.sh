#!/usr/bin/env bash
set -euo pipefail

CACHE_DIR="${1:-.bench-cache}"
TOOLS_DIR="${2:-tools}"
PP_REV="f0572c98cbfac7377412dc6d4094c7d7dfc5de2c"
MINOTARI_REV="1391dbd2155c96e885379d72b76e33582f0aad87"
TARI_CONSOLE_WALLET_REV="9f5adb7183dc2ec285f5c8fae05f4be9735d9749"
TARI_NODE_REV="v5.4.0"
PP_DIR="${CACHE_DIR}/minotari_payment_processor"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PATCH_FILE="${SCRIPT_DIR}/../patches/payment-processor-fee-rate.patch"
MANIFEST="${TOOLS_DIR}/build-manifest.json"

sha256_file() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d ' ' -f 1
  else
    sha256sum "$1" | cut -d ' ' -f 1
  fi
}

mkdir -p "${CACHE_DIR}"

if [ ! -d "${PP_DIR}/.git" ]; then
  git clone https://github.com/tari-project/minotari_payment_processor.git "${PP_DIR}"
fi

git -C "${PP_DIR}" fetch --tags origin
git -C "${PP_DIR}" checkout "${PP_REV}"
if git -C "${PP_DIR}" apply --check "${PATCH_FILE}"; then
  if [ -n "$(git -C "${PP_DIR}" status --porcelain --untracked-files=all)" ]; then
    printf 'payment-processor source tree is dirty before patching; use a fresh cache directory\n' >&2
    exit 1
  fi
  git -C "${PP_DIR}" apply "${PATCH_FILE}"
elif ! git -C "${PP_DIR}" apply --reverse --check "${PATCH_FILE}"; then
  printf 'payment-processor fee patch does not apply cleanly\n' >&2
  exit 1
fi
if [ "$(git -C "${PP_DIR}" diff --name-only)" != "minotari_payment_processor/src/workers/unsigned_tx_creator.rs" ] ||
   [ -n "$(git -C "${PP_DIR}" ls-files --others --exclude-standard)" ]; then
  printf 'payment-processor tree contains changes beyond the tracked fee patch\n' >&2
  exit 1
fi
git -C "${PP_DIR}" diff --check

mkdir -p "${PP_DIR}/data"
(
  cd "${PP_DIR}"
  rm -f data/payments.db
  for migration in migrations/*.sql; do
    sqlite3 data/payments.db < "${migration}"
  done
  DATABASE_URL=sqlite://data/payments.db cargo build --release
)

mkdir -p "${TOOLS_DIR}"
cp "${PP_DIR}/target/release/minotari_payment_processor" "${TOOLS_DIR}/minotari_payment_processor"
MINOTARI_SHA="$(sha256_file "${TOOLS_DIR}/minotari")"
CONSOLE_SHA="$(sha256_file "${TOOLS_DIR}/minotari_console_wallet")"
NODE_SHA="$(sha256_file "${TOOLS_DIR}/minotari_node")"
PP_SHA="$(sha256_file "${TOOLS_DIR}/minotari_payment_processor")"
PATCH_SHA="$(sha256_file "${PATCH_FILE}")"
printf '{\n  "schema_version": 1,\n  "payment_processor_patch_sha256": "%s",\n  "artifacts": {\n    "minotari": {"source_revision": "%s", "sha256": "%s"},\n    "minotari_console_wallet": {"source_revision": "%s", "sha256": "%s"},\n    "minotari_node": {"source_revision": "%s", "sha256": "%s"},\n    "minotari_payment_processor": {"source_revision": "%s", "sha256": "%s"}\n  }\n}\n' \
  "${PATCH_SHA}" "${MINOTARI_REV}" "${MINOTARI_SHA}" \
  "${TARI_CONSOLE_WALLET_REV}" "${CONSOLE_SHA}" "${TARI_NODE_REV}" "${NODE_SHA}" \
  "${PP_REV}" "${PP_SHA}" > "${MANIFEST}"

printf 'built %s at %s and wrote %s\n' "${PP_DIR}/target/release/minotari_payment_processor" "${PP_REV}" "${MANIFEST}"
