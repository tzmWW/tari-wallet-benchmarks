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

The Minotari fetch script builds matching `minotari`, `minotari_console_wallet`,
and `minotari_node` binaries into `tools/`. If you supply your own binaries,
edit `[paths]` in `harness.toml` to point at them.

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

For a submission-clean run, fund each generated address with exactly one
spendable `A_fund` output from `harness.toml` (`10000 T` by default). Funding is
intentionally outside the measured benchmark. Wait until the funding output has
at least `C_min` confirmations.

After funding, record each tx in `[funding.<mode>]` in `harness.toml` with the
amount, transaction id, and block height. These fields are written to result
profiles as public benchmark inputs.

Before starting live spend cells, audit spendability rather than trusting a
single balance display. `preflight --check-funds` and S0 are intentionally strict
for final profiles: one spendable output and available balance exactly equal to
`A_fund`. Extra spendable outputs, locked outputs, pending rows, invalid rows, or
unknown statuses mean the starting state is not submission-clean.

You can inspect raw output-status totals while debugging:

```sh
sqlite3 .bench-data/new-wallet-fresh-proof/wallet.db \
  "select status, count(*), coalesce(sum(value), 0) from outputs group by status;"
```

The original Mode 2 DB can be polluted by locked/spent proof state. For final
evidence, prefer a fresh seed and fresh DB backed up immediately before the run.
If the scanner stalls just before the funding height, advance the backed-up
ignored DB with supported short rescan/scan steps rather than editing wallet DB
state by hand.

Do not spend from the same Mode 2 DB that you intend to use as final benchmark
evidence unless the post-send change is independently proven spendable. A
2026-06-25 fresh-Mode-1 funding smoke sent tx `9744132983940844747` from the
Mode 2 wallet; the public base-node query showed it mined at height `711891` and
reached `C_min`, but the sender DB still held the large input/change as
`LOCKED` after supported scan/rescan. That DB is useful proof of funding the
fresh Mode 1 seed, not a clean Mode 2 source for a no-caps run.

For the final no-caps pass, fund Mode 1, Mode 2, and PP as independent spendable
wallets before starting the profile. Avoid copying old console-wallet DBs with
historical unmined/rejected rows into a new run directory; fund a fresh Mode 1
seed and confirm the recipient wallet sees the mined output instead.

For development proofs or recoup work, recovered minotari signing wallets can
create multi-output one-sided funding transactions without using the benchmark
scenario path:

```sh
source .secrets/seeds.env
cargo run --features live-minotari -- fund-one-sided \
  --config harness.toml \
  --source-db .bench-data/recovered-source/wallet.db \
  --recipient f2... \
  --recipient f2... \
  --amount "70 T" \
  --outputs 150 \
  --batch-size 50
```

This is an operator funding tool, not a measured benchmark step. Do not use it to
pre-partition the final benchmark starting state; the final profile should begin
from one `A_fund` UTXO per mode and let wallet lock contention show up naturally.
Wait for submitted funding transactions to mine and reach `C_min`, scan the
recipient DB, then run `preflight --check-funds` before starting a benchmark
profile.

`fund-one-sided` accepts repeated `--recipient` values for controlled recoup or
setup sweeps. Keep those uses in ignored operator manifests; do not encode them
as benchmark scenario behavior.

Two live-only diagnostics are also available for recovery and audit work:

```sh
cargo run --features live-minotari -- recover-mode1-wallet --config harness.toml

cargo run --features live-minotari -- query-tx \
  --config harness.toml \
  --db .bench-data/final-local-20260702T151225Z/new-wallet/wallet.db \
  --tx-id 7094477815543133352
```

`recover-mode1-wallet` performs a supported console-wallet recovery path for the
configured Mode 1 wallet. `query-tx` prints wallet DB status plus independent
base-node transaction-query evidence for a Mode 2 transaction id.

## Preflight

```sh
source .secrets/seeds.env
export HARNESS_WALLET_PW='replace-with-a-long-local-password'
cargo run --features live-minotari -- preflight --config harness.toml
cargo run --features live-minotari -- preflight --config harness.toml --check-funds
```

Preflight validates the Esmeralda-only guard, seed material, wallet password env,
and local binary paths. It prints the PP build command if the PP binary is
missing. With `--check-funds`, it also audits wallet DB output-status totals,
proves the configured console-wallet address over Mode 1 gRPC, proves the
minotari account fingerprints, checks scanner/selected-chain state, and fails
unless each configured live wallet has exactly one spendable output with value
exactly equal to `A_fund`, and no pending, encumbered, invalid, cancelled,
not-stored, or unknown outputs. Text minotari statuses and numeric console-wallet
statuses are printed with labels. Use `--mode1-db`, `--mode2-db`, and
`--payment-receiver-db` to audit recovered DBs before updating `harness.toml`.

