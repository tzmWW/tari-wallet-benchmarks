# Tari Wallet Benchmarks

Reproducible Esmeralda benchmark harness for:

- `minotari_console_wallet` over gRPC
- the `minotari` Rust library with offline signing
- `minotari_payment_processor` batch payments

The canonical protocol is `B0,S0,S1,S2,S3,S4,S5,S6,S7`. Wallet rejection,
locking, contention, stalls, and timeouts are measured outcomes. The harness
does not retry scenario transactions or pre-partition UTXOs.

The current uncapped Esmeralda profile is
[`baselines/esmeralda_baseline.json`](baselines/esmeralda_baseline.json), from
completed candidate `baseline-20260730T220138Z` at measurement commit
`fc37c668b3ff1ccbe94e343d80a7065db9fa0cf6`. Its SHA-256 is
`e00d8f9ea4711f825bb81e8af3abfc3a02a9831cc3f3030483aea0b39400a024`.
All 27 cells are present; failed cells remain failed measurements. See the
adjacent generated summary and the
[`2026-07-31 audit`](analysis/baseline-20260731-audit.md).

## Prerequisites

- Rustup; `rust-toolchain.toml` installs the pinned Rust toolchain, `rustfmt`, and `clippy`
- Git, Bash, `curl`, `jq`, `lsof`, `sqlite3`, `protobuf-compiler`, and standard C/C++ build tools
- Node.js/npm only for installing `@ast-grep/cli`
- An unpruned, synchronized Esmeralda HTTP wallet-query endpoint
- Optionally, a second public Esmeralda endpoint for cross-node anchor checks
- A separate funded source wallet DB for the one external S0 funding transaction

macOS:

```sh
xcode-select --install
brew install rustup git jq protobuf sqlite3 node
npm install --global @ast-grep/cli
```

Ubuntu/Debian:

```sh
sudo apt-get update
sudo apt-get install -y build-essential clang cmake git curl jq lsof protobuf-compiler sqlite3 nodejs npm
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
npm install --global @ast-grep/cli
```

## Fresh Clone

```sh
git clone https://github.com/tzmWW/tari-wallet-benchmarks.git
cd tari-wallet-benchmarks
cp examples/harness-dev.toml harness.toml
scripts/fetch-dev-stack.sh .bench-cache/dev tools
cargo run -- verify-build-manifest --config harness.toml
cargo build --release --features live-minotari
```

Before using the template, follow `RUNBOOK.md` to initialize and synchronize the
local node, then replace `REPLACE_WITH_LOCAL_NODE_PUBLIC_KEY` with its `whoami`
public key. The wallet surfaces and harness use that same node. An optional
`authority_http_url` may be added for cross-node tip and finalized-hash checks.

The development fetcher resolves Minotari `main`, the newest Tari prerelease, and
payment-processor `main` once per invocation. It records their exact commits and
trees, applies only the password-input and fee-rate integration patches, builds
the runtime binaries, and writes their artifact hashes to
`tools/build-manifest.json`. Moving development refs are expected; a patch,
compile, or test failure is a compatibility result rather than a canonical pin
violation. The frozen manifest still prevents source or binary substitution
within a run.

The published baseline remains immutable historical schema-v6 evidence. Its
original fixed sources and patch hashes remain available through the legacy
canonical fetch scripts and the provenance recorded in `build.rs`. They are
historical audit inputs, not the current build path.

### Development Baselines

Use `provenance.policy = "dev"` with `examples/harness-dev.toml`. Dev manifests
record the requested moving refs, resolution timestamp, exact resolved commits,
result trees, patch hashes, and runtime artifact hashes. They are validated as
internally consistent but are not compared with the published baseline pins.
New profiles use schema v7, which permits the single-node topology; validation
continues to accept the historical schema-v6 baseline.
Commit `Cargo.lock` and any required harness API adaptation produced by a new
resolution before starting a measured run; measured candidates reject dirty
harness checkouts.

