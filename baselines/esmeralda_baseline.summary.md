# Tari Wallet Benchmark Result

- Run ID: `run-1785151147009533000-15398`
- Profile: `final`
- Complete: `true`
- Network: `esmeralda`
- Measurement commit: `769e75c365ed470fbb3d964df4f30d3763f0439e`
- Export commit: `ce86e0bc5a287edd3510ac35c810ff4185aa42b9`
- Selected scan node: `http://127.0.0.1:18142` (`v5.4.0`; `local`)
- Independent authority: `https://rpc.esmeralda.tari.com` (`remote`)

| Mode | Scenario | Execution | Outcome | Median ms (ok) | Median ms (all) | API accepted | Chain confirmed | Rejected | Stalled | Timed out | Successes | Failures |
|---|---:|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| old_wallet | B0 | completed | success | 414007 | 414007 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| old_wallet | S0 | completed | success | 1060 | 1060 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| old_wallet | S1 | completed | success | 1928820 | 1928820 | 127 | 127 | 0 | 0 | 0 | 127 | 0 |
| old_wallet | S2 | completed | failure | — | 363018 | 0 | 0 | 0 | 0 | 0 | 0 | 1 |
| old_wallet | S3 | completed | failure | — | 5014 | 0 | 0 | 0 | 0 | 0 | 0 | 1 |
| old_wallet | S4 | completed | failure | — | 1490613 | 16 | 240 | 8 | 0 | 0 | 240 | 8 |
| old_wallet | S5 | completed | failure | — | 174382 | 98 | 98 | 2 | 0 | 0 | 98 | 2 |
| old_wallet | S6 | completed | failure | — | 361008 | 0 | 0 | 0 | 0 | 0 | 0 | 1 |
| old_wallet | S7 | completed | failure | — | 6008 | 0 | 0 | 0 | 0 | 0 | 0 | 1 |
| new_wallet | B0 | completed | success | 77979 | 77979 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| new_wallet | S0 | completed | success | 198 | 198 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| new_wallet | S1 | completed | success | 1247945 | 1247945 | 127 | 127 | 0 | 0 | 0 | 127 | 0 |
| new_wallet | S2 | completed | success | 82858 | 82858 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| new_wallet | S3 | completed | success | 883 | 883 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| new_wallet | S4 | completed | failure | — | 4500023 | 247 | 148 | 1 | 0 | 99 | 148 | 100 |
| new_wallet | S5 | completed | success | 128839 | 128839 | 100 | 100 | 0 | 0 | 0 | 100 | 0 |
| new_wallet | S6 | completed | success | 86674 | 86674 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| new_wallet | S7 | completed | success | 1014 | 1014 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| payment_processor | B0 | completed | success | 78454 | 78454 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| payment_processor | S0 | completed | success | 2219 | 2219 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| payment_processor | S1 | completed | success | 1430483 | 1430483 | 127 | 127 | 0 | 0 | 0 | 127 | 0 |
| payment_processor | S2 | completed | success | 78470 | 78470 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| payment_processor | S3 | completed | success | 944 | 944 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| payment_processor | S4 | completed | failure | — | 1338282 | 82 | 82 | 166 | 0 | 0 | 82 | 166 |
| payment_processor | S5 | completed | success | 232059 | 232059 | 10 | 10 | 0 | 0 | 0 | 10 | 0 |
| payment_processor | S6 | completed | success | 78597 | 78597 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |
| payment_processor | S7 | completed | success | 972 | 972 | 0 | 0 | 0 | 0 | 0 | 1 | 0 |

Confirmed top-level transactions: **1059**