## Local Base Node

Final evidence should prefer a local synced Esmeralda base node over the public
RPC endpoint. `get_tip_info.is_synced=true` is not sufficient by itself: a local
node can report synced while stale if seed peers are banned or marked offline.
Compare the local height with a public tip and prove the funding block is
queryable locally before starting the benchmark.

The local node path that produced the July 2026 baseline used
`.bench-data/local-base-node-54` and HTTP wallet-query service
`http://127.0.0.1:18142`. If sync stalls with peer bans or `NetworkSilence`, run
the maintenance commands with the same base path:

```sh
tools/minotari_node -b .bench-data/local-base-node-54 \
  --network esmeralda --non-interactive-mode --disable-splash-screen \
  --watch "unban-all-peers"

tools/minotari_node -b .bench-data/local-base-node-54 \
  --network esmeralda --non-interactive-mode --disable-splash-screen \
  --watch "reset-offline-peers"
```

Then restart the node with explicit Esmeralda TCP seed peers, `base_node.use_libtor=false`,
`base_node.http_wallet_query_service.port=18142`, long blockchain-sync RPC
deadlines, short ban durations, and `base_node.storage.pruning_horizon=0`.
Avoid `-b .bench-data/local-base-node-54/esmeralda`; that creates a nested
`esmeralda/esmeralda` data directory.

Local funding proof for the July 2026 baseline used funding tx
`5740188747787224553` at height `725415`, header hash
`84821095ce94cf98a88932bb287bcb09f0a641d48ab29f70663481fff4addbf2`. A local
proof query should return the funding outputs:

```sh
curl -fsS \
  "http://127.0.0.1:18142/get_utxos_by_block?header_hash=84821095ce94cf98a88932bb287bcb09f0a641d48ab29f70663481fff4addbf2"
```

## Run

By default, live profile generation will verify the funded Mode 2 wallet balance
but will not spend funds and will not run the long fresh-scan matrix. Enable the
extra live gates intentionally in `[benchmark]`. The scenario caps default to
`0`, which means "use the configured benchmark size"; set small positive caps
for compatibility or development runs.

```toml
live_fresh_scan_cells = true    # long-running B0/S2/S3 fresh database scans
scan_repetitions = 1            # scan cells; live send cells currently emit one repetition

mode1_live_topology = true      # runs real minotari_console_wallet with gRPC
mode1_payment_amount = "1 T"
mode1_live_max_s1_txs = 1       # 0 means full doubling/fanout target
mode1_live_max_s4_batch = 1     # 0 means use each concurrent_batches value
mode1_live_max_s5_items = 2     # 0 means use S5_M
settle_cooldown_secs = 60       # cooldown between S5 arms
# settle_wait_blocks omitted means max(C_min + 1, 4)

mode2_send_smoke = true         # spends mode2_send_smoke_amount once
mode2_send_smoke_amount = "1 T"

mode2_live_scenarios = true     # spends via Mode 2 S1/S4/S5 cells
mode2_payment_amount = "1 T"
mode2_live_max_s1_txs = 2       # 0 means full doubling/fanout target
mode2_live_max_s4_batch = 2     # 0 means use each concurrent_batches value
mode2_live_max_s5_txs = 2       # 0 means use S5_M

mode3_live_topology = true      # runs real PP plus minotari payment receiver
mode3_payment_amount = "1 T"
mode3_live_max_s1_batches = 1   # 0 means full doubling/fanout target
mode3_live_max_s4_batch = 1     # 0 means use each concurrent_batches value
mode3_live_max_s5_items = 2     # 0 means use S5_M
mode3_worker_sleep_secs = 10    # PP worker cadence during live runs
```

```sh
PROFILE="candidates/esmeralda-$(date -u +%Y%m%dT%H%M%SZ).json"
cargo run --features live-minotari -- run \
  --config harness.toml \
  --profile "$PROFILE"
```

The result profile is written atomically and does not contain seed phrases or
passwords. Public addresses may appear in the profile.

