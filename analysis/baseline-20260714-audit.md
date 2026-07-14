# Baseline 2026-07-14 Audit

## Verdict

The uncapped `baseline-20260714T121001Z` workflow is promotable after two
evidence-preserving reporting corrections. The corrected joint profile is
`baselines/esmeralda_baseline.json` and passes schema-v5 submission validation.
No measured duration, fee, API result, or chain result was changed.

## Run Integrity

- The workflow ran from `2026-07-14T12:10:59Z` to `15:16:02Z` under harness
  commit `0b5d4f4ed3ec00cc19610021b3a042959914b281`.
- B0 completed before funding. S0 then funded all three fresh seeds with one
  shared transaction at height 754008 and proved exactly one spendable
  `10000 T` output per mode at depth `C_min=3`.
- Every scan repetition used its own path. Library scans removed the DB, WAL,
  and SHM files before initialization; console scans recursively removed and
  recreated the complete base path. Genesis scans rewrote the encoded birthday
  to zero.
- The final selected-chain block `754422@0354d948...6878ba4` was independently
  re-read from both the local archival endpoint and remote authority endpoint.
- All reference parameters were uncapped and recorded: 512 target outputs, six
  doubling rounds, 8-output fan-out, S4 ramp 8/16/32/64/128, 900-second S4
  budget per arm, S5 M=100/K=10, and fee rate `5 uT`.

## Reporting Corrections

1. New-wallet and companion-wallet scans store wallet-local IDs for one-sided
   incoming history. The old harness compared those IDs with sender-side chain
   transaction IDs, falsely failing S2/S3/S6/S7 despite exact cursor, balance,
   and spendable-state results. The corrected contract proves each expected
   chain transaction by intersection with its independently verified output
   commitments. Mode 2 S2/S3/S6/S7 and PP S2/S3 therefore become successful.
2. PP S1 proved 512 outputs in its final round, but a concurrent receiver DB
   read returned an error and overwrote exported `unspent_after` with null.
   PP balance components were similarly lost. They were reconstructed exactly
   from the one-output S0 state, independently confirmed payments, verified
   fees, scan balances, and final DB state. The run stopped only because the
   validator looked for PP `unspent_after` at the wrong JSON nesting level.

## Scenario Review

| Mode | Scenario | Outcome | Wall ms | Fees uT | Audit conclusion |
|---|---|---:|---:|---:|---|
| Old | B0 | success | 357012 | 0 | Empty genesis scan reached shared target 754000. |
| Old | S0 | success | 1066 | 0 | Exact one-output funding state proved. |
| Old | S1 | success | 1148696 | 193260 | 127/127 confirmed; exactly 512 outputs. |
| Old | S2 | failure | 365012 | 0 | Genuine recovery pain: 639 spent+live ancestors marked spendable. |
| Old | S3 | failure | 5014 | 0 | Same genuine spent-state recovery defect from birthday. |
| Old | S4 | success | 710643 | 163680 | 248/248 independently mined; only 15 API calls returned success, exposing 233 false `NotFound` API errors. |
| Old | S5 | success | 174480 | 66000 | 100/100 individual sends confirmed. |
| Old | S6 | failure | 350010 | 0 | Genuine recovery pain: 987 outputs marked spendable. |
| Old | S7 | failure | 6008 | 0 | Same genuine post-S5 spent-state recovery defect. |
| New | B0 | success | 71579 | 0 | Empty genesis scan reached shared target 754000. |
| New | S0 | success | 155 | 0 | Exact one-output funding state proved. |
| New | S1 | success | 873439 | 193260 | 127/127 confirmed; exactly 512 outputs. |
| New | S2 | success | 72290 | 0 | Exact 512 spendable and post-S1 balance; identity false failure corrected. |
| New | S3 | success | 799 | 0 | Same result from birthday. |
| New | S4 | failure | 4500154 | 173600 | 119 confirmed and 129 timed out across five independent 15-minute arms. |
| New | S5 | success | 105741 | 70000 | 100/100 individual sends confirmed from disclosed post-S4 state. |
| New | S6 | success | 71466 | 0 | Exact 512 spendable and balance; all expected transactions proved by commitments. |
| New | S7 | success | 997 | 0 | Same result from birthday. |
| PP | B0 | success | 71020 | 0 | Empty companion genesis scan reached shared target 754000. |
| PP | S0 | success | 2210 | 0 | Exact one-output funding state proved. |
| PP | S1 | success | 1106488 | 193260 | 127/127 batches confirmed; exactly 512 outputs. |
| PP | S2 | success | 72363 | 0 | Exact 512 spendable and balance; identity false failure corrected. |
| PP | S3 | success | 799 | 0 | Same result from birthday. |
| PP | S4 | failure | 743257 | 55440 | 84 confirmed; 164 HTTP 500 SQLite-lock failures are genuine PP/API contention. |
| PP | S5 | success | 83051 | 32250 | 10/10 batches and 100 payments confirmed. |
| PP | S6 | failure | 72954 | 0 | Genuine scanner persistence issue: 512 observed vs 520 proven post-S5 outputs. |
| PP | S7 | failure | 874 | 0 | Same genuine incomplete recovery from birthday. |

## S4 Concurrency And Budget

- Every mode creates one Tokio task per S4 call with `JoinSet`; calls are not
  retried, backed off, throttled, pre-partitioned, or serialized by the harness.
- Mode 2 took 4,500,154 ms because each of the five arms consumed its own
  900-second absolute budget while waiting for submitted transactions and state
  refresh. This is compliant with the bounty's `T_budget` stop condition per
  `N_concurrent`, not one 15-minute budget for the whole ramp.
- The dominant Mode 2 delay was chain/confirmation progress and state refresh,
  not construction dispatch. Arm walls were 900015, 900020, 900031, 900023,
  and 900063 ms.
- PP's HTTP 500 bodies came from the payment processor itself: SQLite code 5,
  `database is locked`. The harness only issued concurrent HTTP requests and
  recorded the responses. This is genuine wallet/payment-processor pain.

## Cursor And Metrics Audit

- S0 recipient synchronization advanced Mode 2 and PP to the funding target.
  Strict preflight then refreshed both library cursors immediately before each
  selected-chain gate. The repeated same-tip refreshes were fixed-target,
  typically one invocation, and did not replay history.
- S1 refreshes happened between dependent rounds. Mode 2 refreshed within each
  S4 arm deadline and after S5. PP refreshed receiver state after each S1 round
  and observed balances after S4/S5. Fresh scans never reused active cursors.
- The structured profile records wall time, explicit zero/nonzero fees, outcome
  counts and reasons, start/end tips, balance reconciliation, transaction
  timing, resource peaks, scan cursors/hashes, and computed deltas. Submission
  semantic validation passes.
- S5 throughput is 1.2732056206427376x for Mode 2 individual over PP batch and
  2.1008777738979663x for old-wallet individual over PP batch.
