# Findings

This file records the wallet-pain findings the harness is designed to surface,
not hide.

## Current Implementation Decisions

- The repo is standalone and owned by `tzmWW/tari-wallet-benchmarks`; it is not a
  PR branch for `tari-project/wallet-benchmarks`.
- Mode 2 is library-first and links the pinned `minotari` crate at
  `360c4848a54d65fd710266233cc9277b0f785e74`.
- Mode 3 targets the real `minotari_payment_processor` application at
  `f0572c98cbfac7377412dc6d4094c7d7dfc5de2c`, with a parallel minotari payment
  receiver. The harness does not replace PP with an in-process batch shortcut.
- PP scan-shape cells use the `companion_wallet_scan` surface. PP has no direct
  scan API, but its topology still includes a companion wallet whose scan cost is
  benchmark-relevant.
- Mode 1 uses a real `minotari_console_wallet` process with gRPC enabled. The
  harness does not shell out to transaction commands for scenario execution; it
  starts the wallet, waits for funded balance, and drives gRPC `Transfer`
  requests.

## Upstream Risks To Preserve In Results

- `minotari_payment_processor` signs through a console-wallet subprocess. If the
  PP unsigned/sign/broadcast pipeline hits an upstream format mismatch, the cell
  must be recorded as `blocked_upstream` with logs instead of patched over in the
  harness.
- The pinned PP source uses SQLx compile-time query checks. Build it with a real
  migrated SQLite database at `data/payments.db`; otherwise the build fails with
  `unable to open database file` from SQLx macros. The helper script now creates
  that database from the pinned migrations before compiling.
- At runtime, PP's `DATABASE_URL`, `CONSOLE_WALLET_PATH`, and
  `CONSOLE_WALLET_BASE_PATH` should be absolute paths. Relative paths can work
  from the harness but fail inside PP's console-wallet signing subprocess with
  `No such file or directory`.
- The generated PP seed may be genesis-dated so the address is stable for
  funding. The companion payment-receiver view wallet should use a current
  birthday when it is first initialized for a fresh benchmark; otherwise it
  scans from block 0 before seeing same-day Esmeralda funding.
- `minotari_console_wallet` seed recovery uses the birthday encoded inside the
  mnemonic and ignores the separate `--birthday` flag for seed words. If a
  generated benchmark seed has birthday `0`, the harness must rewrite the launch
  mnemonic birthday while preserving the address and keys; otherwise Mode 1 S0
  performs an accidental long genesis-era scan.
- Wallet construction stalls, UTXO lock contention, and failed concurrent sends
  are benchmark signal. Scenario code must not add retries, backoff, throttling,
  sleeps between S4 dispatches, or hidden UTXO pre-partitioning.
- `WalletHttpClient::new` uses retry middleware by default. The Mode 2 send
  smoke persists the signed transaction to the minotari wallet DB, then submits
  it with a direct `submit_transaction` JSON-RPC request so the harness does not
  hide base-node submission failures behind transport retries.
- The pinned `TransactionSender::start_new_transaction` wrapper is
  single-recipient, but the lower-level `OneSidedTransaction` builder supports
  multiple recipients. Mode 2 S1 uses that builder for self-directed
  doubling/fan-out rounds so later scan cells measure outputs in the Mode 2
  wallet; S4/S5 keep the simpler single-recipient send path where that is the
  scenario shape.
- Mode 2 uses settlement gates between S1 rounds and between S4 and S5. The
  gate runs the wallet scanner and waits for base-node tip advancement rather
  than retrying failed sends.
- A 2026-06-24 live proof showed the Mode 2 wallet's `scanned_tip_blocks`
  metadata can update in coarse height buckets, which made an
  `initial_height + settle_wait_blocks` target wait far longer than actual chain
  advancement. The settlement gate now still runs the scanner each attempt, but
  uses the public base-node HTTP `/get_tip_info` height as the chain-advance
  clock.
- Mode 2 S4 dispatches concurrent attempts against the same wallet DB and keeps
  the pinned `FundLocker` behavior visible. The harness reports success/failure
  counts rather than smoothing over lock contention.
- Mode 2 S5 currently measures the individual-send arm. Batch-send comparison is
  left to Mode 3's real payment processor batch endpoint.
