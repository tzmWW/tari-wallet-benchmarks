# Esmeralda Baseline Summary

This summary accompanies `baselines/esmeralda_baseline.json`.

The checked-in profile was regenerated on `2026-06-24T15:12:19Z` with schema
v3 after the REVIEW_v3 hardening pass. It is a capped live proof, not a full
statistical B0/S0-S7 baseline: `repetitions = 1`, `live_fresh_scan_cells =
false`, and Mode 1/2/3 live topologies were enabled with low safety caps.

- Network: Esmeralda only.
- Harness repository: `https://github.com/tzmWW/tari-wallet-benchmarks`.
- Mode 1 surface: `minotari_console_wallet` gRPC.
- Mode 2 surface: pinned `minotari` crate APIs.
- Mode 3 surface: real `minotari_payment_processor` plus companion minotari
  wallet.
- Seed phrases and wallet passwords are excluded from result profiles.

Current checked-in live evidence:

- Mode 1 S0 started a real `minotari_console_wallet` process with gRPC enabled
  and completed startup in `1662 ms`.
- Mode 1 S1 submitted one capped `1 T` CoinSplit round with two outputs. Tx
  `15297395523124947594` reached terminal-ok status `2` at height `710565` with
  fee `945` microtari.
- Mode 1 S4 submitted one capped concurrent-batch gRPC transfer. Tx
  `16571143755989443134` reached terminal-ok status `2` at height `710568` with
  fee `700` microtari.
- Mode 1 S5 submitted the capped batch arm plus individual arm against
  deterministic distinct recipients. Four txs were confirmed:
  `1810960988092390726`, `10759835787874539413`, `9489914261621933203`, and
  `17517677251440746429`.
- Mode 2 S0 failed because the current live DB has `0` available microtari and
  `50000998600` locked microtari. This preserves the live wallet state after
  earlier proof sends rather than wiping or pre-partitioning the wallet.
- Mode 2 S1 used the multi-recipient round plan but failed before submission
  with `Funds are pending`; the attempted first round required `2 T` for two
  outputs. Mode 2 S4/S5 failed for the same pending-funds condition.
- Mode 2 observations are kept in per-cell metrics under
  `observed_transactions`; only confirmed rows can enter top-level
  `chain_verification.verified_transactions`.
- Mode 3 S0 started the real `minotari_payment_processor` plus companion
  payment receiver in `2031 ms`.
- Mode 3 S1 now drives PP `/v1/payment-batches` in S1 round shape: two capped
  batches, two payments per batch, with round metrics recorded under
  `metrics.extra.rounds`. Both PP batches were accepted but remained
  `PENDING_BATCHING`.
- Mode 3 S4 accepted one capped one-payment PP batch and Mode 3 S5 accepted one
  capped two-payment PP batch. Both remained `PENDING_BATCHING`.
- PP DB observations are labeled `payment_processor_db_observed`; pending PP
  batches stay in metrics/notes and are not emitted as confirmed
  chain-verification rows.
- Mode 3 scan-shape cells are `not_applicable` when
  `live_fresh_scan_cells=false` because PP has no direct scan API. Companion
  wallet scans are only recorded when explicitly enabled.

REVIEW_v3 status:

- Fixed after the review: Mode 2 S1 multi-recipient round shape, Mode 2
  settlement gates, Mode 1 verified fee backfill, Mode 3 S1 PP batch shape,
  confirmed-only top-level verification rows, and PP scan-cell ambiguity.
- Still not claimed as complete: three-repetition statistical evidence and the
  full fresh-scan matrix. The profile is intentionally labeled as capped proof
  evidence.
