# Tari Wallet Benchmark Result

- Run ID: `run-1784031664818940000-77079`
- Profile: `final`
- Complete: `true`
- Network: `esmeralda`
- Harness commit: `0b5d4f4ed3ec00cc19610021b3a042959914b281`
- Selected scan node: `http://127.0.0.1:18142` (`v5.4.0`; `local`)
- Independent authority: `https://rpc.esmeralda.tari.com` (`remote`)

| Mode | Scenario | Execution | Outcome | Median ms | Successes | Failures |
|---|---:|---|---|---:|---:|---:|
| old_wallet | B0 | completed | success | 357012 | 1 | 0 |
| old_wallet | S0 | completed | success | 1066 | 1 | 0 |
| old_wallet | S1 | completed | success | 1148696 | 127 | 0 |
| old_wallet | S2 | completed | failure | — | 0 | 1 |
| old_wallet | S3 | completed | failure | — | 0 | 1 |
| old_wallet | S4 | completed | success | 710643 | 248 | 0 |
| old_wallet | S5 | completed | success | 174480 | 100 | 0 |
| old_wallet | S6 | completed | failure | — | 0 | 1 |
| old_wallet | S7 | completed | failure | — | 0 | 1 |
| new_wallet | B0 | completed | success | 71579 | 1 | 0 |
| new_wallet | S0 | completed | success | 155 | 1 | 0 |
| new_wallet | S1 | completed | success | 873439 | 127 | 0 |
| new_wallet | S2 | completed | success | 72290 | 1 | 0 |
| new_wallet | S3 | completed | success | 799 | 1 | 0 |
| new_wallet | S4 | completed | failure | — | 119 | 129 |
| new_wallet | S5 | completed | success | 105741 | 100 | 0 |
| new_wallet | S6 | completed | success | 71466 | 1 | 0 |
| new_wallet | S7 | completed | success | 997 | 1 | 0 |
| payment_processor | B0 | completed | success | 71020 | 1 | 0 |
| payment_processor | S0 | completed | success | 2210 | 1 | 0 |
| payment_processor | S1 | completed | success | 1106488 | 127 | 0 |
| payment_processor | S2 | completed | success | 72363 | 1 | 0 |
| payment_processor | S3 | completed | success | 799 | 1 | 0 |
| payment_processor | S4 | completed | failure | — | 84 | 164 |
| payment_processor | S5 | completed | success | 83051 | 10 | 0 |
| payment_processor | S6 | completed | failure | — | 0 | 1 |
| payment_processor | S7 | completed | failure | — | 0 | 1 |

Confirmed top-level transactions: **1042**
