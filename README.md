# Tari Wallet Benchmarks

Standalone reproducible wallet performance harness for
`tari-project/wallet-benchmarks`.

The harness targets Esmeralda only and models the three required wallet surfaces:

- old wallet: `minotari_console_wallet` process managed by the harness
- new wallet: `minotari` crate APIs for scan, selection, signing, and broadcast
- payment processor: real `minotari_payment_processor` plus a parallel `minotari` payment-receiver wallet

Current implementation status: CLI/config/schema/seed handling, Esmeralda
guarding, result-profile generation, PP environment/API contract, static
"no hidden wallet pain" rules, and live evidence paths for all three wallet
surfaces are in place. The harness writes per-stage checkpoint profiles during
live runs, supports `preflight --check-funds` for wallet DB UTXO audits, and
separates the configured send repetition count from long fresh-scan repetitions
via `scan_repetitions`. Current live stateful send cells still emit one observed
repetition per scenario. It also includes `fund-one-sided` for operator-controlled
multi-output Esmeralda funding from a recovered minotari signing wallet. Mode 2
submitted transactions are independently queried through the public base-node
`/transactions` endpoint by extracting kernel signature data from the wallet DB's
serialized transaction. Top-level chain verification rows are emitted only for
mined transactions that are at least `C_min` deep. Fresh scan cells are
checkpointed: B0 uses an empty genesis seed, S2/S3 run after a valid S1
checkpoint, and S6/S7 run after S5. Mode 1 scan cells use real
`minotari_console_wallet --recovery`; Mode 2 and PP companion scans use fresh
minotari scanner databases. The checked baseline is no-cap send evidence with a
partial S4 ramp (`concurrent_batches = [1]`); it records real Mode 1 and Mode 2
pending-funds failures rather than hiding them, but it is not the final reference
baseline for the bounty.

Start by generating fundable addresses:

```sh
cp harness.toml.example harness.toml
scripts/fetch-minotari-cli.sh .bench-cache tools
export HARNESS_WALLET_PW='replace-with-a-long-local-password'
cargo run -- addresses --config harness.toml --out .secrets/seeds.env
```

Then fund the printed addresses on Esmeralda, run `preflight`, and execute the baseline run:

```sh
source .secrets/seeds.env
cargo run -- preflight --config harness.toml
cargo run -- preflight --config harness.toml --check-funds
cargo run --features live-minotari -- run --config harness.toml --profile baselines/esmeralda_baseline.json
```

Full operator detail is in [RUNBOOK.md](RUNBOOK.md).

The committed baseline JSON is schema-valid harness output and deliberately
contains no secrets. It includes final funding evidence, all live topology flags,
no live caps, the fresh-scan matrix, independent Mode 2 base-node transaction
queries, and confirmed PP chain evidence from the partial S4-ramp run. It is not
an all-ok, complete S4-reference, or three-repetition statistical baseline:
current live stateful send paths record one repetition, and the no-cap Mode 1/2
runs expose wallet pending-funds behavior in the result profile. A final
submission rerun still needs `concurrent_batches = [8, 16, 32, 64, 128]`,
`repetitions = 1`, all live caps at `0`, and clean spendable Mode 1, Mode 2, and
PP wallets.
