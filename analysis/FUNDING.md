# Funding Evidence

These Esmeralda funding transactions were supplied by the operator after funding
the three harness-generated wallets with `50000 T` each.

| Mode | Amount | Transaction ID | Height |
|---|---:|---:|---:|
| `old_wallet` | `50000 T` | `11463237927696771510` | `707731` |
| `new_wallet` | `50000 T` | `7676530785144502866` | `707741` |
| `payment_processor` | `50000 T` | `4002233626181090692` | `707747` |

Funding is outside the measured benchmark path. The result profile records these
transactions as benchmark inputs so later scenario results can be tied back to
the funded wallet state without storing seed phrases or passwords.

## 2026-06-26 Recoup Audit

Several interrupted live runs left funds spread across ignored local wallet DBs.
Supported scans/rescans recovered the usable pools below:

- `HARNESS_SEED_OLD` console wallet: `818` spendable outputs,
  `15476.624585 T`. Four status-`4` console outputs totaling about `50303 T` are
  `Invalid` in the pinned console-wallet enum, not locked funds; do not count
  them as recoverable without upstream wallet repair.
- `HARNESS_SEED_NEW` / `HARNESS_SEED_NEW_FULL`: the latest recovered minotari DB
  `.bench-data/fund-old-final2-20260625T161138Z/new-wallet/wallet.db` has `15`
  spendable outputs, `38001.290980 T`, after scanning through the confirmed Mode
  1 funding tx.
- `HARNESS_SEED_PP`: `.bench-data/payment-receiver/wallet.db` has `1` spendable
  output, `49991.996505 T`, after scanning to reconcile the prior PP send.
- `HARNESS_SEED_OLD_FINAL2`: fresh recovery DB found `1` spendable output,
  `12000 T`, at height `711891`.
- `HARNESS_SEED_PP_FULL`: fresh recovery plus continued scan found the expected
  `150` confirmed `70 T` outputs, `10500 T` total, at heights `711784`-`711785`.
- `HARNESS_SEED_NEW_FRESH`: supported scan recovered `12` small spendable
  outputs, `4.274230 T`.

Do not double-count historical backup DBs: several contain older scan states for
the same seed. The usable post-recoup pool is roughly `125974 T` excluding the
invalid console-wallet outputs.

Fresh final-baseline funding was started from `.secrets/final-baseline.env`:

- Mode 1 first attempted console-wallet send tx `1052930016067525279` for
  `10000 T`, but the source wallet recorded `send_count=0`; querying the public
  base node by kernel signature returned `NotStored`, and a fresh recipient scan
  found no output.
- Mode 1 was re-funded from `.bench-data/recoup-old-final/wallet.db` with
  `fund-one-sided` tx `18012736798975040370` for `10000 T`. Public base-node
  query reported `Mined` at height `719363`, and a fresh check DB
  `.bench-data/final-baseline/old-check-719350/wallet.db` found `UNSPENT|1|10000000000`.
- Mode 2 fresh address received a `fund-one-sided` tx `941100472214723063` with
  `64 x 160 T` outputs. Recipient DB
  `.bench-data/final-baseline/new-wallet/wallet.db` shows
  `UNSPENT|64|10240000000`.
- PP fresh address received `50 x 70 T` from tx `15213625203447512294`, then
  another `100 x 70 T` from txs `16863047476553249751` and
  `6385847600539795173`. Recipient DB
  `.bench-data/final-baseline/payment-receiver/wallet.db` shows
  `UNSPENT|150|10500000000`.

Before the final no-cap run, `preflight --check-funds` passed against the final
DBs with required output counts: Mode 1 `1`, Mode 2 `64`, PP `150`. SQLite
backups were written under `.bench-data/_backups/pre-nocaps-20260629T230200Z`.

The final no-cap run wrote `baselines/esmeralda_baseline.json` with
`generated_at = 2026-06-29T23:04:13.700658Z`. It is a no-cap live evidence run,
not a three-repetition statistical profile: current live stateful send cells
still emit one repetition even when `benchmark.repetitions = 3`. The run records
real Mode 1 and Mode 2 pending-funds/selection failures and a successful no-cap
PP path; see `baselines/esmeralda_baseline.summary.md` for cell counts.

## 2026-06-30 Post-run Spendability

The pre-run backups under `.bench-data/_backups/pre-nocaps-20260629T230200Z`
prove that the no-cap evidence run started from clean spendable wallets:

- Mode 1 check DB:
  `.bench-data/final-baseline/old-check-719350/wallet.db` showed
  `UNSPENT|1|10000000000`.
- Mode 2 backup:
  `.bench-data/_backups/pre-nocaps-20260629T230200Z/new-wallet.wallet.db`
  showed `UNSPENT|64|10240000000`.