- Mode 2 stores wallet DB observations in scenario metrics, but top-level
  `chain_verification.verified_transactions` is confirmed-only. For submitted
  transactions, the harness deserializes
  `completed_transactions.serialized_transaction`, extracts the first kernel
  excess signature nonce/signature, queries the public base-node `/transactions`
  endpoint, and checks `/get_tip_info` for `C_min` depth. Broadcast, pending,
  mempool-only, timeout, or query-failed rows must not be counted as verified
  evidence.
- Mode 2 completed-transaction status mapping is source-backed: the pinned
  minotari `CompletedTransactionStatus` serializes as lowercase
  `completed`, `broadcast`, `mined_unconfirmed`, `mined_confirmed`, `rejected`,
  and `canceled`. The harness maps and tests those strings explicitly.
- Real Mode 3 live smoke confirmed PP can accept `/v1/payment-batches`, create
  unsigned transaction JSON, sign through `minotari_console_wallet`, broadcast,
  and reach confirmed batch state on Esmeralda. In the checked capped run, the
  first S1 batch advanced through signing/broadcast and later reached
  `CONFIRMED`; subsequent S4/S5 batches were accepted by PP but remained
  `PENDING_BATCHING` because the single funded PP UTXO/change was locked by the
  first transaction while awaiting confirmation.
- Mode 3 S1 now submits PP batches in the benchmark round shape: each planned
  S1 tx becomes one PP batch, and `outputs_per_tx` becomes payments per batch.
  PP confirmations are labeled `payment_processor_db_observed`; pending PP
  batches remain in metrics/notes instead of appearing as confirmed
  chain-verification rows.
- Real Mode 1 capped smoke confirmed `minotari_console_wallet` gRPC recovery and
  transfer behavior. S0 recovered the funded old wallet from a birthday-encoded
  mnemonic in about 23 seconds, S1 and S4 each created a real transaction, and
  S5 `single_tx=true` failed with `Funds are still pending` after prior sends
  locked available funds.
- Scan timings are noisy. Publish medians plus spread where a funded live run can
  support repetitions.
- Fresh full-chain scans are materially sensitive to scan batch size. A 100-block
  HTTP batch left the first Esmeralda B0 scan running for several minutes during
  development; `[benchmark].scan_batch_size` is now explicit and defaults to
  `1000`. Fresh scan cells are checkpointed: B0 is empty-genesis, S2/S3 require
  an S1 checkpoint, and S6/S7 require an S5 checkpoint. Mode 1 scans use real
  `minotari_console_wallet --recovery`; Mode 2 and PP companion scans use the
  minotari scanner.
- `TX_MINED_CONFIRMED` is recorded as status value `6` in result profiles.
- Environment capture records disk kind/name plus whether the base-node path is
  local or remote. The committed Esmeralda baseline records remote
  `rpc.esmeralda.tari.com` on an SSD-backed macOS host.

## Local Baseline Status

The committed `baselines/esmeralda_baseline.json` is generated by this harness
schema and is safe to share. The current schema v3 profile was regenerated on
2026-06-24 after REVIEW_v3 hardening. It includes capped real Mode 1
S0/S1/S4/S5 evidence, live Mode 2 pending-funds evidence from the current wallet
DB, and capped real Mode 3 PP S0/S1/S4/S5 evidence with PP scan cells marked
`not_applicable` when companion scans are disabled.

Post-REVIEW_v4 follow-up added the independent Mode 2 base-node transaction
query path and checkpointed fresh scan orchestration without a schema migration.
The polluted current Mode 2 DB still reports two completed rows as `broadcast`,
but querying their extracted kernel signatures against the public Esmeralda base
node shows both are `Mined` at heights `710477` and `710503`; those observations
prove the query path but do not create a fresh-funded proof. A fresh Mode 2 seed
was funded from the old wallet with tx `5240074109649442333`; a public
base-node transaction query later reported it `Mined` at height `710698`, while
the source wallet still reported `Broadcast`. The recipient fresh wallet stayed
at scan tip `710600` with no outputs, and the capped proof profile generated at
`2026-06-24T16:18:05Z` still saw `0` available microtari. The baseline was
therefore not replaced.

A full funded Esmeralda baseline still requires the complete B0/S0-S7 matrix
across all modes with repetitions where wallet state supports them. Do not treat
capped proof cells as final performance data.
