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

By default, live profile generation will verify the funded Mode 2 wallet balance
but will not spend funds and will not run the long fresh-scan matrix. Enable the
extra live gates intentionally in `[benchmark]`. The scenario caps default to
`0`, which means "use the configured benchmark size"; set small positive caps
for compatibility or development runs.

```toml
live_fresh_scan_cells = true    # long-running B0/S2/S3 fresh database scans

mode1_live_topology = true      # runs real minotari_console_wallet with gRPC
mode1_scenario_amount = "1 T"
mode1_live_max_s1_txs = 1       # 0 means full doubling/fanout target
mode1_live_max_s4_batch = 1     # 0 means use each concurrent_batches value
mode1_live_max_s5_items = 2     # 0 means use S5_M
settle_cooldown_secs = 60       # cooldown between S5 arms
# settle_wait_blocks omitted means max(C_min + 1, 4)

mode2_send_smoke = true         # spends mode2_send_smoke_amount once
mode2_send_smoke_amount = "1 T"

mode2_live_scenarios = true     # spends via Mode 2 S1/S4/S5 cells
mode2_scenario_amount = "1 T"
mode2_live_max_s1_txs = 2       # 0 means full doubling/fanout target
mode2_live_max_s4_batch = 2     # 0 means use each concurrent_batches value
mode2_live_max_s5_txs = 2       # 0 means use S5_M

mode3_live_topology = true      # runs real PP plus minotari payment receiver
mode3_scenario_amount = "1 T"
mode3_live_max_s1_batches = 1   # 0 means full doubling/fanout target
mode3_live_max_s4_batch = 1     # 0 means use each concurrent_batches value
mode3_live_max_s5_items = 2     # 0 means use S5_M
mode3_worker_sleep_secs = 1     # PP worker cadence during live runs
```

```sh
cargo run --features live-minotari -- run \
  --config harness.toml \
  --profile baselines/esmeralda_baseline.json
```

The result profile is written atomically and does not contain seed phrases or
passwords. Public addresses may appear in the profile.

Implementation note: the committed harness currently writes the full result
profile shape and can exercise Mode 2 plus PP companion fresh scan paths when
`live_fresh_scan_cells` is enabled. The `[benchmark].scan_batch_size` setting
controls how many blocks each HTTP scan request fetches; larger values make
full-chain scan cells practical on Esmeralda. These fresh scan cells deliberately
wipe their local databases per repetition, so they are long-running and print
per-cell progress while they execute.

Fresh scan cells are checkpointed against actual scenario progress. B0 uses an
empty genesis seed before spends. S2/S3 run only after an S1 checkpoint exists;
S6/S7 run only after an S5 checkpoint exists. If the prerequisite spend is
blocked, the scan cell records a note instead of inventing a measurement. Mode 1
fresh scans launch a real `minotari_console_wallet --recovery` instance with a
fresh base path and poll its gRPC state until it reaches the public tip. Mode 2
and PP companion scan cells use the minotari scanner path with fresh databases.

When `mode2_send_smoke` is enabled, the harness constructs, signs, persists, and
submits one one-sided transaction from the Mode 2 wallet using a direct JSON-RPC
request. This avoids `WalletHttpClient::new`, whose default transport retries
transient failures. `mode2_send_smoke` and `mode2_live_scenarios` are mutually
exclusive.

When `mode1_live_topology` is enabled, the harness starts a real
`minotari_console_wallet` process with gRPC enabled, waits for recovery to find
the funded balance, drives S1 through gRPC `CoinSplit`, and drives S4/S5 through
`Transfer` requests. The console
wallet seed-recovery path reads the birthday embedded in the mnemonic; it does
not apply the separate `--birthday` flag to seed words. The harness therefore
rewrites only the mnemonic birthday before launch. This preserves the address and
keys while avoiding an accidental genesis scan for freshly funded Esmeralda
benchmark wallets.

Do not treat deleting `.bench-data/old-wallet-console` as a harmless reset after
that seed has already made live sends. In a 2026-06-24 proof, fresh recovery of a
previously spent old-wallet seed imported historical matching outputs but did not
fully restore their spent state before the next `CoinSplit`; the wallet selected
the original funding output, and the base node rejected the tx as `AlreadyMined`.
For iterative proofs, keep a wallet DB that has validated through the latest live
sends, or fund a fresh seed. If a proof is interrupted after such a false spend,
restart once from the same DB so validation can mark the stale output spent before
the next benchmark send.

Mode 1 S1 follows the spec round shape: six doubling rounds and one fan-out
round, capped only by `mode1_live_max_s1_txs` for development runs. Between
planned spend rounds the harness waits for chain/scanner height advancement;
this is a settlement gate, not a retry. S5 uses deterministic distinct
Esmeralda recipients and records both batch-shaped and individual-shaped arms.
If earlier S1/S4 sends have locked the single large funded UTXO or change
output, S5 can fail with `Funds are still pending`; that is recorded as wallet
behavior rather than retried or hidden.

When `mode2_live_scenarios` is enabled, the harness records Mode 2 S1, S4, and
S5 through the direct minotari crate path:

