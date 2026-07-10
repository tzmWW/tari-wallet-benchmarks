# Tari Wallet Benchmarks

Standalone reproducible wallet performance harness for
`tari-project/wallet-benchmarks`.

The harness targets Esmeralda only and models the three required wallet surfaces:

- old wallet: `minotari_console_wallet` process managed by the harness
- new wallet: `minotari` crate APIs for scan, selection, signing, and broadcast
- payment processor: real `minotari_payment_processor` plus a parallel `minotari` payment-receiver wallet

Current implementation status: CLI/config/schema-v4/seed handling, Esmeralda
guarding, result-profile generation, PP environment/API contract, static
"no hidden wallet pain" rules, and live evidence paths for all three wallet
surfaces are in place. The harness writes per-stage checkpoint profiles during
live runs, supports `preflight --check-funds` for wallet DB UTXO audits, and
separates the configured send repetition count from long fresh-scan repetitions
via `scan_repetitions`. Current live stateful send cells still emit one observed
repetition per scenario. It also includes `fund-one-sided` for operator-controlled
Esmeralda funding from a recovered minotari signing wallet; the final benchmark
starting state should still be one clean `A_fund` UTXO per mode. Mode 2 submitted
transactions are independently queried through the public base-node
`/transactions` endpoint by extracting kernel signature data from the wallet DB's
serialized transaction. Top-level chain verification rows are emitted only for
mined transactions that are at least `C_min` deep; PP rows additionally require
the real signed-transaction kernel and independent base-node proof. Result profiles include
computed scan/S5 deltas, strict S0 checks, scan resource peaks, per-scenario
balance reconciliation, and S5 per-arm metrics. Fresh scan cells are
checkpointed: B0 uses an empty genesis seed, S2/S3 run after a valid S1
checkpoint, and S6/S7 run after S5. Mode 1 scan cells use real
`minotari_console_wallet --recovery`; Mode 2 and PP companion scans use fresh
minotari scanner databases. The July 2 profile is historical, non-promotable
evidence: schema-v4 submission validation is required before a candidate can be
accepted. The harness preserves real pending-funds, locked-change, PP contention,
and below-tip scanner failures rather than hiding them.

Start by generating fundable addresses:

```sh
cp harness.toml.example harness.toml
scripts/fetch-minotari-cli.sh .bench-cache tools
export HARNESS_WALLET_PW='replace-with-a-long-local-password'
cargo run -- addresses --config harness.toml --out .secrets/seeds.env
```

The fetch script installs `tools/minotari`, `tools/minotari_console_wallet`, and
`tools/minotari_node` from the pinned source revisions.

Then fund each printed address with one clean `A_fund` Esmeralda UTXO, run
`preflight`, and execute the baseline run:

```sh
source .secrets/seeds.env
cargo run --features live-minotari -- preflight --config harness.toml
cargo run --features live-minotari -- preflight --config harness.toml --check-funds
PROFILE="candidates/esmeralda-$(date -u +%Y%m%dT%H%M%SZ).json"
cargo run --features live-minotari -- run --config harness.toml --profile "$PROFILE"
cargo run -- validate-profile --profile "$PROFILE" --submission
cargo run -- summarize-profile --profile "$PROFILE" --out "${PROFILE%.json}.md"
```

Full operator detail is in [RUNBOOK.md](RUNBOOK.md).

Result profiles contain no seeds or passwords. A candidate is promotable only
after `validate-profile --submission` passes; this requires all 27 cells,
canonical 512-output S1 evidence, zero live caps, independent `C_min` chain
proofs, and no fabricated incomplete-arm S5 comparison. Fund fresh seeds rather
than restoring or repairing spent benchmark DBs, then pass
`preflight --check-funds` before spending.
