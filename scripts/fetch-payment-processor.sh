#!/usr/bin/env bash
set -euo pipefail

VERIFY_ONLY=false
POSITIONAL=()
for arg in "$@"; do
  case "${arg}" in
    --verify-only) VERIFY_ONLY=true ;;
    *) POSITIONAL+=("${arg}") ;;
  esac
done
if [ "${#POSITIONAL[@]}" -gt 2 ]; then
  printf 'usage: %s [--verify-only] [CACHE_DIR] [TOOLS_DIR]\n' "$0" >&2
  exit 2
fi

CACHE_DIR="${POSITIONAL[0]:-.bench-cache}"
TOOLS_DIR="${POSITIONAL[1]:-tools}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PATCH_FILE="${SCRIPT_DIR}/../patches/payment-processor-fee-rate.patch"
MANIFEST="${TOOLS_DIR}/build-manifest.json"

PP_REPO="https://github.com/tari-project/minotari_payment_processor.git"
PP_REV="f0572c98cbfac7377412dc6d4094c7d7dfc5de2c"
PP_BASE_TREE="add06a544f950f724caa13b972cfc13e5d666c90"
PP_PATCH_SHA256="69c3001b4474d478822651810dc5f25cae5c8bfede2f9bc756de6ded37dc89fe"
PP_RESULT_TREE="8f15669442f3da67fc4636de00b80c666d890c5c"
PP_COMPLETE_DIFF_SHA256="8b467bf65003de81ea752092ea3b4f2914e28b284590425d155fda4ad13287d8"
PP_DIR="${CACHE_DIR}/minotari_payment_processor"

MINOTARI_REPO="https://github.com/tari-project/minotari-cli.git"
MINOTARI_BASE_REV="360c4848a54d65fd710266233cc9277b0f785e74"
MINOTARI_BASE_TREE="e9bbd1fb7b538e213e17c2986b85940435adce26"
MINOTARI_FEATURE_REV="1391dbd2155c96e885379d72b76e33582f0aad87"
MINOTARI_RESULT_TREE="f36ef55c065732ea9cfcfdfda94f71b7199842e1"
MINOTARI_COMPLETE_DIFF_SHA256="881428c6a82e1add7a516e16b706c4d168ef14f222085f03cd9b792c523deef7"

TARI_REPO="https://github.com/tari-project/tari.git"
TARI_CONSOLE_WALLET_REV="9f5adb7183dc2ec285f5c8fae05f4be9735d9749"
TARI_CONSOLE_WALLET_TREE="be2020d2eb904507fa20442448ef76b6e8f0d502"
TARI_NODE_REV="v5.4.0"
TARI_NODE_COMMIT="03e7ccd3257d669f8d73662bb214602fe0987c17"
TARI_NODE_TREE="cd365137e77901f5ddcc484ef0d2faf3c042c8bf"
EMPTY_DIFF_SHA256="e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"

sha256_file() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d ' ' -f 1
  else
    sha256sum "$1" | cut -d ' ' -f 1
  fi
}

sha256_stdin() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 | cut -d ' ' -f 1
  else
    sha256sum | cut -d ' ' -f 1
  fi
}

require_sha256() {
  local path="$1"
  local expected="$2"
  local actual
  actual="$(sha256_file "${path}")"
  if [ "${actual}" != "${expected}" ]; then
    printf 'SHA-256 mismatch for %s: expected %s, got %s\n' "${path}" "${expected}" "${actual}" >&2
    exit 1
  fi
}

require_clean_repository() {
  if [ -n "$(git -C "$1" status --porcelain --untracked-files=all)" ]; then
    printf '%s source tree is dirty; use a fresh cache directory\n' "$2" >&2
    exit 1
  fi
}

"${SCRIPT_DIR}/fetch-minotari-cli.sh" --verify-only "${CACHE_DIR}" "${TOOLS_DIR}"

mkdir -p "${CACHE_DIR}"
if [ ! -d "${PP_DIR}/.git" ]; then
  git clone "${PP_REPO}" "${PP_DIR}"
