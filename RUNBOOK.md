# Runbook

This repository is a standalone harness for `tari-project/wallet-benchmarks#1`.
It is not a PR checkout. Clone it directly, build it, fund the generated Esmeralda
addresses, then run the benchmark.

## Prerequisites

- Rust stable with edition 2024 support.
- Access to Esmeralda funds. Tari Universe mining is the expected way to fund the
  three generated addresses.
- `minotari` built from `360c4848a54d65fd710266233cc9277b0f785e74` and
  `minotari_console_wallet` built from Tari
  `9f5adb7183dc2ec285f5c8fae05f4be9735d9749`, placed at the paths in
  `harness.toml`.
- `minotari_payment_processor` built from
  `f0572c98cbfac7377412dc6d4094c7d7dfc5de2c`, using the helper script below.

## One-time setup

```sh
cp harness.toml.example harness.toml
scripts/fetch-minotari-cli.sh .bench-cache tools
scripts/fetch-payment-processor.sh .bench-cache
cargo build --release --all-features
```

Build or copy the matching `minotari_console_wallet` and `minotari` binaries into
`tools/`, or edit `[paths]` in `harness.toml` to point at your binaries.

## Generate wallet addresses

```sh
export HARNESS_WALLET_PW='replace-with-a-long-local-password'
cargo run -- addresses --config harness.toml --out .secrets/seeds.env
```

The command prints three Esmeralda addresses:

- `old_wallet`: Mode 1, `minotari_console_wallet`.
- `new_wallet`: Mode 2, in-process `minotari` library path.
- `payment_processor`: Mode 3, real `minotari_payment_processor`.

The seed phrases are written only to `.secrets/seeds.env`, which is ignored by
Git. Do not commit it.

## Fund wallets

Fund each generated address with at least `A_fund` from `harness.toml`
(`10000 T` by default). Funding is intentionally outside the measured benchmark.
Wait until the funding output has at least `C_min` confirmations.

After funding, record each tx in `[funding.<mode>]` in `harness.toml` with the
amount, transaction id, and block height. These fields are written to result
profiles as public benchmark inputs.

## Preflight

```sh
source .secrets/seeds.env
export HARNESS_WALLET_PW='replace-with-a-long-local-password'
cargo run -- preflight --config harness.toml
```

Preflight validates the Esmeralda-only guard, seed material, wallet password env,
and local binary paths. It prints the PP build command if the PP binary is
missing.

## Run

```sh
cargo run --features live-minotari -- run \
  --config harness.toml \
  --profile baselines/esmeralda_baseline.json
```

The result profile is written atomically and does not contain seed phrases or
passwords. Public addresses may appear in the profile.

Implementation note: the committed harness currently writes the full result
profile shape and exercises Mode 2 plus PP companion fresh scan paths when the
`live-minotari` feature is enabled. The `[benchmark].scan_batch_size` setting
controls how many blocks each HTTP scan request fetches; larger values make
full-chain scan cells practical on Esmeralda. These fresh scan cells deliberately
wipe their local databases per repetition, so they are long-running and print
per-cell progress while they execute. The funded send-side B0/S0-S7 runner still
has to be completed before the profile can be used as final bounty performance
evidence.

## Schema

```sh
cargo run -- schema --out RESULT_PROFILE_SCHEMA.json
```

The JSON profile is designed for automated comparison. Every profile records the
network, hardware environment, pinned versions, benchmark parameters, per-mode
scenario cells, findings, and chain-verification status value.

## Verification Gates

Before publishing a result profile, run:

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
ast-grep scan
```

The AST rules intentionally block harness-level retry, backoff, throttling,
scenario dispatch sleeps, and hidden UTXO pre-partitioning in source code.
