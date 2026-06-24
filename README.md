# Tari Wallet Benchmarks

Standalone reproducible wallet performance harness for
`tari-project/wallet-benchmarks`.

The harness targets Esmeralda only and models the three required wallet surfaces:

- old wallet: `minotari_console_wallet` process managed by the harness
- new wallet: `minotari` crate APIs for scan, selection, signing, and broadcast
- payment processor: real `minotari_payment_processor` plus a parallel `minotari` payment-receiver wallet

Current implementation status: CLI/config/schema/seed handling, Esmeralda
guarding, result-profile generation, PP environment/API contract, static
"no hidden wallet pain" rules, and the Mode 2 `live-minotari` scan/signing smoke
path are in place. Mode 2 submitted transactions are independently queried
through the public base-node `/transactions` endpoint by extracting kernel
signature data from the wallet DB's serialized transaction. Top-level chain
verification rows are emitted only for mined transactions that are at least
`C_min` deep. Fresh scan cells are checkpointed: B0 uses an empty genesis seed,
S2/S3 run after a valid S1 checkpoint, and S6/S7 run after S5. Mode 1 scan cells
use real `minotari_console_wallet --recovery`; Mode 2 and PP companion scans use
fresh minotari scanner databases. The checked baseline includes capped real Mode
1 `minotari_console_wallet` gRPC S0/S1/S4/S5 evidence, capped Mode 2 live
S1/S4/S5 send-side cells, and capped real Mode 3 payment-processor S0/S1/S4/S5
coverage through the pinned PP daemon. The remaining bounty-critical work is
promoting a fresh-funded Mode 2 proof and running the full funded B0/S0-S7
matrix with repetitions where wallet state supports it.

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
cargo run --features live-minotari -- run --config harness.toml --profile baselines/esmeralda_baseline.json
```

Full operator detail is in [RUNBOOK.md](RUNBOOK.md).

The committed baseline JSON is schema-valid harness output and deliberately
contains no secrets. It includes funding evidence plus capped Mode 1, Mode 2,
and real Mode 3 live evidence, but not the completed all-mode benchmark matrix
or the fresh-funded Mode 2 proof yet.