fi
require_clean_repository "${PP_DIR}" "payment-processor"
git -C "${PP_DIR}" remote set-url origin "${PP_REPO}"
git -C "${PP_DIR}" fetch --tags origin
git -C "${PP_DIR}" checkout --detach "${PP_REV}"
if [ "$(git -C "${PP_DIR}" rev-parse HEAD)" != "${PP_REV}" ] ||
   [ "$(git -C "${PP_DIR}" rev-parse HEAD^{tree})" != "${PP_BASE_TREE}" ]; then
  printf 'payment-processor upstream revision/tree verification failed\n' >&2
  exit 1
fi
require_clean_repository "${PP_DIR}" "payment-processor"

cleanup_payment_processor() {
  git -C "${PP_DIR}" reset --hard "${PP_REV}" >/dev/null 2>&1 || true
}
trap cleanup_payment_processor EXIT

require_sha256 "${PATCH_FILE}" "${PP_PATCH_SHA256}"
git -C "${PP_DIR}" apply --check --index "${PATCH_FILE}"
git -C "${PP_DIR}" apply --index "${PATCH_FILE}"
if [ "$(git -C "${PP_DIR}" write-tree)" != "${PP_RESULT_TREE}" ]; then
  printf 'payment-processor patched tree does not match expected tree %s\n' "${PP_RESULT_TREE}" >&2
  exit 1
fi
git -C "${PP_DIR}" diff --cached --check
if ! git -C "${PP_DIR}" diff --quiet; then
  printf 'payment-processor source contains unstaged changes beyond the tracked patch\n' >&2
  exit 1
fi
if [ -n "$(git -C "${PP_DIR}" ls-files --others --exclude-standard)" ]; then
  printf 'payment-processor source contains untracked files beyond the tracked patch\n' >&2
  exit 1
fi
ACTUAL_COMPLETE_DIFF_SHA256="$(git -c diff.algorithm=myers -C "${PP_DIR}" diff --cached --full-index --binary --no-ext-diff --no-textconv --no-renames "${PP_REV}" | sha256_stdin)"
if [ "${ACTUAL_COMPLETE_DIFF_SHA256}" != "${PP_COMPLETE_DIFF_SHA256}" ]; then
  printf 'payment-processor complete diff SHA-256 mismatch: expected %s, got %s\n' \
    "${PP_COMPLETE_DIFF_SHA256}" "${ACTUAL_COMPLETE_DIFF_SHA256}" >&2
  exit 1
fi

if [ "${VERIFY_ONLY}" = true ]; then
  printf 'source provenance PASS: payment processor %s + tracked fee patch -> %s\n' \
    "${PP_REV}" "${PP_RESULT_TREE}"
  exit 0
fi

