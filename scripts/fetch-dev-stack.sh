#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -gt 2 ]; then
  printf 'usage: %s [CACHE_DIR] [TOOLS_DIR]\n' "$0" >&2
  exit 2
fi

CACHE_DIR="${1:-.bench-cache/dev}"
TOOLS_DIR="${2:-tools}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PATCH_DIR="${SCRIPT_DIR}/../patches"
MANIFEST="${TOOLS_DIR}/build-manifest.json"

MINOTARI_REPO="https://github.com/tari-project/minotari-cli.git"
MINOTARI_REF="${MINOTARI_DEV_REF:-main}"
TARI_REPO="https://github.com/tari-project/tari.git"
TARI_REQUESTED_REF="${TARI_DEV_REF:-latest-prerelease}"
TARI_REF="${TARI_DEV_REF:-}"
PP_REPO="https://github.com/tari-project/minotari_payment_processor.git"
PP_REF="${PP_DEV_REF:-main}"

MINOTARI_DIR="${CACHE_DIR}/minotari-cli"
TARI_DIR="${CACHE_DIR}/tari"
PP_DIR="${CACHE_DIR}/minotari_payment_processor"

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

prepare_checkout() {
  local repository="$1"
  local directory="$2"
  local ref="$3"
  local label="$4"
  if [ ! -d "${directory}/.git" ]; then
    git clone "${repository}" "${directory}"
  fi
  if [ -n "$(git -C "${directory}" status --porcelain --untracked-files=all)" ]; then
    printf '%s source tree is dirty; use a fresh dev cache directory\n' "${label}" >&2
    exit 1
  fi
  git -C "${directory}" remote set-url origin "${repository}"
  git -C "${directory}" fetch --prune --prune-tags --tags origin
  local resolved
  if git -C "${directory}" rev-parse --verify --quiet "origin/${ref}^{commit}" >/dev/null; then
    resolved="$(git -C "${directory}" rev-parse "origin/${ref}^{commit}")"
  elif git -C "${directory}" rev-parse --verify --quiet "refs/tags/${ref}^{commit}" >/dev/null; then
    resolved="$(git -C "${directory}" rev-parse "refs/tags/${ref}^{commit}")"
  else
    printf '%s ref %s is not a fetched origin branch or tag\n' "${label}" "${ref}" >&2
    exit 1
  fi
  git -C "${directory}" checkout --detach "${resolved}"
  printf '%s' "${resolved}"
}

mkdir -p "${CACHE_DIR}" "${TOOLS_DIR}"

if [ ! -d "${TARI_DIR}/.git" ]; then
  git clone "${TARI_REPO}" "${TARI_DIR}"
fi
git -C "${TARI_DIR}" remote set-url origin "${TARI_REPO}"
git -C "${TARI_DIR}" fetch --prune --prune-tags --tags origin
if [ -z "${TARI_REF}" ]; then
  TARI_REF="$(git -C "${TARI_DIR}" tag --list 'v*-pre.*' --sort=-version:refname | perl -ne 'print; exit')"
fi
if [ -z "${TARI_REF}" ]; then
  printf 'no Tari development prerelease tag was found\n' >&2
  exit 1
fi

MINOTARI_COMMIT="$(prepare_checkout "${MINOTARI_REPO}" "${MINOTARI_DIR}" "${MINOTARI_REF}" "minotari-cli")"
TARI_COMMIT="$(prepare_checkout "${TARI_REPO}" "${TARI_DIR}" "${TARI_REF}" "Tari")"
PP_COMMIT="$(prepare_checkout "${PP_REPO}" "${PP_DIR}" "${PP_REF}" "payment processor")"

cleanup_sources() {
  git -C "${MINOTARI_DIR}" reset --hard "${MINOTARI_COMMIT}" >/dev/null 2>&1 || true
  git -C "${PP_DIR}" reset --hard "${PP_COMMIT}" >/dev/null 2>&1 || true
}
trap cleanup_sources EXIT

