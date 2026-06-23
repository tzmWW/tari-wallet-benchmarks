# Tari Wallet Benchmarks

Standalone reproducible wallet performance harness for
`tari-project/wallet-benchmarks#1`.

This is the owned delivery repository at
`https://github.com/tzmWW/tari-wallet-benchmarks`; it is not a PR branch in the
upstream bounty repo.

The harness targets Esmeralda only and measures three wallet surfaces:

- old wallet: `minotari_console_wallet` process managed by the harness
- new wallet: `minotari` crate APIs for scan, selection, signing, and broadcast
- payment processor: real `minotari_payment_processor` plus a parallel `minotari` payment-receiver wallet

Start by generating fundable addresses:

```sh
cp harness.toml.example harness.toml
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
contains no secrets. Replace it with a funded live Esmeralda run before using it
as performance evidence.