Current caveat: `benchmark.repetitions` is recorded in the config metadata, but
the live stateful send implementations still record one observed repetition per
scenario. The local-node profile generated at `2026-07-02T22:16:39.401016Z` uses
the full S4 ramp and no live caps. It is intentionally strict: send cells that hit
single-UTXO pending-funds/PP contention are failed observations, and scan cells
whose scanner DB height did not reach the local tip within `C_min` blocks are
failed observations rather than silently treated as successful scans.

Before the final submission rerun, set the live config to the full reference S4
ramp and one stateful repetition unless live repetition loops have been funded
and implemented:

```toml
concurrent_batches = [8, 16, 32, 64, 128]
repetitions = 1
scan_repetitions = 1
live_fresh_scan_cells = true
mode1_live_max_s1_txs = 0
mode1_live_max_s4_batch = 0
mode1_live_max_s5_items = 0
mode2_live_max_s1_txs = 0
mode2_live_max_s4_batch = 0
mode2_live_max_s5_txs = 0
mode3_live_max_s1_batches = 0
mode3_live_max_s4_batch = 0
mode3_live_max_s5_items = 0
```

The July 2 pre-run backup
`.bench-data/_backups/pre-final-local-20260702T221606Z/final-local-20260702T151225Z`
is historical starting-state evidence only. Later recoup attempts touched the
backup Mode 2 and PP DBs, so another final submission run must use freshly funded
seed material rather than restoring that backup as live state. The current fresh
prep artifact is `.bench-data/final-submit-20260703T224503Z/RUN_PREP.md`.

Long live runs also write checkpoint profiles next to the requested profile path,
for example `baselines/esmeralda_baseline.old_wallet.json`,
`baselines/esmeralda_baseline.new_wallet.json`,
`baselines/esmeralda_baseline.fresh_scans.json`, and
`baselines/esmeralda_baseline.payment_processor.json`. These preserve completed
stage evidence if an unattended run is interrupted; the final merged profile is
still written atomically to the requested `--profile` path.

Use `run --fresh-data-dir --yes` only when intentionally starting from clean data
directories. It deletes enabled mode data dirs before the run, so the generated
addresses must be funded again before `preflight --check-funds` can pass.

Implementation note: the committed harness currently writes the full result
profile shape and can exercise Mode 2 plus PP companion fresh scan paths when
`live_fresh_scan_cells` is enabled. Mode 2 and PP companion scan cells use the
pinned minotari HTTP scanner, which can return from a full-chain scan with a DB
height far below the base-node tip because of an upstream downloader/processor
completion race. The harness therefore records `tip_lag_blocks`,
`tip_lag_tolerance_blocks`, and `scan_reached_tip`, and marks the scan failed if
`max_height + C_min < tip_end`. `scan_to_tip` uses bounded partial scan chunks to
improve progress before recording the observation, but a below-tip scan remains a
failed benchmark measurement. These fresh scan cells deliberately wipe their
local databases per repetition, so they are long-running and print per-cell
progress while they execute.

Fresh scan cells are checkpointed against actual scenario progress. B0 uses an
empty genesis seed before spends. S2/S3 run only after an S1 checkpoint exists;
S6/S7 run only after an S5 checkpoint exists. If the prerequisite spend is
blocked, the scan cell records an explicit failed repetition with
`blocked_prerequisite = true`, `wall_ms = null`, `success_count = 0`, and
`failure_count = 1` instead of inventing a measurement. Mode 1 fresh scans launch
a real `minotari_console_wallet --recovery` instance with a fresh base path and
poll its gRPC state until it reaches the public tip. Mode 2 and PP companion scan
cells use the minotari scanner path with fresh databases.

When `mode2_send_smoke` is enabled, the harness constructs, signs, persists, and
submits one one-sided transaction from the Mode 2 wallet using a direct JSON-RPC
request. This avoids `WalletHttpClient::new`, whose default transport retries
transient failures. `mode2_send_smoke` and `mode2_live_scenarios` are mutually
exclusive.

When `mode1_live_topology` is enabled, the harness starts a real
`minotari_console_wallet` process with gRPC enabled, waits for recovery to find
the funded balance, and drives S1/S4/S5 through one-shot gRPC `Transfer`
requests. S1 uses an exact no-change self-directed multi-recipient shape. The console
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
round, capped only by `mode1_live_max_s1_txs` for development runs. Every round
reads live spendable values, derives equal no-change children from the pinned
weight/fee shape, settles to `C_min`, and verifies the exact UTXO-count and
fee-only balance delta before advancing. S1, S4, and S5 submit each attempt once:
there is no retry, backoff, or direct wallet-state repair. S5 uses deterministic
distinct Esmeralda recipients. If earlier S1/S4 sends have locked the single
large funded UTXO or change output, S5 can fail with `Funds are still pending`;
that is recorded as wallet behavior rather than retried or hidden.

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
- Each Mode 2 S1 round independently re-queries every submitted transaction
  until it reaches `C_min` or the confirmation timeout expires. A failed round
  blocks later S1 rounds and all dependent cells. S4/S5 preserve the same
  confirmed-only top-level evidence rule without retrying rejected sends.
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