mkdir -p "${PP_DIR}/data"
(
  cd "${PP_DIR}"
  rm -f data/payments.db
  for migration in migrations/*.sql; do
    sqlite3 data/payments.db < "${migration}"
  done
  DATABASE_URL=sqlite://data/payments.db cargo build --release
)

for artifact in minotari minotari_console_wallet minotari_node; do
  if [ ! -f "${TOOLS_DIR}/${artifact}" ]; then
    printf 'required artifact %s is missing; run fetch-minotari-cli.sh without --verify-only first\n' \
      "${TOOLS_DIR}/${artifact}" >&2
    exit 1
  fi
done
mkdir -p "${TOOLS_DIR}"
cp "${PP_DIR}/target/release/minotari_payment_processor" "${TOOLS_DIR}/minotari_payment_processor"

MINOTARI_SHA="$(sha256_file "${TOOLS_DIR}/minotari")"
CONSOLE_SHA="$(sha256_file "${TOOLS_DIR}/minotari_console_wallet")"
NODE_SHA="$(sha256_file "${TOOLS_DIR}/minotari_node")"
PP_SHA="$(sha256_file "${TOOLS_DIR}/minotari_payment_processor")"

printf '{\n  "schema_version": 2,\n  "sources": {\n    "minotari_cli": {\n      "repository": "%s",\n      "upstream": {"revision": "%s", "commit": "%s", "tree": "%s"},\n      "patches": [\n        {"path": "patches/minotari-fixed-range-scan.patch", "sha256": "8efbed4f8cfbd87f5ad83080fd9ad70fdf9b8841b48b13279c9863b38fda807d", "result_tree": "2fc434e0309f0ee92806eeea97bc33edacfbb793"},\n        {"path": "patches/minotari-exact-output-locking.patch", "sha256": "56f65ce897c1f428aeb8858faefeaf691d66e4cfa4e3027bd27b2ac856461b63", "result_tree": "818201e82cc3ab35cccba2fd1ffa4b95bdc08fd2"},\n        {"path": "patches/minotari-wallet-password-env.patch", "sha256": "c8f203f78cf5a2549be49e1e52e27474e13955a89c79a54658a0e2c06ae039c9", "result_tree": "%s"}\n      ],\n      "complete_diff_sha256": "%s",\n      "result_tree": "%s"\n    },\n    "tari_console_wallet": {\n      "repository": "%s",\n      "upstream": {"revision": "%s", "commit": "%s", "tree": "%s"},\n      "patches": [],\n      "complete_diff_sha256": "%s",\n      "result_tree": "%s"\n    },\n    "minotari_node": {\n      "repository": "%s",\n      "upstream": {"revision": "%s", "commit": "%s", "tree": "%s"},\n      "patches": [],\n      "complete_diff_sha256": "%s",\n      "result_tree": "%s"\n    },\n    "payment_processor": {\n      "repository": "%s",\n      "upstream": {"revision": "%s", "commit": "%s", "tree": "%s"},\n      "patches": [\n        {"path": "patches/payment-processor-fee-rate.patch", "sha256": "%s", "result_tree": "%s"}\n      ],\n      "complete_diff_sha256": "%s",\n      "result_tree": "%s"\n    }\n  },\n  "artifacts": {\n    "minotari": {"source": "minotari_cli", "source_revision": "%s", "source_tree": "%s", "sha256": "%s"},\n    "minotari_console_wallet": {"source": "tari_console_wallet", "source_revision": "%s", "source_tree": "%s", "sha256": "%s"},\n    "minotari_node": {"source": "minotari_node", "source_revision": "%s", "source_tree": "%s", "sha256": "%s"},\n    "minotari_payment_processor": {"source": "payment_processor", "source_revision": "%s", "source_tree": "%s", "sha256": "%s"}\n  }\n}\n' \
  "${MINOTARI_REPO}" "${MINOTARI_BASE_REV}" "${MINOTARI_BASE_REV}" "${MINOTARI_BASE_TREE}" \
  "${MINOTARI_RESULT_TREE}" "${MINOTARI_COMPLETE_DIFF_SHA256}" "${MINOTARI_RESULT_TREE}" \
  "${TARI_REPO}" "${TARI_CONSOLE_WALLET_REV}" "${TARI_CONSOLE_WALLET_REV}" "${TARI_CONSOLE_WALLET_TREE}" \
  "${EMPTY_DIFF_SHA256}" "${TARI_CONSOLE_WALLET_TREE}" \
  "${TARI_REPO}" "${TARI_NODE_REV}" "${TARI_NODE_COMMIT}" "${TARI_NODE_TREE}" \
  "${EMPTY_DIFF_SHA256}" "${TARI_NODE_TREE}" \
  "${PP_REPO}" "${PP_REV}" "${PP_REV}" "${PP_BASE_TREE}" \
  "${PP_PATCH_SHA256}" "${PP_RESULT_TREE}" "${PP_COMPLETE_DIFF_SHA256}" "${PP_RESULT_TREE}" \
  "${MINOTARI_FEATURE_REV}" "${MINOTARI_RESULT_TREE}" "${MINOTARI_SHA}" \
  "${TARI_CONSOLE_WALLET_REV}" "${TARI_CONSOLE_WALLET_TREE}" "${CONSOLE_SHA}" \
  "${TARI_NODE_REV}" "${TARI_NODE_TREE}" "${NODE_SHA}" \
  "${PP_REV}" "${PP_RESULT_TREE}" "${PP_SHA}" > "${MANIFEST}"

printf 'built payment processor at %s, verified exact source provenance, and wrote schema-v2 manifest %s\n' \
  "${PP_REV}" "${MANIFEST}"
