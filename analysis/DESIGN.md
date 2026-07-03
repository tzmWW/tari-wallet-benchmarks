# Design Notes

- Mode 1 uses a real `minotari_console_wallet` process with gRPC. The harness
  manages lifecycle, drives S1 through `CoinSplit`, and drives S4/S5 through
  `Transfer`. The only retry is a capped S1 `CoinSplit` retry for a transient
  submit-time SQLite `database is locked` before a tx id exists; S4/S5 transfer
  errors are recorded without retry.
- Mode 2 uses the pinned `minotari` library path for signing and direct base-node
  HTTP submission. It verifies submitted transactions by extracting kernel
  signatures from the wallet DB and querying the public Esmeralda base node.
- Mode 3 uses the real `minotari_payment_processor` plus a companion minotari
  receiver wallet. PP scan-shaped cells are reported as `companion_wallet_scan`
  because PP has no direct scan API.
- Fresh scan cells are checkpointed. B0 uses an empty genesis seed; S2/S3 require
  an S1 checkpoint; S6/S7 require an S5 checkpoint. Blocked prerequisites produce
  explicit failed repetitions with `blocked_prerequisite = true` instead of
  synthetic measurements.
- Send repetitions and scan repetitions are intentionally separate. Current live
  stateful send cells emit one observed repetition; long fresh scan cells use
  `scan_repetitions` so future repeated send stats do not multiply genesis-scan
  runtime.
- Result profiles carry raw observations and derived comparisons. Live scenario
  metrics include strict S0 checks, balance reconciliation, scan expected-vs-found
  checks, scan peak RSS/CPU, per-tx timing rows, and S5 per-arm metrics. The
  top-level `computed_deltas` section derives scan deltas and S5 throughput
  ratios from those observations.
- Long live runs write per-stage checkpoint profiles next to the final profile so
  interrupted unattended runs preserve completed-stage evidence.
- `preflight --check-funds` is a usability gate, not a benchmark operation. It is
  meant to prevent stale DBs, locked outputs, and wrong recovered wallets from
  wasting a no-caps Esmeralda run.
- `fund-one-sided` is also outside the measured benchmark path. It reuses the
  pinned Mode 2 signing/broadcast code to help operators fund fresh benchmark
  seeds from recovered minotari signing wallets. It must not be used to
  pre-partition the final benchmark starting state, which should be one clean
  `A_fund` UTXO per mode.
- `src/live_minotari.rs` still owns orchestration, but isolated helper modules now
  live under `src/live_minotari/` for Mode 1 retry detection, Mode 2 status
  mapping, Mode 3 terminal PP observations, and scan resource sampling.
