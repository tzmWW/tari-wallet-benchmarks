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
PATCH_DIR="${SCRIPT_DIR}/../patches"

MINOTARI_REPO="https://github.com/tari-project/minotari-cli.git"
MINOTARI_FEATURE_REPO="https://github.com/tzmWW/minotari-cli.git"
MINOTARI_BASE_REV="360c4848a54d65fd710266233cc9277b0f785e74"
MINOTARI_BASE_TREE="e9bbd1fb7b538e213e17c2986b85940435adce26"
MINOTARI_FEATURE_REV="c2b8d7b65a3b4320d85b7ba118145d190c264777"
MINOTARI_FEATURE_TREE="2fc434e0309f0ee92806eeea97bc33edacfbb793"
MINOTARI_FINAL_TREE="bd03010af1c92a690bc55d1c4931c683a78d4571"
MINOTARI_COMPLETE_DIFF_SHA256="23740f25aca7031827506d814ace58f30ed181bf22e269840cdc63bc3759b11f"
MINOTARI_DIR="${CACHE_DIR}/minotari-cli"

TARI_REPO="https://github.com/tari-project/tari.git"
TARI_CONSOLE_WALLET_REV="9f5adb7183dc2ec285f5c8fae05f4be9735d9749"
TARI_CONSOLE_WALLET_COMMIT="9f5adb7183dc2ec285f5c8fae05f4be9735d9749"
TARI_CONSOLE_WALLET_TREE="be2020d2eb904507fa20442448ef76b6e8f0d502"
TARI_NODE_REV="v5.4.0"
TARI_NODE_COMMIT="03e7ccd3257d669f8d73662bb214602fe0987c17"
TARI_NODE_TREE="cd365137e77901f5ddcc484ef0d2faf3c042c8bf"
TARI_DIR="${CACHE_DIR}/tari"

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

normalize_macos_signature() {
  if [ "$(uname -s)" = "Darwin" ]; then
    codesign --force --sign - "$1"
    codesign --verify --strict "$1"
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
  local directory="$1"
  local label="$2"
  if [ -n "$(git -C "${directory}" status --porcelain --untracked-files=all)" ]; then
    printf '%s source tree is dirty; use a fresh cache directory\n' "${label}" >&2
    exit 1
  fi
}

verify_checkout() {
  local directory="$1"
  local revision="$2"
  local expected_commit="$3"
  local expected_tree="$4"
  local label="$5"
  git -C "${directory}" checkout --detach "${revision}"
  if [ "$(git -C "${directory}" rev-parse HEAD)" != "${expected_commit}" ]; then
    printf '%s revision did not resolve to expected commit %s\n' "${label}" "${expected_commit}" >&2
    exit 1
  fi
  if [ "$(git -C "${directory}" rev-parse HEAD^{tree})" != "${expected_tree}" ]; then
    printf '%s upstream tree does not match expected tree %s\n' "${label}" "${expected_tree}" >&2
    exit 1
  fi
  require_clean_repository "${directory}" "${label}"
}

apply_minotari_patch() {
  local file_name="$1"
  local expected_sha="$2"
  local expected_tree="$3"
  local patch_path="${PATCH_DIR}/${file_name}"
  require_sha256 "${patch_path}" "${expected_sha}"
  git -C "${MINOTARI_DIR}" apply --check --index "${patch_path}"
  git -C "${MINOTARI_DIR}" apply --index "${patch_path}"
  local actual_tree
  actual_tree="$(git -C "${MINOTARI_DIR}" write-tree)"
  if [ "${actual_tree}" != "${expected_tree}" ]; then
    printf 'Minotari result tree after %s does not match expected tree %s\n' "${file_name}" "${expected_tree}" >&2
    exit 1
  fi
}

mkdir -p "${CACHE_DIR}"

if [ ! -d "${MINOTARI_DIR}/.git" ]; then
  git clone "${MINOTARI_REPO}" "${MINOTARI_DIR}"
fi
require_clean_repository "${MINOTARI_DIR}" "minotari-cli"
git -C "${MINOTARI_DIR}" remote set-url origin "${MINOTARI_REPO}"
git -C "${MINOTARI_DIR}" fetch --tags origin
verify_checkout "${MINOTARI_DIR}" "${MINOTARI_BASE_REV}" "${MINOTARI_BASE_REV}" "${MINOTARI_BASE_TREE}" "minotari-cli"

cleanup_minotari() {
  git -C "${MINOTARI_DIR}" reset --hard "${MINOTARI_BASE_REV}" >/dev/null 2>&1 || true
}
trap cleanup_minotari EXIT

apply_minotari_patch \
  "minotari-fixed-range-scan.patch" \
  "8efbed4f8cfbd87f5ad83080fd9ad70fdf9b8841b48b13279c9863b38fda807d" \
  "${MINOTARI_FEATURE_TREE}"

