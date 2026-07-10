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
- Result profiles now include strict spec-facing observations: exact S0 funding
  checks, scan expected-vs-found checks, scan RSS/CPU peaks, per-tx timing rows,
  balance reconciliation, S5 per-arm metrics, and top-level `computed_deltas` for
  scan and S5 comparisons.
- The live runner uses substantive `src/live_minotari/{mode1,mode2,mode3,scan,
  verification}.rs` modules for mode paths, fresh scans, and independent chain
  evidence. The root module retains only shared orchestration, data models,
  transaction construction, and tests.

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
- Mode 1 S1/S4/S5 `Transfer` submissions are one-shot. SQLite lock,
  pending-funds, and construction errors are measured outcomes; the harness has
  no scenario retry or backoff path.
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
  local or remote. The current committed Esmeralda baseline records local
  `127.0.0.1` base-node evidence on an SSD-backed macOS host.
- The final submission starting state should use one clean `A_fund` UTXO per
  mode. Multi-UTXO funding remains useful development proof infrastructure but
  should not be promoted as the final reference profile because it can look like
  hidden UTXO pre-partitioning.

## Local Baseline Status

The committed `baselines/esmeralda_baseline.json` is generated by this harness
schema and is safe to share. The current schema v3 profile was generated on
`2026-07-02T22:16:39.401016Z` against the local Esmeralda node at
`127.0.0.1`. It uses the full reference S4 ramp
`[8, 16, 32, 64, 128]`, `repetitions = 1`, `scan_repetitions = 1`,
`live_fresh_scan_cells = true`, and all live caps set to `0`.

The current profile contains 6 top-level confirmed chain-verification rows:
Mode 1 confirmed one S1 transaction, two S4 transactions, and one S5
transaction; Mode 2 confirmed one S1 transaction through independent base-node
kernel-signature query; Mode 3 confirmed one PP S1 batch. Those are the only
rows promoted to top-level chain evidence. Accepted-but-pending PP batches,
wallet DB observations, mempool-only rows, and failed base-node queries stay in
scenario metrics.

The failed cells are intentional observations, not harness polish gaps. Mode 1
S1/S4/S5 expose console-wallet pending-funds behavior from the single-UTXO
starting state, and Mode 1 S2/S3/S6/S7 fail strict checkpoint reconciliation
after partial sends because the recovered wallet finds more outputs/balance than
the checkpoint expected. Mode 2 S1 confirms the first self-directed transaction
then records `Funds are pending` for later S1/S4/S5 attempts; S6/S7 are blocked
checkpoint rows because S5 did not produce a runnable checkpoint. Mode 3 accepts
the PP batch shapes but leaves most rows pending or failed under PP/API
contention, so only confirmed batches are top-level evidence.

Strict scanner validation is now part of the evidence. Mode 2 and PP companion
B0/S2/S3/S6/S7 scans can return from the pinned minotari scanner far below the
local base-node tip. The harness records `tip_lag_blocks`,
`tip_lag_tolerance_blocks`, and `scan_reached_tip`, and marks those cells failed
when `max_height + C_min < tip_end` rather than treating them as successful
full-chain scans.

The stale finding `mode2-single-recipient-library-limit` has been superseded.
Mode 2 S1 uses the lower-level multi-recipient `OneSidedTransaction` builder for
the required doubling/fan-out round shape. S4 and S5 remain single-recipient
sends because that is their scenario shape.

Post-run funds are not ready for another no-cap attempt. The current baseline
mutated `.bench-data/final-local-20260702T151225Z`, and later recoup attempts
touched the July 2 pre-run backup, so that backup is historical evidence only.
Another final run must fund fresh seeds, then require `preflight --check-funds`
to pass with exactly one spendable `A_fund` output per mode and no locked,
pending, invalid, cancelled, not-stored, or unknown outputs.

The final submission rerun shape remains `concurrent_batches = [8, 16, 32, 64, 128]`,
`repetitions = 1`, `scan_repetitions = 1`, `live_fresh_scan_cells = true`, and
all live caps at `0` unless stateful live repetitions are implemented and funded.

## 2026-07-01 Interrupted Final Rerun Findings (Superseded By July 2 Baseline)

Two final-run attempts were intentionally stopped during Mode 1 fresh scan
coverage and must not be promoted as submission baselines:

- `logs/final-live-clean-20260701T024039Z.log` proved the required Mode 1 S1
  no-cap shape through the full doubling/fan-out path. The final fan-out
  submitted all `64` `CoinSplit` transactions successfully and verified
  `64/64`, then entered `old_wallet/S2` genesis recovery.
- During that first attempt, the Mode 1 S2 recovery console wallet repeatedly
  hit public Esmeralda base-node request failures, restarted from nearby scan
  checkpoints, and eventually sat idle around block `364823` of roughly
  `722484`. The harness was blocked waiting on an unbounded gRPC `get_state`
  call and did not reach its own startup timeout.
- The harness was hardened after this finding: Mode 1 fresh-scan state polling
  now wraps `get_state` in a bounded `10s` timeout, and also fails a scan cell
  after `timeouts.scan_batch_secs` with no scanned-height progress. The intended
  result is an explicit failed S2 repetition rather than another hung run.
- `logs/final-live-clean-rerun-20260701T105707Z.log` again submitted and
  verified the full Mode 1 S1 fan-out (`64/64`), then entered `old_wallet/S2`.
  The scan was manually interrupted instead of waiting hours for another
  genesis recovery pass. It is useful operational evidence, not a final profile.

The post-interruption `harness.toml` was set up for the required full
uncapped/ramped run: `concurrent_batches = [8, 16, 32, 64, 128]`,
`repetitions = 1`, `scan_repetitions = 1`, `live_fresh_scan_cells = true`, and
all live caps at `0`. As of the post-interruption check, no benchmark or
minotari child processes were left running, and `preflight --check-funds` passed
on the then-active final wallet paths:

- Mode 1 final console wallet: `0(Unspent):759:10727996113`.
- Mode 2 final wallet: `SPENT|764|30896757960`, `UNSPENT|778|10565612275`.
- PP final receiver: `SPENT|138|9660000000`, `UNSPENT|151|10005737635`.

Those paths were later consumed by the July 2 local-node profile above. Treat
the July 1 notes as historical evidence explaining why scan-timeout hardening
was needed, not as current wallet readiness.
