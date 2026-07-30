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
MINOTARI_RESULT_TREE="cf6acf000f787817a795668c93470b139970feb6"
MINOTARI_COMPLETE_DIFF_SHA256="118dbe659efed99528159e56f509a01f5a9b789ea57a9ea3267e2b60fbf0d144"
MINOTARI_PASSWORD_PATCH_SHA256="fa49b2d0fa25ae31e2fdc9e17f85ca67a9a0206b9a62192d1b632d14b67888a6"

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

cat > "${MANIFEST}" <<EOF
{
  "schema_version": 2,
  "sources": {
    "minotari_cli": {
      "repository": "${MINOTARI_REPO}",
      "upstream": {"revision": "${MINOTARI_BASE_REV}", "commit": "${MINOTARI_BASE_REV}", "tree": "${MINOTARI_BASE_TREE}"},
      "patches": [
        {"path": "patches/minotari-wallet-password-env.patch", "sha256": "${MINOTARI_PASSWORD_PATCH_SHA256}", "result_tree": "${MINOTARI_RESULT_TREE}"}
      ],
      "complete_diff_sha256": "${MINOTARI_COMPLETE_DIFF_SHA256}",
      "result_tree": "${MINOTARI_RESULT_TREE}"
    },
    "tari_console_wallet": {
      "repository": "${TARI_REPO}",
      "upstream": {"revision": "${TARI_CONSOLE_WALLET_REV}", "commit": "${TARI_CONSOLE_WALLET_REV}", "tree": "${TARI_CONSOLE_WALLET_TREE}"},
      "patches": [],
      "complete_diff_sha256": "${EMPTY_DIFF_SHA256}",
      "result_tree": "${TARI_CONSOLE_WALLET_TREE}"
    },
    "minotari_node": {
      "repository": "${TARI_REPO}",
      "upstream": {"revision": "${TARI_NODE_REV}", "commit": "${TARI_NODE_COMMIT}", "tree": "${TARI_NODE_TREE}"},
      "patches": [],
      "complete_diff_sha256": "${EMPTY_DIFF_SHA256}",
      "result_tree": "${TARI_NODE_TREE}"
    },
    "payment_processor": {
      "repository": "${PP_REPO}",
      "upstream": {"revision": "${PP_REV}", "commit": "${PP_REV}", "tree": "${PP_BASE_TREE}"},
      "patches": [
        {"path": "patches/payment-processor-fee-rate.patch", "sha256": "${PP_PATCH_SHA256}", "result_tree": "${PP_RESULT_TREE}"}
      ],
      "complete_diff_sha256": "${PP_COMPLETE_DIFF_SHA256}",
      "result_tree": "${PP_RESULT_TREE}"
    }
  },
  "artifacts": {
    "minotari": {"source": "minotari_cli", "source_revision": "${MINOTARI_BASE_REV}", "source_tree": "${MINOTARI_RESULT_TREE}", "sha256": "${MINOTARI_SHA}"},
    "minotari_console_wallet": {"source": "tari_console_wallet", "source_revision": "${TARI_CONSOLE_WALLET_REV}", "source_tree": "${TARI_CONSOLE_WALLET_TREE}", "sha256": "${CONSOLE_SHA}"},
    "minotari_node": {"source": "minotari_node", "source_revision": "${TARI_NODE_REV}", "source_tree": "${TARI_NODE_TREE}", "sha256": "${NODE_SHA}"},
    "minotari_payment_processor": {"source": "payment_processor", "source_revision": "${PP_REV}", "source_tree": "${PP_RESULT_TREE}", "sha256": "${PP_SHA}"}
  }
}
EOF

printf 'built payment processor at %s, verified exact source provenance, and wrote schema-v2 manifest %s\n' \
  "${PP_REV}" "${MANIFEST}"
