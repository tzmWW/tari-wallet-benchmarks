# Baseline 2026-07-31 Audit

## Verdict

The uncapped `baseline-20260730T220138Z` run completed all 27 cells. The
byte-exact final profile is `baselines/esmeralda_baseline.json` and passes strict
schema-v6 submission validation. Failed scenarios remain failed measurements.

## Integrity

- Measurement and export commit: `fc37c668b3ff1ccbe94e343d80a7065db9fa0cf6`.
- Profile SHA-256:
  `e00d8f9ea4711f825bb81e8af3abfc3a02a9831cc3f3030483aea0b39400a024`.
- Configuration: `A_fund=10000 T`, `C_min=3`, 512 S1 outputs, S4
  `[8,16,32,64,128]`, 900-second arm budget, S5 `M=100/K=10`, and `5 uT`
  fee rate, with all live safety caps disabled.
- Start anchor: height `792984`, hash
  `3ef122af8419d45b30dd9f94e19c116a5629aa9c5b0828b3678d508a07b1ccbf`.
- End anchor: height `793402`, hash
  `3d2e67b755c01386e1c8cb6226002547bd8551c53ba3e5aa5d3e68ba9dbe478f`.
  Local and authority endpoints matched at both anchors.
- The profile contains 1,073 independently confirmed transaction rows.

## Findings

- Mode 1 S2/S3 reached their exact target but recovered 639 spendable outputs
  instead of 512. S6/S7 likewise recovered 987 instead of 512. Active and fresh
  wallet databases account for the excess exactly as spent ancestors exposed as
  spendable by console-wallet recovery. These are wallet results, not relaxed or
  corrected harness outcomes.
- Mode 1 S4 dispatched each of 248 attempts once. All 248 reached `C_min`, while
  only 15 calls returned an ordinary successful gRPC response; 233 returned a
  transaction-identified `NotFound` before independent confirmation. The profile
  preserves both the API errors and confirmed terminal outcomes.
- Mode 2 S1 completed 127/127 transactions and every round reached its expected
  2/4/8/16/32/64/512 spendable-output count with exact fee reconciliation. Build
  provenance lists only the fixed-range scanner and password-input patches; the
  removed exact-output selection patch was not present. Transaction selection
  used upstream `FundLocker`, and every S1 transaction independently proved one
  input and 2 or 8 outputs.
- Mode 2 and payment-processor S4 failures remain the measured contention and
  deadline outcomes. No retries, backoff, pre-partitioning, or post-run outcome
  correction was applied.

The workflow's final re-validation originally stopped on a one-digit JSON
floating-point round-trip difference in a derived scan ratio. Canonicalizing the
derived number makes persisted validation deterministic; the promoted profile
bytes and all measured fields are unchanged.
