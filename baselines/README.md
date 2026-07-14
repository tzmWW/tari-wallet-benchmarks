# Baseline Status

The former files in this directory were schema-v3 diagnostic output and were
not valid under the current schema-v5 submission contract. They were removed to
avoid presenting obsolete evidence as a deliverable.

No replacement is fabricated during implementation. After an explicitly
authorized fresh, uncapped run passes:

```sh
cargo run --release -- validate-profile --profile candidates/esmeralda-baseline.json --submission
```

promote that JSON as `baselines/esmeralda_baseline.json` and generate
`baselines/esmeralda_baseline.summary.md` with `summarize-profile`.
