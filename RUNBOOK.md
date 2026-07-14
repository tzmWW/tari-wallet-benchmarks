# Benchmark Runbook

## Safety Rules

- Use fresh seeds and a unique `paths.data_dir` for every candidate.
- Run B0 before any benchmark address is funded.
- Fund only one separate source wallet. Never fund benchmark addresses manually.
- Let `baseline-workflow` finish all three B0 scans before it automatically sends
  one transaction with three `A_fund` outputs to the fresh mode seeds.
- Never edit wallet databases, unlock outputs, retry scenario transactions, or
  normalize state between S4 and S5.
- A failed wallet operation is a result. A harness, provenance, schema, or
  environment failure invalidates the candidate.
- Keep AC power and sleep prevention enabled for a full run.

## Topology

The preferred final topology uses a pinned unpruned local Esmeralda node for
wallet scans and operations, plus public Esmeralda as independent authority.
The local node only reduces request latency; it cannot accelerate blocks or
`C_min`.

Set:

```toml
[network]
name = "esmeralda"
base_node_http_url = "http://127.0.0.1:18142"
authority_http_url = "https://rpc.esmeralda.tari.com"
mode1_base_node_service_peer = "PUBLIC_KEY::/ip4/127.0.0.1/tcp/18189"
```

Strict checks require `pruning_horizon=0`, selected/authority tip distance at
most `C_min`, matching finalized hashes, queryable funding headers, selected-
chain unspent funding outputs, and current wallet scanner heights.

### Local Node Setup

Initialize the pinned node in its own base path:

```sh
tools/minotari_node \
  --base-path .bench-data/esmeralda-node \
  --network esmeralda \
  --init
```

Stop it after it creates
`.bench-data/esmeralda-node/esmeralda/config/config.toml`. In that file, set the
following values in the existing sections:

```toml
[base_node]
use_libtor = false

[base_node.storage]
pruning_horizon = 0

[base_node.p2p.transport]
type = "tcp"
tcp.listener_address = "/ip4/127.0.0.1/tcp/18189"

[base_node.http_wallet_query_service]
port = 18142
listen_ip = "127.0.0.1"
external_address = "http://127.0.0.1:18142"
```

Start it and leave it running:

```sh
tools/minotari_node \
  --base-path .bench-data/esmeralda-node \
  --network esmeralda
```

At the node prompt, run `whoami` and copy the displayed public key into
`network.mode1_base_node_service_peer`. Wait for the node to synchronize fully.
Compare `http://127.0.0.1:18142/get_tip_info` with
`https://rpc.esmeralda.tari.com/get_tip_info`; `baseline-workflow` then performs
the authoritative archival, process-hash, tip-distance, and finalized-hash
checks. A stale node that reports `is_synced=true` still fails these checks.

## Build and Verify

Install the dependencies listed in `README.md`, then run:

```sh
scripts/fetch-minotari-cli.sh .bench-cache tools
scripts/fetch-payment-processor.sh .bench-cache tools
cargo fmt --check
cargo check --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo test --all-features
ast-grep scan
```

The fetch scripts pin:

- minotari CLI/scanner: `tzmWW/minotari-cli@1391dbd2155c96e885379d72b76e33582f0aad87`
- console wallet: `9f5adb7183dc2ec285f5c8fae05f4be9735d9749`
- node: `v5.4.0`
- payment processor: `f0572c98cbfac7377412dc6d4094c7d7dfc5de2c`

The minotari pin is exactly two commits ahead of upstream `360c4848`:

- `c2b8d7b` makes the processor, rather than the downloader, publish fixed-range
  scan completion after queued blocks are persisted; it also fixes inclusive
  range arithmetic and clips responses at the requested end height. Without it,
  two pre-funding runs stopped at batch boundaries and could not prove the shared
  exact B0 anchor.
- `1391dbd` adds atomic caller-selected output locking and exact-shape fee
  estimation. S1 must bind every planned 1-input transaction to a specific
  parent and consume it without change; the upstream selection API cannot express
  that invariant and previously selected a different input or under-estimated a
  fee. The full comparison is
  `https://github.com/tzmWW/minotari-cli/compare/360c4848a54d65fd710266233cc9277b0f785e74...1391dbd2155c96e885379d72b76e33582f0aad87`.

The PP build applies `patches/payment-processor-fee-rate.patch` to both ordinary
payment construction and self-spend consolidation. Upstream hard-codes `5`; the
bounty requires the exposed `benchmark.fee_rate` to control every mode, so the
patch makes both paths require the harness-provided `FEE_PER_GRAM`. The fetch
script verifies that this is the only PP source change and records its hash.

### Source Wallet

The only manually funded wallet is a `minotari` signing-wallet SQLite DB. It must
use the same `HARNESS_WALLET_PW` exported for the workflow and hold at least
`3 * A_fund` plus the shared transaction fee. Create one if needed:

```sh
export HARNESS_WALLET_PW='local-password'
export SOURCE_DB=/absolute/path/to/source-wallet.db
tools/minotari --network esmeralda create \
  --password "$HARNESS_WALLET_PW" \
  --database-path "$SOURCE_DB"
```

Start the wallet daemon in a second terminal so its API exposes the address and
keeps the source DB synchronized:

```sh
tools/minotari --network esmeralda daemon \
  --password "$HARNESS_WALLET_PW" \
  --database-path "$SOURCE_DB" \
  --base-url http://127.0.0.1:18142 \
  --batch-size 1000 \
  --scan-interval-secs 10 \
  --api-port 9147
```

