# Baseline Status

`esmeralda_baseline.json` is the historical schema-v5 joint baseline from the
uncapped `baseline-20260714T121001Z` run. Validate it with the explicit legacy
path (this does not recast it as schema-v6 evidence):

```sh
cargo run --release -- validate-profile --profile baselines/esmeralda_baseline.json --legacy-v5
cargo run --release -- summarize-profile --profile baselines/esmeralda_baseline.json --legacy-v5 --out /tmp/esmeralda_baseline.summary.generated.md
cmp -s /tmp/esmeralda_baseline.summary.generated.md baselines/esmeralda_baseline.summary.md
```

The profile discloses evidence-backed post-run reporting corrections: one-sided
receive history is matched by chain output commitments because its wallet-local
history IDs differ from sender transaction IDs, and payment-processor balance
fields lost to concurrent SQLite reads were reconstructed from confirmed
payments, verified fees, fresh-scan balances, and final DB state. Timings and
wallet outcomes are unchanged. The Mode 2 S1 note was also corrected to describe
balanced no-change children rather than the unrelated configured payment amount.
Genuine recovery, timeout, and database-lock failures remain reported.

New harness output uses schema-v6. Validate non-funded schema-v6 fixtures with
the normal command; the committed baseline is intentionally not rewritten here
because no new funded benchmark was run.

No authenticated raw schema-v6 artifact is present in this repository, so no
corrected schema-v6 baseline is claimed. When an authenticated raw profile is
available, `scripts/correct-profile.py` applies only the JSON Pointer/value
mutations listed in a hash-bound manifest and prints the resulting hashes. A
funded schema-v6 rerun remains external work.
