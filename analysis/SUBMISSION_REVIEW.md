# Submission Review

This is the final status review for
`https://github.com/tari-project/wallet-benchmarks/issues/1`.

## Upstream Requirement Check

| Requirement | Current status |
|---|---|
| Standalone harness source | Satisfied. This repo builds a Rust `wallet-bench` binary and pins external Tari/minotari sources. |
| Run instructions | Satisfied. `README.md` gives the short path; `RUNBOOK.md` covers setup, funding, local-node proof, preflight, live run, and diagnostics. |
| Baseline result profile | Satisfied as current-status evidence. `baselines/esmeralda_baseline.json` is schema v3 output from the July 2 local-node run. |
| Three wallet modes | Satisfied. Mode 1 is real console-wallet gRPC, Mode 2 is pinned `minotari` library APIs, Mode 3 is real `minotari_payment_processor` plus companion wallet. |
| All B0/S0-S7 cells | Satisfied structurally. Every cell emits a concrete status/repetition. Failed cells are preserved as observations. |
| Config parameters recorded | Satisfied. The profile records `A_fund`, `C_min`, S1/S4/S5 parameters, fee rate, versions, live flags, scan/send repetitions, and caps. |
| One `A_fund` UTXO per mode | Satisfied for the July 2 starting state. All three modes used the same three-recipient funding tx `5740188747787224553` for exactly `10000 T` each. |
| Full S4 ramp | Satisfied in current baseline: `[8, 16, 32, 64, 128]`. |
| Structured output for comparison | Satisfied. JSON profile plus `RESULT_PROFILE_SCHEMA.json`; schema v3 includes top-level `computed_deltas`. |
| No hidden wallet pain | Satisfied by design and static checks. Scenario failures, pending funds, PP contention, and below-tip scans are recorded instead of retried away. |

## Review Signals Considered

- PR #8 was closed by the maintainer because it did not provide actual
  benchmarks. This repo must keep the committed baseline and evidence files as
  first-class deliverables, not just framework code.
- PR #4 and PR #6 review history repeatedly emphasized: correct seed birthday
  handling, real end-to-end Esmeralda runs, actual `minotari_payment_processor`
  topology, self-testing before asking maintainers to run, visible terminal
  progress, and clear config/run docs.
- `0xPepeSilvia/tari-wallet-benchmarks` is useful as a benchmark for honest
  evidence framing: every claimed number linked to committed results, failures
  were reported as wallet pain, and S5 was framed as both throughput and fee
  behavior. That repo stopped with Mode 2/3 blockers; this repo should preserve
  its broader three-mode live evidence as the differentiator.

## Current Baseline Integrity

- Latest profile: `baselines/esmeralda_baseline.json`, generated at
  `2026-07-02T22:16:39.401016Z`.
- Local node: `127.0.0.1`, with funding-block proof recorded in `RUNBOOK.md`.
- Live shape: all live topology flags enabled, all live caps `0`,
  `live_fresh_scan_cells = true`, `scan_repetitions = 1`, `repetitions = 1`,
  S4 ramp `[8, 16, 32, 64, 128]`.
- Confirmed top-level chain evidence: 6 rows total. Pending or unconfirmed rows
  remain scenario metrics.
- Failed cells are expected for this current-status baseline:
  - Mode 1/2 send failures expose single-UTXO pending funds and locked change.
  - Mode 3 failures expose PP/API contention and `PENDING_BATCHING` behavior.
  - Mode 2/PP companion scanner failures expose below-tip scanner completion.
  - Mode 1 scan mismatches expose checkpoint reconciliation differences after
    partial sends.

## Remaining Risks

- The profile is not an all-ok statistical benchmark. It is a real, strict
  current-status run that surfaces wallet pain.
- Live stateful send paths currently emit one repetition per scenario. Do not
  claim three-repetition statistics until those loops are implemented and funded.
- Post-run wallet DBs are mutated, and the July 2 pre-run backup is no longer a
  safe restore source after recoup attempts touched it. Another submission run
  must fund fresh seeds and pass `preflight --check-funds`.
- Cleanup and recoup work must never delete useful proof artifacts or stage
  ignored secrets/DBs/logs.

## Final Publish Gate

Before pushing, run:

```sh
cargo fmt --check
cargo check --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
ast-grep scan
jq empty baselines/esmeralda_baseline.json RESULT_PROFILE_SCHEMA.json
cargo run -- preflight --config harness.toml
```