For final Mode 2 evidence, start from the spec's single `A_fund` UTXO and let any
locked-change behavior surface in S4/S5. Development runs can use additional
funded UTXOs to prove plumbing, but those profiles should be labeled as
non-final because they do not match the benchmark starting state.

The V5 capped proof used `mode2_scenario_amount = "0.02 T"` and one live S1, S4,
and S5 attempt against six small fresh UTXOs. That produced confirmed
`base_node_transaction_query` rows for all three Mode 2 stateful cells while
leaving the full-volume and repeated statistical runs for a separate evidence
pass.

When `mode3_live_topology` is enabled, the harness starts a real
`minotari_payment_processor` process plus a parallel `minotari daemon` payment
receiver. The PP companion view wallet is initialized with a current birthday
when the generated PP seed is genesis-dated, so fresh benchmark funding does not
force an accidental genesis scan. Dirty or locked receiver state is a preflight
failure: the harness never expires or unlocks it directly.

Mode 3 S1/S4/S5 drive `/v1/payment-batches` with the configured caps. S1 is a
PP batch-shape analogue to the doubling/fan-out plan: each planned S1 tx becomes
one PP batch, and `outputs_per_tx` becomes payments per batch. S5 uses the same
deterministic distinct-recipient pool shape as the other modes, grouped by
`S5_K`. PP DB observations are labeled `payment_processor_db_observed`; only
confirmed PP batches with a real signed-transaction kernel, fee, mined height,
and independent base-node `C_min` proof are emitted as top-level
chain-verification rows. Pending PP batches remain in metrics/notes. The harness waits up to
`[timeouts].confirmation_secs` for accepted PP batches to reach terminal DB
status before recording the cell; `AWAITING_CONFIRMATION` and signed-but-not-yet
confirmed batches are not counted as done. With a single large funded UTXO, the
first signed/broadcast PP batch can lock the wallet change while it waits for
confirmation, and later PP batches may remain `PENDING_BATCHING` with worker logs
reporting insufficient available funds. That is valid benchmark signal for the
spec's single-UTXO starting state. Multi-output PP funding is useful for capped
development proofs, not for the final reference profile.

PP has no direct scan API. When `live_fresh_scan_cells=false`, PP scan-shape
cells are marked `not_applicable`; when enabled, those cells measure the
companion minotari wallet scan surface.

## Schema

```sh
cargo run -- schema --out RESULT_PROFILE_SCHEMA.json
```

The JSON profile is a real Draft 2020-12 schema-v4 document. It records a stable
run identity, checkpoint/final state, harness commit, base-node endpoint and
anchors, resolved funding birthdays, and separate execution/outcome statuses for
every cell. Per-repetition metrics include common transaction observations,
strict S0 checks, scan resource/expectation evidence, balance reconciliation,
and S5 arms. The top-level `computed_deltas` section derives scan deltas and
S5 throughput ratios only from complete source arms.
Each transaction observation carries the submitted transaction or PP batch
identity when the surface returned one. Its `confirmation_ms` is the enclosing
scenario/arm wall time from first dispatch through terminal `C_min` observation
or timeout; mempool timestamps remain explicitly unavailable where the wallet
surface does not expose them.
Environment capture includes OS, CPU, memory, disk kind/name, base-node host, and
whether the base-node path is local or remote.

## Verification Gates

Before publishing a result profile, run:

```sh
cargo fmt --check
cargo check --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo test
ast-grep scan
git diff -- RESULT_PROFILE_SCHEMA.json # empty after `schema`
git diff --check
```

The AST rules intentionally block harness-level retry, backoff, throttling,
scenario dispatch sleeps, direct wallet-state repair, and hidden UTXO
pre-partitioning in source code. Validate the candidate and derive its Markdown
summary only after the run is complete:

```sh
cargo run -- validate-profile --profile candidate.json --submission
cargo run -- summarize-profile --profile candidate.json --out candidate.md
```
