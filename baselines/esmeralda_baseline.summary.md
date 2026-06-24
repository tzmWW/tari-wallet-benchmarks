# Esmeralda Baseline Summary

This summary accompanies `baselines/esmeralda_baseline.json`.

Note: this checked-in JSON predates schema v3 and the later Mode 2 S1
multi-recipient/settlement hardening. The current harness records
scenario-specific metrics, transaction verification rows, Mode 1 S1 `CoinSplit`
rounds, Mode 2 self-directed doubling/fan-out rounds, scanner-backed Mode 2
settle gates, and deterministic distinct-recipient S5 shapes. Regenerate the
baseline after a capped proof passes and enough spendable Esmeralda funds are
available.

- Network: Esmeralda only.
- Harness repository: `https://github.com/tzmWW/tari-wallet-benchmarks`.
- Mode 1 surface: `minotari_console_wallet` gRPC.
- Mode 2 surface: pinned `minotari` crate APIs.
- Mode 3 surface: real `minotari_payment_processor` plus companion minotari
  wallet.
- Seed phrases and wallet passwords are excluded from result profiles.

Current checked-in live evidence:

- Mode 1 S0 started a real `minotari_console_wallet` process with gRPC enabled.
  The harness rewrote only the mnemonic birthday before launch because console
  wallet seed recovery reads the birthday embedded in the seed words. Recovery
  found `50003000000` microtari available for old-wallet funding tx
  `11463237927696771510` at height `707731`.
- Mode 1 S1 submitted one capped `1 T` gRPC `Transfer` request. It succeeded in
  `41 ms` with fee `660` microtari and tx id `6866732622268686463`.
- Mode 1 S4 submitted one capped concurrent-batch gRPC transfer. It succeeded in
  `27 ms` with fee `700` microtari and tx id `3905119411675345783`.
- Mode 1 S5 submitted one two-recipient `single_tx=true` gRPC transfer after
  S1/S4 had locked funds. It failed with `Funds are still pending`, preserving
  real wallet lock behavior as benchmark signal.
- Mode 2 S0 funded scan detected `49998999300` microtari available after the
  earlier compatibility smoke, tied to funding tx `7676530785144502866` at
  height `707741`.
- Mode 2 S1 live send cell attempted 2 capped `1 T` one-sided sends. The first
  was constructed, signed, persisted, and accepted by Esmeralda through direct
  no-retry JSON-RPC submit; tx id `6699431803862839962`. The second failed with
  `Funds are pending`.
- Mode 2 S4 live cell dispatched capped concurrent batches for configured
  batch sizes `[8, 16, 32, 64, 128]` with 2 attempts each. All 10 attempts
  failed with the same pending-funds condition after S1 locked the large
  available output/change.
- Mode 2 S5 individual-send arm attempted 2 capped sends. Both failed with the
  same pending-funds condition.
- SQLite inspection after the run showed two broadcast completed transactions
  total, including the prior smoke and new S1 tx, plus one locked output worth
  `49998999300` microtari tied to the new S1 pending transaction.
- Mode 3 S0 started the real `minotari_payment_processor` plus companion
  `minotari daemon` payment receiver. The receiver view wallet used effective
  birthday `1635` and detected `49999000000` microtari from funding tx
  `4002233626181090692` at height `707747`.
- Mode 3 S1 submitted one capped `1 T` `/v1/payment-batches` request. PP accepted
  batch `d30a3dd8-7243-47ce-a5cc-c66496815fbe`; the profile snapshot captured
  unsigned and signed tx JSON with status `AWAITING_BROADCAST`, and later SQLite
  inspection showed the batch reached `CONFIRMED` at height `708613`.
- Mode 3 S4 accepted one capped PP batch for each configured concurrency tier
  `[8, 16, 32, 64, 128]`. Those later batches remained `PENDING_BATCHING` because
  the first PP transaction locked the single large funded UTXO/change while it
  awaited confirmation.
- Mode 3 S5 accepted one capped two-item PP batch and likewise remained
  `PENDING_BATCHING` under the same single-UTXO lock condition.

The checked-in profile is not a completed all-mode performance baseline. It is
capped Mode 1, Mode 2, and Mode 3 live evidence that preserves the wallets'
current UTXO-locking behavior instead of hiding it with harness-side
pre-partitioning.