- PP receiver backup:
  `.bench-data/_backups/pre-nocaps-20260629T230200Z/payment-receiver.wallet.db`
  showed `UNSPENT|150|10500000000`.

Those backups are evidence of starting state, not reusable live state for another
benchmark run. The current post-run final DBs have locked/spent outputs:

- Final Mode 1 console wallet:
  `0|3|9995995390` plus `1|8|39992994615` (`0 = Unspent`,
  `1 = Spent`). It has enough spendable outputs for Mode 1 preflight, but it is
  no longer a pristine one-output S0 starting point.
- Final Mode 2 wallet:
  `LOCKED|64|10240000000` plus `UNSPENT|1|1000000`. This is not ready for a
  final rerun.
- Final PP receiver:
  `LOCKED|138|9660000000` plus `UNSPENT|12|840000000`. This is not ready for a
  final rerun.

Before another submission-quality run, first try supported scan/rescan recoup on
the final Mode 2 and PP receiver DBs. If locked state remains, fund fresh seeds
from known spendable pools and prove readiness with `preflight --check-funds`
before running the profile. The known spendable source pools include
`.bench-data/recovery-audit-20260625T122926Z/original-new/wallet.db`
(`UNSPENT|15|50001291680`) and `.bench-data/payment-receiver/wallet.db`
(`UNSPENT|1|49991996505`), but do not double-count older backup DBs for the same
seed.

## 2026-07-01 Final-rerun Readiness

The 2026-07-01 final-run attempts consumed additional Mode 1 and Mode 2 wallet
state before being interrupted in the Mode 1 S2 genesis recovery scan. They are
not final profiles, but the active final wallet paths were checked afterward and
remain spendable for another full uncapped attempt:

- Mode 1 final console wallet:
  `0(Unspent):759:10727996113`.
- Mode 2 final wallet:
  `SPENT|764|30896757960`, `UNSPENT|778|10565612275`.
- PP final receiver:
  `SPENT|138|9660000000`, `UNSPENT|151|10005737635`.

`cargo run --features live-minotari -- preflight --config harness.toml --check-funds`
passed with those active paths after the interrupted rerun. No benchmark,
console-wallet, minotari daemon, or payment-processor child processes were left
running.

Fresh ignored ready-state backups were written after that passing preflight:
`.bench-data/_backups/pre-next-uncapped-20260701T151331Z/`.

Before starting the next full live uncapped test:

1. Source `.secrets/final-baseline.env`.
2. Run `cargo run --features live-minotari -- preflight --config harness.toml --check-funds`.
3. Create fresh local backups of the three active wallet DBs under
   `.bench-data/_backups/`.
4. Start the run with a timestamped log under `logs/`.

Do not reuse `.bench-data/_backups/pre-nocaps-20260629T230200Z` as live state;
it remains starting-state evidence for the older no-cap run only.

## 2026-07-03 Final-submit Prep

The July 2/3 local-node baseline remains the current committed evidence, but the
active final-local wallets are not reusable for another submission run:

- Active Mode 1 final console wallet:
  `0(Unspent):2:9996997035`, which is two outputs and slightly below `A_fund`.
- Active Mode 2 final wallet:
  `LOCKED|1|10000000000`.
- Active PP receiver:
  `LOCKED|1|9989995275`, `SPENT|5|49979990550`.

`preflight --check-funds` fails on those paths, as intended. The July 2 pre-run
backup is also no longer a clean restore source for final-submit prep. The remote
recoup pass submitted sweeps from the backup Mode 2 and PP DBs, and current
base-node queries report those backup sweep txs as `NotStored`; the local backup
rows should be treated as stale wallet-state evidence, not spendable run state.

Current node health is good for a future run: local `http://127.0.0.1:18142`
matched the public Esmeralda tip at height `729012`, `is_synced=true`,
`pruning_horizon=0`, and local node status showed `Banned: 0`. The July 2
funding block hash remained queryable locally and returned height `725415` with
5 outputs.

Current treasury evidence is still insufficient for a fresh final run. Current
base-node queries confirm only these treasury-bound recoveries:
`11702729777062395322`, `15314222783538071776`, `9289358583519146549`,
`9752040590455831295`, and `15512637359623282638`, totaling
`13195.039400 T`. The four small sweep attempts and the two backup sweep
attempts are `NotStored` at tip `729013`. A fresh supported treasury scan still
stopped below the sweep heights at `max_height=722300` and detected zero
spendable outputs.

Fresh final-submit seed material was generated in
`.secrets/final-submit-20260703T224503Z.env`, with public addresses and exact
post-funding steps recorded in
`.bench-data/final-submit-20260703T224503Z/RUN_PREP.md`. The next final run must
fund those fresh addresses with exactly one `10000 T` output each, record the
new funding tx ids/heights in the ignored `harness.toml`, and pass strict
`preflight --check-funds` before creating a new pre-run backup and starting the
uncapped profile.
