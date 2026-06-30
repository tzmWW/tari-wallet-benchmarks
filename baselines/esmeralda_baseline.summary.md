# Esmeralda Baseline Summary

This summary accompanies `baselines/esmeralda_baseline.json`.

The checked-in profile was regenerated on `2026-06-29T23:04:13.700658Z` with schema v3. It is no-cap send evidence with `live_fresh_scan_cells = true`, all live caps set to `0`, and `scan_repetitions = 1` for long scan cells, but it used `concurrent_batches = [1]` and therefore only contains a partial S4 ramp. The config records `repetitions = 3`, but the current live stateful send paths still emit one repetition per scenario. Treat this profile as valuable wallet-behavior evidence, not the final bounty reference baseline.

## Run Context

| Item | Value |
|---|---|
| Network | Esmeralda |
| Harness repository | `https://github.com/tzmWW/tari-wallet-benchmarks` |
| Mode 1 surface | `minotari_console_wallet` gRPC |
| Mode 2 surface | pinned `minotari` crate APIs |
| Mode 3 surface | real `minotari_payment_processor` plus companion minotari wallet |
| Environment | macOS 26.5.1, Apple M1 Pro, SSD, remote `rpc.esmeralda.tari.com` |
| Top-level verified transactions | 208 |

## Final Funding

| Mode | Amount | Tx ID | Height |
|---|---:|---|---:|
| `old_wallet` | `10000 T` | `18012736798975040370` | 719363 |
| `new_wallet` | `10240 T` | `941100472214723063` | 712153 |
| `payment_processor` | `10500 T` | `15213625203447512294`, `16863047476553249751`, `6385847600539795173` | 712161 |

Pre-run fund preflight passed with `old_wallet = 1 x 10000 T`, `new_wallet = 64 x 160 T`, and `payment_processor = 150 x 70 T` spendable outputs. Post-run Mode 2 and PP state is locked/spent enough that another final run needs recoup or fresh funding before `preflight --check-funds` can pass again.

## Cell Results

| Mode | Cell | Status | Successes | Failures | Wall ms |
|---|---|---|---:|---:|---:|
| `old_wallet` | B0 | ok | 1 | 0 | 2321439 |
| `old_wallet` | S0 | ok | 1 | 0 | 24649 |
| `old_wallet` | S1 | failed | 2 | 2 | 401165 |
| `old_wallet` | S2 | ok | 1 | 0 | 2892440 |
| `old_wallet` | S3 | ok | 1 | 0 | 24465 |
| `old_wallet` | S4 | ok | 1 | 0 | 31 |
| `old_wallet` | S5 | failed | 3 | 197 | 361033 |
| `old_wallet` | S6 | ok | 1 | 0 | 2588512 |
| `old_wallet` | S7 | ok | 1 | 0 | 19443 |
| `new_wallet` | B0 | ok | 1 | 0 | 778814 |
| `new_wallet` | S0 | ok | 1 | 0 | 1924 |
| `new_wallet` | S1 | failed | 64 | 63 | 871093 |
| `new_wallet` | S2 | ok | 1 | 0 | 841665 |
| `new_wallet` | S3 | ok | 1 | 0 | 6474 |
| `new_wallet` | S4 | failed | 0 | 1 | 286 |
| `new_wallet` | S5 | failed | 0 | 100 | 28227 |
| `new_wallet` | S6 | failed | 0 | 1 | n/a |
| `new_wallet` | S7 | failed | 0 | 1 | n/a |
| `payment_processor` | B0 | ok | 1 | 0 | 1134332 |
| `payment_processor` | S0 | ok | 1 | 0 | 2209 |
| `payment_processor` | S1 | ok | 127 | 0 | 336 |
| `payment_processor` | S2 | ok | 1 | 0 | 837108 |
| `payment_processor` | S3 | ok | 1 | 0 | 6352 |
| `payment_processor` | S4 | ok | 1 | 0 | 123006 |
| `payment_processor` | S5 | ok | 10 | 0 | 45 |
| `payment_processor` | S6 | ok | 1 | 0 | 760019 |
| `payment_processor` | S7 | ok | 1 | 0 | 6527 |

## Chain Evidence

| Mode/Scenario | Confirmed txs | Mined height range |
|---|---:|---|
| `old_wallet/S1` | 2 | 719444-719449 |
| `old_wallet/S4` | 1 | 719581 |
| `old_wallet/S5` | 3 | 719593-719599 |
| `new_wallet/S1` | 64 | 719704-719731 |
| `payment_processor/S1` | 127 | 720060-720061 |
| `payment_processor/S4` | 1 | 720090 |
| `payment_processor/S5` | 10 | 720095 |

## Interpretation

- Mode 1 started from a fresh final console-wallet recovery and proved S0/S4 plus all scan cells. S1 stopped during round 2 after the console wallet returned `OutputManagerError(NotEnoughFunds)`, and S5 recorded pending-funds failures after three confirmed sends. These failures are recorded as benchmark signal, not hidden with harness retries.
- Mode 2 proved 64 no-cap S1 transactions through independent base-node kernel-signature queries. The final fan-out arm then failed 63 attempts with `Funds are pending`; S4 and S5 also failed for the same pending-funds state. Confirmed Mode 2 evidence remains top-level `chain_verification`; failed attempts remain in cell metrics/notes. S6/S7 are explicit failed rows because S5 did not produce a runnable checkpoint.
- Mode 3 completed the no-cap PP path: S1 accepted and confirmed 127 payment batches, S4 confirmed one batch, S5 confirmed ten batches, and all PP companion scan cells completed.
- Seed phrases and wallet passwords are excluded from result profiles. Public addresses, funding tx ids, and verified tx ids are intentionally present for auditability.

## Final Rerun Needed

The next submission-quality run should use `concurrent_batches = [8, 16, 32, 64, 128]`, `repetitions = 1`, `scan_repetitions = 1`, `live_fresh_scan_cells = true`, and all live caps at `0`. Run it only after Mode 1, Mode 2, and PP fund preflight passes with no locked, pending, invalid, cancelled, not-stored, or unknown outputs.