From the first terminal, read and fund the returned `address` from Tari Universe
mining, a faucet, or another wallet. Wait until the balance endpoint reports at
least `30000 T` plus fees, then stop the daemon cleanly with Ctrl+C before the
workflow opens the DB:

```sh
curl http://127.0.0.1:9147/accounts/default/address
curl http://127.0.0.1:9147/accounts/default/balance
```

This is setup, not measurement. Do not send funds to the three generated mode
addresses: `baseline-workflow` does that only after B0 succeeds.

## B0 and Funding

Start from `harness-prefunding.toml`. Change the candidate `paths.data_dir` and
the matching `modes.new_wallet_database`; set the local-node identity above. Do
not add funding records.

```sh
cp harness-prefunding.toml harness.toml
mkdir -p .secrets candidates
cargo run --release -- addresses --config harness.toml --out .secrets/candidate.env
set -a; . .secrets/candidate.env; set +a
export HARNESS_WALLET_PW='local-password'

caffeinate -dimsu -- cargo run --release --features live-minotari -- baseline-workflow \
  --config harness.toml \
  --source-db /absolute/path/to/source-wallet.db \
  --b0-profile candidates/prefunding-b0.json \
  --s0-evidence candidates/s0-funding.json \
  --profile candidates/esmeralda-baseline.json \
  --summary candidates/esmeralda-baseline.md
```

`baseline-workflow` runs launch-invariant disk and build-manifest verification
once. Stage-sensitive port, endpoint, wallet-state, identity, and selected-chain
checks still run immediately before the stages they protect. `prepare-b0` binds
the harness commit, resolved protocol, timeouts, topology,
seed/address fingerprints, host/disk environment, endpoint identity, source
revisions, binary/patch hashes, and scan batch size. Because the console wallet
does not expose a stop-height API, its persisted completion cursor establishes
one B0 anchor height/hash. Both library-backed scans must reach that exact same
anchor. `prepare-b0` therefore requires `scan_repetitions = 1` and fails before
scanning otherwise. Every B0 scan must recover zero outputs, balance, and history
there.

`caffeinate` is the macOS sleep inhibitor used for the published run. On Linux,
run the same `cargo` command under the host's equivalent inhibitor, such as
`systemd-inhibit --what=sleep --why="Tari wallet benchmark"`.

If the process is interrupted after B0, resume the same funding transaction with
the standalone stage command:

```sh
cargo run --release --features live-minotari -- fund-s0 \
  --config harness.toml \
  --source-db /absolute/path/to/source-wallet.db \
  --b0-profile candidates/prefunding-b0.json \
  --evidence-out candidates/s0-funding.json
```

The funding stage creates fresh benchmark wallet DBs at one measured birthday and
broadcasts one transaction with three recipients. It atomically records the tx ID and
broadcast timing before confirmation polling. Repeating the command resumes that
tx. Once confirmed, it synchronizes all three recipient DBs and requires the
strict one-output/`A_fund` readiness gate before proceeding. Evidence records the
mined height, `C_min` tip, signed-kernel fee, and birthday start height.

## Candidate Run

No funding values are copied into TOML. The run derives them from evidence and
executes strict non-spending checks before starting scenarios.

The workflow proceeds to the candidate run only after recipient readiness. The
standalone `prepare-b0`, `fund-s0`, and `run` commands remain available for
diagnosis and documented interrupted-funding recovery; each performs its own
launch checks when invoked separately.

Each non-B0 scan captures one immutable target height/hash. Library-backed scans
continue toward that same target when the pinned scanner returns before queued
blocks are persisted; they never replace it with a newer tip. Continuation is
bounded by the original deadline and three consecutive no-progress returns.
Overshoot, cursor-hash mismatch, target reorganization, or deadline expiry fails
the cell. Completion tip/hash and scanner invocation count are recorded as drift
and implementation evidence. Recovered transaction IDs, not only counts, are
checked.

S0 failure blocks S1. S1 halts at the first failed round. S4 emits one observation
per requested call. S5 starts directly from disclosed post-S4 state. S6/S7 scan
the actual post-attempt state even if S5 is partial or fails. Per-transaction
confirmation time is the first independent `C_min` observation or null with a
reason; scenario wall time is never substituted.

## Publication

```sh
cargo run --release -- validate-profile \
  --profile candidates/esmeralda-baseline.json --submission
cargo run --release -- summarize-profile \
  --profile candidates/esmeralda-baseline.json \
  --out candidates/esmeralda-baseline.md
```

Submission validation requires successful B0 and S0, all cells present, no
harness errors, exact canonical configuration, coherent blocked prerequisites,
recomputed deltas, realistic transaction shapes, and complete provenance. Honest
downstream wallet failure is valid evidence.

Only after validation should the candidate JSON and summary replace the files in
`baselines/`. Never promote partial checkpoints or combine modes from separate
runs.

## Interrupted Runs

- Before S0 funding: rerun `prepare-b0` only in a new namespace if the checkpoint
  did not complete.
- During S0 confirmation: rerun the same `fund-s0` command.
- During measured scenarios: preserve the namespace and logs as diagnostics; do
  not resume or reuse mutated benchmark wallets for a candidate.
- SIGINT/SIGTERM triggers managed process-group shutdown. A stale
  `.wallet-bench.lock` means the namespace is non-canonical; preserve it and use a
  new namespace rather than deleting the lock to continue.

`fund-one-sided`, `scan-wallet`, `recover-mode1-wallet`, `sweep-mode1`, and
`query-tx` are noncanonical operator diagnostics/recovery commands. They are not
called by measured scenarios and must not be used to alter a candidate between
cells.
