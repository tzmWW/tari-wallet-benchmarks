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

- Mode 2 S0 funded scan detected `50000000000` microtari available from funding
  tx `7676530785144502866` at height `707741`.
- Mode 2 S1 contains a compatibility smoke only: one `1 T` one-sided transaction
  was constructed, signed, persisted to the wallet DB, and accepted by
  Esmeralda through direct no-retry JSON-RPC submit. Tx id:
  `18389397492102525901`.

The checked-in profile is not a completed performance baseline. Replace it with
the full B0/S0-S7 matrix before using the numbers as benchmark evidence.
