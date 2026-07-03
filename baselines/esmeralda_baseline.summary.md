# Esmeralda Baseline Summary

This summary accompanies `baselines/esmeralda_baseline.json`.

The checked-in profile was generated on `2026-07-02T22:16:39.401016Z`
with schema v3 against a local Esmeralda base node at `127.0.0.1`. It uses the
reference S4 ramp `concurrent_batches = [8, 16, 32, 64, 128]`,
`repetitions = 1`, `scan_repetitions = 1`, `live_fresh_scan_cells = true`, and
all live caps set to `0`. It is the current status artifact for the bounty
submission.

This is not an all-ok benchmark. The profile intentionally records wallet
pain: pending funds, locked change, payment-processor contention, and below-tip
scanner observations remain failed cells instead of being smoothed over by
retries, throttling, or UTXO pre-partitioning.

## Run Context

| Item | Value |
|---|---|
| Network | Esmeralda |
| Harness repository | `https://github.com/tzmWW/tari-wallet-benchmarks` |
| Mode 1 surface | `minotari_console_wallet` gRPC |
| Mode 2 surface | pinned `minotari` crate APIs |
| Mode 3 surface | real `minotari_payment_processor` plus companion minotari wallet |
| Environment | macOS 26.5.1, Apple M1 Pro, SSD, local base node `127.0.0.1` |
| Top-level confirmed transactions | 6 |

## Final Funding

Each mode started from one clean `10000 T` funding output. The three addresses
were funded by one three-recipient transaction, which is recorded as a public
input to the result profile.

| Mode | Amount | Tx ID | Height |
|---|---:|---|---:|
| `old_wallet` | `10000 T` | `5740188747787224553` | 725415 |
| `new_wallet` | `10000 T` | `5740188747787224553` | 725415 |
| `payment_processor` | `10000 T` | `5740188747787224553` | 725415 |

The local node proof for that funding block is documented in `RUNBOOK.md`.
Post-run DBs are mutated by the benchmark and are not clean starting wallets for
another final run; restore from the pre-run backup or fund fresh seeds before
running `preflight --check-funds` again.

## Cell Results

| Mode | Cell | Status | Successes | Failures | Wall ms |
|---|---|---|---:|---:|---:|
| `old_wallet` | B0 | ok | 1 | 0 | 2940036 |
| `old_wallet` | S0 | ok | 1 | 0 | 1638 |
| `old_wallet` | S1 | failed | 1 | 4 | 181192 |
| `old_wallet` | S2 | failed | 0 | 1 | 2949010 |
| `old_wallet` | S3 | failed | 0 | 1 | 32010 |
| `old_wallet` | S4 | failed | 2 | 246 | 106 |
| `old_wallet` | S5 | failed | 1 | 199 | 241234 |
| `old_wallet` | S6 | failed | 0 | 1 | 2938010 |
| `old_wallet` | S7 | failed | 0 | 1 | 30016 |
| `new_wallet` | B0 | failed | 0 | 1 | 457771 |
| `new_wallet` | S0 | ok | 1 | 0 | 823 |
| `new_wallet` | S1 | failed | 1 | 2 | 153979 |
| `new_wallet` | S2 | failed | 0 | 1 | 460088 |
| `new_wallet` | S3 | failed | 0 | 1 | 1311 |
| `new_wallet` | S4 | failed | 0 | 248 | 14292 |
| `new_wallet` | S5 | failed | 0 | 100 | 28252 |
| `new_wallet` | S6 | failed | 0 | 1 | n/a |
| `new_wallet` | S7 | failed | 0 | 1 | n/a |
| `payment_processor` | B0 | failed | 0 | 1 | 440584 |
| `payment_processor` | S0 | ok | 1 | 0 | 2217 |
| `payment_processor` | S1 | failed | 127 | 0 | 221 |
| `payment_processor` | S2 | failed | 0 | 1 | 457906 |
| `payment_processor` | S3 | failed | 0 | 1 | 1246 |
| `payment_processor` | S4 | failed | 95 | 153 | 9000207 |
| `payment_processor` | S5 | failed | 10 | 0 | 35 |
| `payment_processor` | S6 | failed | 0 | 1 | 466666 |
| `payment_processor` | S7 | failed | 0 | 1 | 1314 |

## Chain Evidence

Only mined transactions that reached `C_min` depth are emitted in
`chain_verification.verified_transactions`.

| Mode/Scenario | Confirmed txs | Mined height range |
|---|---:|---|
| `old_wallet/S1` | 1 | 726398 |
| `old_wallet/S4` | 2 | 726523 |
| `old_wallet/S5` | 1 | 726532 |
| `new_wallet/S1` | 1 | 726653 |
| `payment_processor/S1` | 1 | 726822 |

## Interpretation

- Mode 1 started from a real console-wallet recovery and passed S0. S1, S4, and
  S5 produced confirmed transactions but then exposed console-wallet pending-fund
  behavior under the spec's single-UTXO starting state. S2/S3/S6/S7 scan cells
  reached the local node tip but found more outputs/balance than the checkpoint
  expected after partial sends, so they are failed mismatch observations.
- Mode 2 started from exactly one clean funded output and passed S0. Its first
  S1 transaction was accepted and confirmed by independent base-node kernel
  query, then later S1/S4/S5 attempts hit `Funds are pending`. B0/S2/S3 fresh
  scans and PP companion scans that stopped far below the local tip are failed
  below-tip scanner observations with `tip_lag_blocks`.
- Mode 3 used the real payment processor plus companion wallet. PP accepted the
  full S1 and S5 batch shapes and part of the S4 ramp, but only one S1 batch
  reached confirmed chain evidence before the remaining batches stayed pending
  or hit PP/API contention. Pending PP rows remain metrics, not confirmed chain
  verification.
- Seed phrases and wallet passwords are excluded from result profiles. Public
  addresses, funding tx ids, and verified tx ids are intentionally present for
  auditability.

## Next Run Requirements

Another final run must fund fresh seeds, then pass `preflight --check-funds`
with exactly one spendable `A_fund` output per mode and no locked, pending,
invalid, cancelled, not-stored, or unknown outputs. Do not use the July 2 pre-run
backup as live restore state; later recoup attempts touched those backup DBs, so
they are historical evidence only.
