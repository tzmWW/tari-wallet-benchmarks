# Esmeralda Baseline Summary

This summary accompanies `baselines/esmeralda_baseline.json`.

- Network: Esmeralda only.
- Harness repository: `https://github.com/tzmWW/tari-wallet-benchmarks`.
- Mode 1 surface: `minotari_console_wallet` gRPC.
- Mode 2 surface: pinned `minotari` crate APIs.
- Mode 3 surface: real `minotari_payment_processor` plus companion minotari
  wallet.
- Seed phrases and wallet passwords are excluded from result profiles.

The checked-in profile is a schema-valid harness output, not a completed
performance baseline. Replace it with a funded live Esmeralda run after the full
B0/S0-S7 runner is completed before using the numbers as benchmark evidence.
