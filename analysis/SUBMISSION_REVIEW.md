# Submission Review

This is the final status review for
`https://github.com/tari-project/wallet-benchmarks/issues/1`.

## Upstream Requirement Check

| Requirement | Current status |
|---|---|
| Standalone harness source | Satisfied. This repo builds a Rust `wallet-bench` binary and pins external Tari/minotari sources. |
| Run instructions | Satisfied. `README.md` gives the short path; `RUNBOOK.md` covers setup, funding, local-node proof, preflight, live run, and diagnostics. |
| Baseline result profile | **Blocked.** `baselines/esmeralda_baseline.json` is historical schema-v3 evidence and fails the current schema-v4/submission contract. It must be replaced only by a passing clean candidate. |
| Three wallet modes | Satisfied. Mode 1 is real console-wallet gRPC, Mode 2 is pinned `minotari` library APIs, Mode 3 is real `minotari_payment_processor` plus companion wallet. |
| All B0/S0-S7 cells | Implemented in required per-mode order: `B0,S0,S1,S2,S3,S4,S5,S6,S7`. Every scan repetition wipes its DB; B0 uses seed words whose encoded birthday is `0`. Failed and blocked cells remain visible. |
| Config parameters recorded | Satisfied. The profile records `A_fund`, `C_min`, S1/S4/S5 parameters, fee rate, versions, live flags, scan/send repetitions, and caps. |
| One `A_fund` UTXO per mode | Must be reproven for the next namespace by selected-chain strict preflight. Prior funded namespaces were spent and are historical only. |
| Full S4 ramp | Satisfied in current baseline: `[8, 16, 32, 64, 128]`. |
| Structured output for comparison | Implemented in schema v4. Submission validation now also requires canonical order, wall time, explicit fees, and final-balance evidence for every completed repetition. |
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

## Latest Run Integrity

- Latest complete diagnostic profile:
  `.bench-data/final-ready-20260711T024112Z/candidates/clean-20260711T104411Z`.
  It is schema-v4 and `run_complete=true`, but is not promotable.
- The wrapper failed after profile creation because zsh reserves `status`; the
  ignored namespace wrappers now use `run_status`, so validation and summary are
  no longer skipped after a completed run.
- The old runner appended scan work out of mode-local chronology. Orchestration
  now runs each mode B0 through S7 in order and checkpoints after each mode.
- Mode 2 and PP each confirmed the first S1 transaction, but evaluated balance
  before their wallet/companion state converged. Both now perform a bounded
  post-confirmation refresh before applying the exact S1 invariant.
- The diagnostic run preserves real wallet behavior: Mode 1 S1 was 127/127;
  recovery rediscovered 127 spent parents plus 512 children; S4 was 17/248; S5
  was 99/100 with one SQLite lock; later recovery hit RPC decoding failures.
- The profile contains 245 independently confirmed transactions. That does not
  override the failed submission gate (`new_wallet/S1` was non-canonical).

## Remaining Risks

- No committed submission baseline currently passes schema v4 and
  `validate-profile --submission`.
- Live stateful send paths currently emit one repetition per scenario. Do not
  claim three-repetition statistics until those loops are implemented and funded.
- Post-run wallet DBs are mutated, and the July 2 pre-run backup is no longer a
  safe restore source after recoup attempts touched it. Another submission run
  must fund fresh seeds and pass `preflight --check-funds`.
- Cleanup and recoup work must never delete useful proof artifacts or stage
  ignored secrets/DBs/logs.
- S0 funding occurs before the measured run, as the bounty permits. The profile
  records its tx/height/birthday and explicitly marks broadcast/confirmation
  timing unavailable rather than fabricating per-transaction durations.

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
cargo run -- validate-profile --profile candidate.json --submission
```