# Cargo follows Minotari's development branch. Updating the lockfile is an
# intentional source update; commit it together with any required API adaptation
# before starting a measured run.
cargo update -p minotari
LOCKED_MINOTARI_COMMIT="$(cargo metadata --all-features --format-version 1 --locked | jq -r '.packages[] | select(.name == "minotari" and (.source // "" | startswith("git+"))) | .source' | perl -ne 'if (/#([0-9a-f]{40})$/) { print $1; exit }')"
if [ "${LOCKED_MINOTARI_COMMIT}" != "${MINOTARI_COMMIT}" ]; then
  printf 'Cargo resolved Minotari %s but upstream %s resolved to %s\n' \
    "${LOCKED_MINOTARI_COMMIT}" "${MINOTARI_REF}" "${MINOTARI_COMMIT}" >&2
  exit 1
fi
SOURCE_TARI_VERSION="$(cargo metadata --manifest-path "${TARI_DIR}/Cargo.toml" --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "tari_common") | .version')"
LOCKED_TARI_VERSION="$(cargo metadata --all-features --format-version 1 --locked | jq -r '.packages[] | select(.name == "tari_common" and (.source // "" | startswith("registry+"))) | .version')"
if [ "${LOCKED_TARI_VERSION}" != "${SOURCE_TARI_VERSION}" ]; then
  printf 'harness Tari API crates are %s but resolved runtime source is %s; update Cargo.toml and adapt the harness\n' \
    "${LOCKED_TARI_VERSION}" "${SOURCE_TARI_VERSION}" >&2
  exit 1
fi
cargo check --all-features

MINOTARI_UPSTREAM_TREE="$(git -C "${MINOTARI_DIR}" rev-parse HEAD^{tree})"
MINOTARI_PASSWORD_PATCH="${PATCH_DIR}/minotari-wallet-password-env.patch"
MINOTARI_PASSWORD_PATCH_SHA="$(sha256_file "${MINOTARI_PASSWORD_PATCH}")"
git -C "${MINOTARI_DIR}" apply --check --index "${MINOTARI_PASSWORD_PATCH}"
git -C "${MINOTARI_DIR}" apply --index "${MINOTARI_PASSWORD_PATCH}"
git -C "${MINOTARI_DIR}" diff --cached --check
MINOTARI_RESULT_TREE="$(git -C "${MINOTARI_DIR}" write-tree)"
MINOTARI_DIFF_SHA="$(git -c diff.algorithm=myers -C "${MINOTARI_DIR}" diff --cached --full-index --binary --no-ext-diff --no-textconv --no-renames "${MINOTARI_COMMIT}" | sha256_stdin)"

PP_UPSTREAM_TREE="$(git -C "${PP_DIR}" rev-parse HEAD^{tree})"
PP_PATCH="${PATCH_DIR}/payment-processor-fee-rate.patch"
PP_PATCH_SHA="$(sha256_file "${PP_PATCH}")"
git -C "${PP_DIR}" apply --check --index "${PP_PATCH}"
git -C "${PP_DIR}" apply --index "${PP_PATCH}"
git -C "${PP_DIR}" diff --cached --check
PP_RESULT_TREE="$(git -C "${PP_DIR}" write-tree)"
PP_DIFF_SHA="$(git -c diff.algorithm=myers -C "${PP_DIR}" diff --cached --full-index --binary --no-ext-diff --no-textconv --no-renames "${PP_COMMIT}" | sha256_stdin)"

TARI_TREE="$(git -C "${TARI_DIR}" rev-parse HEAD^{tree})"
EMPTY_DIFF_SHA="e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"

