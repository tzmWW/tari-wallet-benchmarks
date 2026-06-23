#!/usr/bin/env bash
set -euo pipefail

CACHE_DIR="${1:-.bench-cache}"
PP_REV="f0572c98cbfac7377412dc6d4094c7d7dfc5de2c"
PP_DIR="${CACHE_DIR}/minotari_payment_processor"

mkdir -p "${CACHE_DIR}"

if [ ! -d "${PP_DIR}/.git" ]; then
  git clone https://github.com/tari-project/minotari_payment_processor.git "${PP_DIR}"
fi

git -C "${PP_DIR}" fetch --tags origin
git -C "${PP_DIR}" checkout "${PP_REV}"

mkdir -p "${PP_DIR}/data"
(
  cd "${PP_DIR}"
  DATABASE_URL=sqlite://data/payments.db cargo build --release
)

printf 'built %s at %s\n' "${PP_DIR}/target/release/minotari_payment_processor" "${PP_REV}"
