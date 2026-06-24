# Esmeralda Baseline Summary

This summary accompanies `baselines/esmeralda_baseline.json`.

- Network: Esmeralda only.
- Harness repository: `https://github.com/tzmWW/tari-wallet-benchmarks`.
- Mode 1 surface: `minotari_console_wallet` gRPC.
- Mode 2 surface: pinned `minotari` crate APIs.
- Mode 3 surface: real `minotari_payment_processor` plus companion minotari
  wallet.
- Seed phrases and wallet passwords are excluded from result profiles.

Current checked-in live evidence:

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

The checked-in profile is not a completed all-mode performance baseline. It is
Mode 2 live evidence that preserves the wallet's current UTXO-locking behavior
instead of hiding it with harness-side pre-partitioning.