- S1 follows the same doubling/fan-out round plan as Mode 1, capped by
  `mode2_live_max_s1_txs` when non-zero. It uses the pinned
  `OneSidedTransaction` multi-recipient builder with the Mode 2 wallet's own
  address as the recipient, so later Mode 2 scan cells rediscover outputs in
  the measured wallet instead of draining the wallet to another mode.
- Between Mode 2 S1 rounds and between S4 and S5, the harness runs the wallet
  scanner and waits for the public base-node HTTP `/get_tip_info` height to
  advance by `settle_wait_blocks`. The wallet scan is still run on each attempt,
  but the base-node tip is the chain-advance clock because wallet scan-tip
  metadata can update in coarse buckets. This is a settlement gate for the known
  `FundsPending` lock behavior, not a retry.
- After S5, any Mode 2 tx ids produced by S1/S4/S5 are re-queried until every
  observed tx reaches `C_min` or the confirmation timeout expires. The harness
  replaces the original single repetition in place and removes any stale
  top-level rows for that Mode 2 scenario before adding newly confirmed rows, so
  the refresh does not create fake repetitions or duplicate chain evidence.
- S4 dispatches each configured concurrent batch against the same wallet
  database, capped by `mode2_live_max_s4_batch` when non-zero. Wallet lock
  contention and failed sends are counted as benchmark signal.
- S5 measures the Mode 2 individual-send arm against deterministic distinct
  Esmeralda recipients, capped by `mode2_live_max_s5_txs` when non-zero. The PP
  Mode 3 surface is responsible for the payment-batch arm.

For every submitted Mode 2 tx id, the harness reads
`completed_transactions.serialized_transaction`, deserializes the transaction,
extracts the first kernel excess signature nonce/signature, and queries the
public base node at
`/transactions?excess_sig_nonce=...&excess_sig_sig=...`. It also reads
`/get_tip_info` and emits a top-level
`chain_verification.verified_transactions` row only when the base-node location
is `Mined` and the mined height is at least `C_min` deep. Wallet DB status,
payment id/payref, fee, query location, mined height, tip height, and query
errors remain in per-repetition metrics. Pending, mempool-only, timeout, and
query-failed cases are observations, not confirmed chain evidence.

For fresh-funded Mode 2 evidence, fund several independent small UTXOs when you
need S1, S4, and S5 in the same run. A single fresh UTXO can prove S1 send-side
construction/broadcast, but S1 may lock the only spendable input before S4/S5
begin. If that happens, S4/S5 should remain funding-state failures in the
profile rather than being retried against synthetic wallet state.

When `mode3_live_topology` is enabled, the harness starts a real
`minotari_payment_processor` process plus a parallel `minotari daemon` payment
receiver. The PP companion view wallet is initialized with a current birthday
when the generated PP seed is genesis-dated, so fresh benchmark funding does not
force an accidental genesis scan. Before the daemon starts, the harness expires
and unlocks stale payment-receiver locks left by previously interrupted local
runs; it does not unlock between scenarios inside a run.

Mode 3 S1/S4/S5 drive `/v1/payment-batches` with the configured caps. S1 is a
PP batch-shape analogue to the doubling/fan-out plan: each planned S1 tx becomes
one PP batch, and `outputs_per_tx` becomes payments per batch. S5 uses the same
deterministic distinct-recipient pool shape as the other modes, grouped by
`S5_K`. PP DB observations are labeled `payment_processor_db_observed`; only
confirmed PP batches are emitted as top-level chain-verification rows. Pending
PP batches remain in metrics/notes. With a single large funded UTXO, the first
signed/broadcast PP batch can lock the wallet change while it waits for
confirmation, and later PP batches may remain `PENDING_BATCHING` with worker logs
reporting insufficient available funds. That is real topology behavior and is
preserved as benchmark signal.

PP has no direct scan API. When `live_fresh_scan_cells=false`, PP scan-shape
cells are marked `not_applicable`; when enabled, those cells measure the
companion minotari wallet scan surface.

## Schema

```sh
cargo run -- schema --out RESULT_PROFILE_SCHEMA.json
```

The JSON profile is designed for automated comparison. Every profile records the
network, hardware environment, pinned versions, benchmark parameters, per-mode
scenario cells, findings, and chain-verification status value. Schema v3 adds
per-repetition `metrics` for scenario-specific values such as S1 round details,
S4 serialization gaps, S5 recipient shape, observed-but-unconfirmed DB rows, and
confirmed chain-verification rows with amount/fee/mined-height fields when the
wallet surface exposes them. It also allows verification-source notes,
base-node query observations, scan checkpoints, birthdays, tip start/end,
blocks scanned per second, detected outputs, and available balance observations.
Environment capture includes OS, CPU, memory, disk kind/name, base-node host,
and whether the base-node path is local or remote.

## Verification Gates

Before publishing a result profile, run:

```sh
cargo fmt --check
cargo check --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo test
ast-grep scan
git diff --check
```

The AST rules intentionally block harness-level retry, backoff, throttling,
scenario dispatch sleeps, and hidden UTXO pre-partitioning in source code.
