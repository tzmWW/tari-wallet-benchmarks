# Baseline Status

`esmeralda_baseline.json` is the complete schema-v5 joint baseline from the
uncapped `baseline-20260714T121001Z` run. Validate it with:

```sh
cargo run --release -- validate-profile --profile baselines/esmeralda_baseline.json --submission
```

The profile discloses two evidence-backed post-run corrections: one-sided
receive history is matched by chain output commitments because its wallet-local
history IDs differ from sender transaction IDs, and payment-processor balance
fields lost to concurrent SQLite reads were reconstructed from confirmed
payments, verified fees, fresh-scan balances, and final DB state. Timings and
wallet outcomes are unchanged. Genuine recovery, timeout, and database-lock
failures remain reported.
