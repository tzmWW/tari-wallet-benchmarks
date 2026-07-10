# Initial Plan Audit — 2026-07-10

This document reconciles every requirement in the initial submission-ready plan
with this branch. “Implemented” means source, tests, and result contract are
present. A live-run or publication status is not claimed without a new artifact.

## Summary

| Plan item | Status | Evidence |
| --- | --- | --- |
| Preserve a pristine final namespace | Implemented; operationally preserved | Strict preflight requires one exact funding UTXO and dirty state fails. Final-clean is never used by development code. |
| Do not promote July 2 baseline | Implemented | README/RUNBOOK require schema-v4 submission validation and call the baseline historical. |
| Use a true fork/PR after accepted artifact | Pending publication | No candidate artifact or PR is claimed. |
| Preserve measured wallet failures | Implemented | No scenario retry, backoff, or repair path; failures remain profile outcomes. |

## Protocol and Code Corrections

| Plan item | Status | Evidence |
| --- | --- | --- |
| S1 exact split, live input, fee-only balance delta, exact output counts | Implemented | Mode 1/2/3 execute canonical rounds and verify UTXO count, fee-only balance delta, and independent confirmation. Mode 3’s required builder change is its final balanced child, yielding the requested output count. |
| One submission per S1 transaction; no Mode 1 lock retry | Implemented | Mode 1 uses one-shot gRPC Transfer; AST rules reject scenario retry/backoff. |
| Mode 3 self-payment, one completed round at a time | Implemented | Mode 3 pays its own address, observes terminal PP state per round, and independently proves signed kernels. |
| S4 distinct recipients and concurrent dispatch | Implemented | Recipient derivation is deterministic; each arm selects distinct recipients and uses join sets without dispatch sleeps. |
| S4 deadlines, terminal observations, full wall time | Implemented | Mode 1/2/3 include confirmation/timeout observation in each arm’s wall clock and preserve terminal failures. |
| S4 structured successes/rejections; wallet failures do not hide S5 | Implemented | Common observations contain identity when available, timings, outcome, error, fee, heights, and tip range. |
| S5 same 100 recipients; individual old/new and PP 10×10 | Implemented | Mode 1/2 derive the same pool; Mode 3 batches it by S5_K. |
| S5 confirmation/timeout timing, PP chain data, null incomplete comparisons | Implemented | Arm walls include terminal observation; confirmation duration is emitted; PP extracts kernel data and queries the base node; semantic validation rejects fabricated ratios. |
| Checkpoints gate scans; no startup DB surgery | Implemented | S1 gates S2–S7; fresh scans record blocked prerequisites; preflight and AST rules reject direct unlock/expiry repair. |
| Substantive module split | Implemented | Root live module is 4,131 lines of shared core/tests; Mode 1 is 1,286, Mode 2 935, Mode 3 799, scan 649, verification 405, and profile validation 860 lines. |

## Public Interfaces and Result Schema

| Plan item | Status | Evidence |
| --- | --- | --- |
| Draft 2020-12 schema v4 | Implemented | Generated RESULT_PROFILE_SCHEMA.json plus schema/profile tests. |
| Explicit execution/outcome statuses | Implemented | Schema and serialization use the required four values for each dimension. |
| Run identity, completion, commit, endpoint/anchors, version reference, birthdays | Implemented | Result profile, schema, environment capture, and submission validation require them where applicable. |
| Honest v5.4.0 reference and unobservable public version | Implemented | Reference revision is fixed; observed version is constrained by observability. |
| S4/S5 amount names with legacy aliases | Implemented | Config parsing accepts legacy names and tests cover compatibility. |
| Common transaction observations | Implemented | Schema requires construction, submission, mempool availability/reason, confirmation duration, fee, outcome, error, mined/tip data, and nullable transaction/batch identity. |
| Fresh addresses; load-required preflight/run seeds | Implemented | Seed generation ignores exported benchmark seeds; operational paths reject missing material. |
| Submission validation and JSON-only summary generation | Implemented | CLI integration tests cover schema/semantic validation and summary generation. |
| Validate checkpoints/final before completion | Implemented | Checkpoints are validated before writing; final validation checks complete run, canonical S1, all cells, and independent chain rows. |

## Preflight, Verification, and Tests

| Plan item | Status | Evidence |
| --- | --- | --- |
| Seed/DB identity, exact funding, scanner freshness, selected-chain proof | Implemented | Strict preflight checks all; final-clean scanners must be refreshed before launch because they have drifted. |
| Pins, ports/disk/endpoint/chain/pruning checks | Implemented where observable | Strict preflight captures configured pins and verifies runtime/endpoint safety; public version remains explicitly unobservable. |
| Automatic strict preflight on live run | Implemented | Runner test covers strict preflight before profile writing. |
| Focused behavioral/schema/security tests | Implemented | 88 library and 4 integration tests cover split math, checkpoints, deadlines, recipients, confirmation, malformed profiles, funding mismatch, and no-repair rules. |
| Complete mechanical gate | Implemented | Format/check/clippy/test/AST/schema/diff/secret inspection are the release gate. |

## Live Proof and Final Run

| Plan item | Status | Evidence |
| --- | --- | --- |
| Preserve final-clean wallets | Implemented; readiness not yet current | Namespace remains outside development work; preflight must be refreshed after scans. |
| Fund separate 2 T × 3 rehearsal namespace | Blocked externally | No independently verified disposable source exists; final-clean funds must not be repurposed. |
| Execute scaled all-mode rehearsal and fix harness errors | Pending funding | No new live claim is made. |
| Re-run strict final-clean preflight | Pending fresh scans | Known scanner heights lag public Esmeralda tip. |
| Power/sleep/log/candidate safeguards | Implemented in instructions; pending execution | RUNBOOK specifies unattended-run capture. |
| Candidate acceptance/promotion criteria | Implemented in submission validation | No candidate exists yet to satisfy them. |

## Publication

| Plan item | Status | Evidence |
| --- | --- | --- |
| Fetch/rebase, scoped commits, push owned harness repo | In progress | Performed before edits; final scoped commit/push follows verification. |
| Fast-forward default branch and push true submission fork | Pending valid candidate | Must not happen before live acceptance. |
| Open upstream PR, wait for CI, review threads | Pending valid candidate | Issue #1 is open; no PR is represented as ready. |

## Result

All implementation and documentation items in the initial plan are complete on
this branch after the module refactor and observation-contract correction. The
remaining items are intentionally operational: separate rehearsal funding, fresh
scanner synchronization, unattended live execution, candidate validation, and
publication. They cannot be completed truthfully without external chain funding
and a real run.