# Cargo consumes this public scanner-fix commit. Its tree must exactly match
# the upstream base plus the tracked scanner patch.
git -C "${MINOTARI_DIR}" fetch --no-tags "${MINOTARI_FEATURE_REPO}" "${MINOTARI_FEATURE_REV}"
if [ "$(git -C "${MINOTARI_DIR}" rev-parse FETCH_HEAD)" != "${MINOTARI_FEATURE_REV}" ] ||
   [ "$(git -C "${MINOTARI_DIR}" rev-parse FETCH_HEAD^{tree})" != "${MINOTARI_FEATURE_TREE}" ]; then
  printf 'Cargo Minotari scanner-fix revision/tree verification failed\n' >&2
  exit 1
fi

apply_minotari_patch \
  "minotari-wallet-password-env.patch" \
  "fa49b2d0fa25ae31e2fdc9e17f85ca67a9a0206b9a62192d1b632d14b67888a6" \
  "${MINOTARI_FINAL_TREE}"

git -C "${MINOTARI_DIR}" diff --cached --check
if ! git -C "${MINOTARI_DIR}" diff --quiet; then
  printf 'Minotari source contains unstaged changes beyond the tracked patches\n' >&2
  exit 1
fi
if [ -n "$(git -C "${MINOTARI_DIR}" ls-files --others --exclude-standard)" ]; then
  printf 'Minotari source contains untracked files beyond the tracked patches\n' >&2
  exit 1
fi
ACTUAL_COMPLETE_DIFF_SHA256="$(git -c diff.algorithm=myers -C "${MINOTARI_DIR}" diff --cached --full-index --binary --no-ext-diff --no-textconv --no-renames "${MINOTARI_BASE_REV}" | sha256_stdin)"
if [ "${ACTUAL_COMPLETE_DIFF_SHA256}" != "${MINOTARI_COMPLETE_DIFF_SHA256}" ]; then
  printf 'Minotari complete diff SHA-256 mismatch: expected %s, got %s\n' \
    "${MINOTARI_COMPLETE_DIFF_SHA256}" "${ACTUAL_COMPLETE_DIFF_SHA256}" >&2
  exit 1
fi

if [ ! -d "${TARI_DIR}/.git" ]; then
  git clone "${TARI_REPO}" "${TARI_DIR}"
fi
require_clean_repository "${TARI_DIR}" "Tari"
git -C "${TARI_DIR}" remote set-url origin "${TARI_REPO}"
git -C "${TARI_DIR}" fetch --tags origin
verify_checkout \
  "${TARI_DIR}" "${TARI_CONSOLE_WALLET_REV}" "${TARI_CONSOLE_WALLET_COMMIT}" \
  "${TARI_CONSOLE_WALLET_TREE}" "Tari console wallet"

if [ "${VERIFY_ONLY}" = true ]; then
  verify_checkout "${TARI_DIR}" "${TARI_NODE_REV}" "${TARI_NODE_COMMIT}" "${TARI_NODE_TREE}" "Tari node"
  printf 'source provenance PASS: minotari base %s + fixed-range scanner and password patches -> %s; console %s; node %s\n' \
    "${MINOTARI_BASE_REV}" "${MINOTARI_FINAL_TREE}" "${TARI_CONSOLE_WALLET_REV}" "${TARI_NODE_REV}"
  exit 0
fi

(
  cd "${MINOTARI_DIR}"
  cargo build --release --bin minotari
)

(
  cd "${TARI_DIR}"
  cargo build --release --bin minotari_console_wallet
)

verify_checkout "${TARI_DIR}" "${TARI_NODE_REV}" "${TARI_NODE_COMMIT}" "${TARI_NODE_TREE}" "Tari node"
(
  cd "${TARI_DIR}"
  cargo build --release --bin minotari_node
)

mkdir -p "${TOOLS_DIR}"
cp "${MINOTARI_DIR}/target/release/minotari" "${TOOLS_DIR}/minotari"
cp "${TARI_DIR}/target/release/minotari_console_wallet" "${TOOLS_DIR}/minotari_console_wallet"
cp "${TARI_DIR}/target/release/minotari_node" "${TOOLS_DIR}/minotari_node"
normalize_macos_signature "${TOOLS_DIR}/minotari"
normalize_macos_signature "${TOOLS_DIR}/minotari_console_wallet"
normalize_macos_signature "${TOOLS_DIR}/minotari_node"

printf 'installed fixed-range minotari %s, minotari_console_wallet at %s, and minotari_node at %s in %s\n' \
  "${MINOTARI_FEATURE_REV}" "${TARI_CONSOLE_WALLET_REV}" "${TARI_NODE_REV}" "${TOOLS_DIR}"
