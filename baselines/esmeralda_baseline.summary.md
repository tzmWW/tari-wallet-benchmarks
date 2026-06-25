# Esmeralda Baseline Summary

This summary accompanies `baselines/esmeralda_baseline.json`.

The checked-in profile was regenerated on `2026-06-25T00:54:28.628697Z` with
schema v3. It is a capped live proof, not a full statistical B0/S0-S7 baseline:
`repetitions = 1`, `live_fresh_scan_cells = false`, and Mode 1/2/3 live
topologies were enabled with low safety caps. This profile includes the
post-REVIEW_v4 independent Mode 2 base-node transaction-query metrics and the
post-V5 fresh-funded Mode 2 proof.

- Network: Esmeralda only.
- Harness repository: `https://github.com/tzmWW/tari-wallet-benchmarks`.
- Mode 1 surface: `minotari_console_wallet` gRPC.
- Mode 2 surface: pinned `minotari` crate APIs.
- Mode 3 surface: real `minotari_payment_processor` plus companion minotari
  wallet.
- Environment metadata now records OS/CPU/memory, disk type/name, and base-node
  network path. This baseline was captured on `macOS 26.5.1`, Apple M1 Pro,
  SSD, with remote base node `rpc.esmeralda.tari.com`.
- Seed phrases and wallet passwords are excluded from result profiles.

Current checked-in live evidence:

- Mode 1 S0 started a real `minotari_console_wallet` process with gRPC enabled.
- Mode 1 S1 submitted one capped `0.02 T` CoinSplit round with two outputs. Tx
  `14858780110045966490` was confirmed at height `711323` with fee `945`
  microtari.
- Mode 1 S4 submitted one capped concurrent-batch gRPC transfer. Tx
  `6263396227549309864` was confirmed at height `711324` with fee `660`
  microtari.
- Mode 1 S5 submitted the capped batch arm plus individual arm against
  deterministic distinct recipients. Four txs were confirmed:
  `181418807368016324`, `9589664585746981326`, `1552568187080471278`, and
  `4961869139025709192`.
- Mode 2 used the ignored fresh-proof wallet DB
  `.bench-data/new-wallet-fresh-proof/wallet.db`, which matches
  `HARNESS_SEED_NEW_FRESH`. It was funded from the old wallet with six
  independent `0.09 T` one-sided transactions:
  `16088670335361737216`, `2162886295002035165`, `3062470075941489107`,
  `12073750951134594766`, `17402180299222494064`, and
  `14747389757172597130`. Those funding txs mined at height `711302`; a
  supported scanner catch-up to height `711305` made them spendable before the
  proof.
- Mode 2 S0 detected `360000` available microtari in the fresh-proof wallet for
  the promoted all-mode run.
- Mode 2 S1 used the self-directed multi-recipient one-sided builder. Tx
  `3342988131844877001` was verified through
  `base_node_transaction_query` as `Mined` at height `711336`, at least `C_min`
  deep, with fee `990` microtari.
- Mode 2 S4 tx `5756695120974193262` was verified through
  `base_node_transaction_query` as `Mined` at height `711341`, fee `700`
  microtari.
- Mode 2 S5 tx `5402626848094413870` was verified through
  `base_node_transaction_query` as `Mined` at height `711345`, fee `700`
  microtari.
- Mode 2 wallet DB observations still reported the submitted txs as `broadcast`;
  confirmed evidence comes from deserializing
  `completed_transactions.serialized_transaction`, querying the public base-node
  `/transactions` endpoint by kernel excess signature, and checking `/get_tip_info`
  for `C_min` depth. Broadcast, pending, mempool-only, timeout, or query-failed
  cases remain metrics/notes rather than top-level chain-verification rows.
- Mode 3 S0 started the real `minotari_payment_processor` plus companion
  payment receiver.
- Mode 3 S1 drove PP `/v1/payment-batches` in S1 round shape. Batch
  `5410f63c-787d-4cde-8696-04293c0df97c` was accepted and recorded as
  `PENDING_BATCHING` in the profile.
- Mode 3 S4 accepted batch `8192f876-0c49-483b-bcf5-5264d55bf1a7`; Mode 3 S5
  accepted batch `4fe8d005-63dd-4fbd-9a81-359b8c362dc7`. Both remained
  `PENDING_BATCHING` in the profile.
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
- Fixed after REVIEW_v4: environment capture includes disk/network-path fields,
  direct `time::sleep(...)` alias calls are covered by ast-grep, Mode 2 DB
  status mapping is extracted/tested against the pinned minotari status strings,
  Mode 2 chain verification uses base-node transaction queries instead of DB-only
  confirmation, and fresh scan cells are checkpointed instead of predeclared.
- Fixed in the promoted V5 evidence pass: fresh-funded Mode 2 S1/S4/S5 now
  construct, sign, broadcast, and produce confirmed base-node transaction-query
  rows from independent spendable UTXOs.
- Still not claimed as complete: three-repetition statistical evidence, the full
  fresh-scan matrix (`B0/S2/S3/S6/S7`), full-volume stateful spend cells, and a PP
  terminal-confirmation rerun. The current profile is intentionally labeled as
  capped proof rather than final performance data.
