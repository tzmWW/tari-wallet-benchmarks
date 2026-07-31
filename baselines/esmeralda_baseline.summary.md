# Tari Wallet Benchmark Result

- Run ID: `run-1785451471350207000-66061`
- Profile: `final`
- Complete: `true`
- Network: `esmeralda`
- Measurement commit: `fc37c668b3ff1ccbe94e343d80a7065db9fa0cf6`
- Export commit: `fc37c668b3ff1ccbe94e343d80a7065db9fa0cf6`
- Selected scan node: `http://127.0.0.1:18142` (`v5.4.0`; `local`)
- Independent authority: `https://rpc.esmeralda.tari.com` (`remote`)

| Mode | Scenario | Execution | Outcome | Median ms (ok) | Median ms (all) | API accepted | Chain confirmed | Rejected | Stalled | Timed out | Successes | Failures |
|---|---:|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| old_wallet | B0 | completed | success | 451007 | 451007 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| old_wallet | S0 | completed | success | 1042 | 1042 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| old_wallet | S1 | completed | success | 1388743 | 1388743 | 127 | 127 | 0 | 0 | 0 | 127 | 0 |
| old_wallet | S2 | completed | failure | — | 369007 | 0 | 0 | 0 | 0 | 0 | 0 | 1 |
| old_wallet | S3 | completed | failure | — | 5008 | 0 | 0 | 0 | 0 | 0 | 0 | 1 |
| old_wallet | S4 | completed | success | 880690 | 880690 | 15 | 248 | 0 | 0 | 0 | 248 | 0 |
| old_wallet | S5 | completed | success | 184521 | 184521 | 100 | 100 | 0 | 0 | 0 | 100 | 0 |
| old_wallet | S6 | completed | failure | — | 362008 | 0 | 0 | 0 | 0 | 0 | 0 | 1 |
| old_wallet | S7 | completed | failure | — | 6007 | 0 | 0 | 0 | 0 | 0 | 0 | 1 |
| new_wallet | B0 | completed | success | 81008 | 81008 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| new_wallet | S0 | completed | success | 171 | 171 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| new_wallet | S1 | completed | success | 754076 | 754076 | 127 | 127 | 0 | 0 | 0 | 127 | 0 |
| new_wallet | S2 | completed | success | 80198 | 80198 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| new_wallet | S3 | completed | success | 818 | 818 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| new_wallet | S4 | completed | failure | — | 4500018 | 247 | 159 | 1 | 0 | 88 | 159 | 89 |
| new_wallet | S5 | completed | success | 117527 | 117527 | 100 | 100 | 0 | 0 | 0 | 100 | 0 |
| new_wallet | S6 | completed | success | 81034 | 81034 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| new_wallet | S7 | completed | success | 1048 | 1048 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| payment_processor | B0 | completed | success | 80962 | 80962 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| payment_processor | S0 | completed | success | 3203 | 3203 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| payment_processor | S1 | completed | success | 1399383 | 1399383 | 127 | 127 | 0 | 0 | 0 | 127 | 0 |
| payment_processor | S2 | completed | success | 80340 | 80340 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| payment_processor | S3 | completed | success | 907 | 907 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| payment_processor | S4 | completed | failure | — | 913226 | 75 | 75 | 173 | 0 | 0 | 75 | 173 |
| payment_processor | S5 | completed | success | 150075 | 150075 | 10 | 10 | 0 | 0 | 0 | 10 | 0 |
| payment_processor | S6 | completed | success | 80838 | 80838 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| payment_processor | S7 | completed | success | 961 | 961 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |

Confirmed top-level transactions: **1073**
