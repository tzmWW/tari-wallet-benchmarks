# Benchmark Runbook

## Safety Rules

- Use fresh seeds and a unique `paths.data_dir` for every candidate.
- Run B0 before any benchmark address is funded.
- Fund all three benchmark addresses once from a separate source wallet.
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
  (upstream `360c4848a54d65fd710266233cc9277b0f785e74` plus fixed-range HTTP scanner completion ordering and clipping)
- console wallet: `9f5adb7183dc2ec285f5c8fae05f4be9735d9749`
- node: `v5.4.0`
- payment processor: `f0572c98cbfac7377412dc6d4094c7d7dfc5de2c`

The PP build applies `patches/payment-processor-fee-rate.patch`; this makes the
worker consume `FEE_PER_GRAM`, which the harness derives from `benchmark.fee_rate`.

## B0 and Funding

Start from `harness-prefunding.toml`. Change only the candidate data paths and,
for a local node, the network values above. Do not add funding records.

```sh
cp harness-prefunding.toml harness.toml
mkdir -p .secrets candidates
cargo run --release -- addresses --config harness.toml --out .secrets/candidate.env
set -a; . .secrets/candidate.env; set +a
export HARNESS_WALLET_PW='local-password'

cargo run --release --features live-minotari -- baseline-workflow \
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
broadcasts one three-recipient transaction. It atomically records the tx ID and
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