(
  cd "${MINOTARI_DIR}"
  cargo build --release --bin minotari
)
(
  cd "${TARI_DIR}"
  cargo build --release --bin minotari_console_wallet --bin minotari_node
)
mkdir -p "${PP_DIR}/data"
(
  cd "${PP_DIR}"
  rm -f data/payments.db
  for migration in migrations/*.sql; do
    sqlite3 data/payments.db < "${migration}"
  done
  DATABASE_URL=sqlite://data/payments.db cargo build --release
)

cp "${MINOTARI_DIR}/target/release/minotari" "${TOOLS_DIR}/minotari"
cp "${TARI_DIR}/target/release/minotari_console_wallet" "${TOOLS_DIR}/minotari_console_wallet"
cp "${TARI_DIR}/target/release/minotari_node" "${TOOLS_DIR}/minotari_node"
cp "${PP_DIR}/target/release/minotari_payment_processor" "${TOOLS_DIR}/minotari_payment_processor"
for artifact in minotari minotari_console_wallet minotari_node minotari_payment_processor; do
  normalize_macos_signature "${TOOLS_DIR}/${artifact}"
done

RESOLVED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
MINOTARI_SHA="$(sha256_file "${TOOLS_DIR}/minotari")"
CONSOLE_SHA="$(sha256_file "${TOOLS_DIR}/minotari_console_wallet")"
NODE_SHA="$(sha256_file "${TOOLS_DIR}/minotari_node")"
PP_SHA="$(sha256_file "${TOOLS_DIR}/minotari_payment_processor")"

cat > "${MANIFEST}" <<EOF
{
  "schema_version": 2,
  "channel": "dev",
  "resolved_at": "${RESOLVED_AT}",
  "sources": {
    "minotari_cli": {
      "repository": "${MINOTARI_REPO}",
      "upstream": {"revision": "${MINOTARI_REF}", "commit": "${MINOTARI_COMMIT}", "tree": "${MINOTARI_UPSTREAM_TREE}"},
      "patches": [{"path": "patches/minotari-wallet-password-env.patch", "sha256": "${MINOTARI_PASSWORD_PATCH_SHA}", "result_tree": "${MINOTARI_RESULT_TREE}"}],
      "complete_diff_sha256": "${MINOTARI_DIFF_SHA}",
      "result_tree": "${MINOTARI_RESULT_TREE}"
    },
    "tari_console_wallet": {
      "repository": "${TARI_REPO}",
      "upstream": {"revision": "${TARI_REQUESTED_REF}", "commit": "${TARI_COMMIT}", "tree": "${TARI_TREE}"},
      "patches": [], "complete_diff_sha256": "${EMPTY_DIFF_SHA}", "result_tree": "${TARI_TREE}"
    },
    "minotari_node": {
      "repository": "${TARI_REPO}",
      "upstream": {"revision": "${TARI_REQUESTED_REF}", "commit": "${TARI_COMMIT}", "tree": "${TARI_TREE}"},
      "patches": [], "complete_diff_sha256": "${EMPTY_DIFF_SHA}", "result_tree": "${TARI_TREE}"
    },
    "payment_processor": {
      "repository": "${PP_REPO}",
      "upstream": {"revision": "${PP_REF}", "commit": "${PP_COMMIT}", "tree": "${PP_UPSTREAM_TREE}"},
      "patches": [{"path": "patches/payment-processor-fee-rate.patch", "sha256": "${PP_PATCH_SHA}", "result_tree": "${PP_RESULT_TREE}"}],
      "complete_diff_sha256": "${PP_DIFF_SHA}",
      "result_tree": "${PP_RESULT_TREE}"
    }
  },
  "artifacts": {
    "minotari": {"source": "minotari_cli", "source_revision": "${MINOTARI_COMMIT}", "source_tree": "${MINOTARI_RESULT_TREE}", "sha256": "${MINOTARI_SHA}"},
    "minotari_console_wallet": {"source": "tari_console_wallet", "source_revision": "${TARI_COMMIT}", "source_tree": "${TARI_TREE}", "sha256": "${CONSOLE_SHA}"},
    "minotari_node": {"source": "minotari_node", "source_revision": "${TARI_COMMIT}", "source_tree": "${TARI_TREE}", "sha256": "${NODE_SHA}"},
    "minotari_payment_processor": {"source": "payment_processor", "source_revision": "${PP_COMMIT}", "source_tree": "${PP_RESULT_TREE}", "sha256": "${PP_SHA}"}
  }
}
EOF

printf 'dev stack resolved and built: minotari=%s tari=%s payment-processor=%s; manifest=%s\n' \
  "${MINOTARI_COMMIT}" "${TARI_COMMIT}" "${PP_COMMIT}" "${MANIFEST}"