At this revision, `Cargo.lock` resolves Minotari `main` to `322a901c` and the Tari
API/runtime line is `v5.6.0-pre.1`. The last verified dev build on 2026-08-07
resolved payment-processor `main` to `f0572c9`. These are not permanent allowlist
pins; the dev fetcher resolves the moving refs again and freezes their full
commits in each run manifest.

### Local Baselines

The published baseline uses `provenance.policy = "canonical"` and the immutable
historical revisions documented in `RUNBOOK.md`. To benchmark a different
explicitly selected Tari/Minotari stack, set
`provenance.policy = "local"`, pin exact commits in `[versions]`, and point
`[paths]` at binaries built from those commits. Do not use a moving branch or
`HEAD` as a recorded revision.

After building all four runtime artifacts, generate their manifest from clean,
committed source checkouts:

```sh
cargo run -- create-local-manifest \
  --config harness.toml \
  --minotari-source /path/to/minotari-cli \
  --console-wallet-source /path/to/tari-console-wallet-checkout \
  --node-source /path/to/tari-node-checkout \
  --payment-processor-source /path/to/minotari-payment-processor
```

The command resolves every configured revision to its checkout commit, records
the repository and Git tree, hashes each runtime artifact, and writes
`paths.build_manifest`. Committed compatibility changes should live in a fork or
branch so the selected commit and tree capture them. Separate Tari checkouts are
needed when the console wallet and node use different revisions. Commit the
harness dependency/API adaptation as well; measured runs reject a dirty harness
checkout.

Local and dev runs still fail on dirty source checkouts, revision mismatches, stale
manifests, changed artifact bytes, unsafe network topology, or invalid result
data. They produce final profiles and deterministic summaries using normal
validation, but are explicitly marked `provenance_policy: local` or `dev` and
cannot pass `validate-profile --submission`. The canonical policy is retained
only for the historical published profile; this development-linked harness
refuses new canonical measurements.

For a local node, set `network.base_node_http_url` to its HTTP endpoint and set
`network.mode1_base_node_service_peer` to `public_key::multiaddr`. The harness
requires an archival synchronized selected endpoint. If `authority_http_url` is
configured, it additionally compares tip distance and finalized hashes.

## Candidate Workflow

Use a new `paths.data_dir`, matching `modes.new_wallet_database`, and new seed env
file for every candidate. Keep at least 20 GiB free on the candidate volume.

```sh
mkdir -p .secrets candidates
cargo run --release -- addresses \
  --config harness.toml \
  --out .secrets/candidate.env
set -a
. .secrets/candidate.env
set +a
read -r -s HARNESS_WALLET_PW
export HARNESS_WALLET_PW

cargo run --release --features live-minotari -- baseline-workflow \
  --config harness.toml \
  --source-db /absolute/path/to/source-wallet.db \
  --b0-profile candidates/prefunding-b0.json \
  --s0-evidence candidates/s0-funding.json \
  --profile candidates/esmeralda-dev.json \
  --summary candidates/esmeralda-dev.md
```

The workflow performs disk/build-manifest checks once, then runs B0, resumable S0
funding, recipient synchronization/readiness, the benchmark, policy-aware final
validation, and summary generation in one process. Dev and local profiles receive
normal schema validation; only the immutable historical canonical profile is
eligible for `--submission`. `fund-s0` still writes a broadcast checkpoint
atomically before waiting for `C_min`; the standalone stage commands remain
available for diagnosis and interrupted-funding recovery.

The operator funds exactly one source wallet, not any benchmark-mode address.
After all three empty-wallet B0 scans pass, the harness automatically broadcasts
one transaction containing three `A_fund` outputs, one to each fresh mode seed,
and waits for recipient readiness. The source wallet is not measured; its shared
funding fee is disclosed but not deducted from any mode balance.

Do not use old namespaces, copied wallet DBs, or `--fresh-data-dir`. The harness
locks the candidate namespace, rejects dirty PP/signer state, stores child logs
under the namespace, and terminates managed process groups on SIGINT/SIGTERM.

See `RUNBOOK.md` for protocol and recovery details.

## License

BSD-3-Clause. See `LICENSE`.
