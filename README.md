# Tari Wallet Benchmarks

Reproducible Esmeralda benchmark harness for:

- `minotari_console_wallet` over gRPC
- the `minotari` Rust library with offline signing
- `minotari_payment_processor` batch payments

The canonical protocol is `B0,S0,S1,S2,S3,S4,S5,S6,S7`. Wallet rejection,
locking, contention, stalls, and timeouts are measured outcomes. The harness
does not retry scenario transactions or pre-partition UTXOs.

## Prerequisites

- Rustup; `rust-toolchain.toml` installs the pinned Rust toolchain, `rustfmt`, and `clippy`
- Git, Bash, `curl`, `lsof`, `sqlite3`, `protobuf-compiler`, and standard C/C++ build tools
- Node.js/npm only for installing `@ast-grep/cli`
- An unpruned, synchronized Esmeralda HTTP wallet-query endpoint
- A public Esmeralda authority endpoint
- A separate funded source wallet DB for the one external S0 funding transaction

macOS:

```sh
xcode-select --install
brew install rustup git protobuf sqlite3 node
npm install --global @ast-grep/cli
```

Ubuntu/Debian:

```sh
sudo apt-get update
sudo apt-get install -y build-essential clang cmake git curl lsof protobuf-compiler sqlite3 nodejs npm
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
npm install --global @ast-grep/cli
```

## Fresh Clone

```sh
git clone https://github.com/tzmWW/tari-wallet-benchmarks.git
cd tari-wallet-benchmarks
scripts/fetch-minotari-cli.sh .bench-cache tools
scripts/fetch-payment-processor.sh .bench-cache tools
cargo build --release --features live-minotari
cp harness-prefunding.toml harness.toml
```

Before using the template, follow `RUNBOOK.md` to initialize and synchronize the
pinned local node, then replace `REPLACE_WITH_LOCAL_NODE_PUBLIC_KEY` with its
`whoami` public key. Canonical live configuration rejects a remote scan endpoint,
a local authority endpoint, or identical scan/authority URLs.

The second fetch script applies the tracked PP fee patch and writes
`tools/build-manifest.json`. Preflight verifies every source revision, the patch
SHA-256, and each runtime binary SHA-256.

For a local node, set `network.base_node_http_url` to its HTTP endpoint and set
`network.mode1_base_node_service_peer` to `public_key::multiaddr`. Keep
`network.authority_http_url` on public Esmeralda. The harness requires an
archival selected endpoint, compares its finalized hash with the authority, and
rejects stale local nodes.

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
export HARNESS_WALLET_PW='choose-a-local-password'

cargo run --release --features live-minotari -- baseline-workflow \
  --config harness.toml \
  --source-db /absolute/path/to/source-wallet.db \
  --b0-profile candidates/prefunding-b0.json \
  --s0-evidence candidates/s0-funding.json \
  --profile candidates/esmeralda-baseline.json \
  --summary candidates/esmeralda-baseline.md
```

The workflow performs disk/build-manifest checks once, then runs B0, resumable S0
funding, recipient synchronization/readiness, the benchmark, submission
validation, and summary generation in one process. `fund-s0` still writes a
broadcast checkpoint atomically before waiting for `C_min`; the standalone stage
commands remain available for diagnosis and interrupted-funding recovery.

The operator funds exactly one source wallet, not any benchmark-mode address.
After all three empty-wallet B0 scans pass, the harness automatically broadcasts
one transaction containing three `A_fund` outputs, one to each fresh mode seed,
and waits for recipient readiness. The source wallet is not measured; its shared
funding fee is disclosed but not deducted from any mode balance.

Do not use old namespaces, copied wallet DBs, or `--fresh-data-dir`. The harness
locks the candidate namespace, rejects dirty PP/signer state, stores child logs
under the namespace, and terminates managed process groups on SIGINT/SIGTERM.

See `RUNBOOK.md` for protocol and recovery details.
