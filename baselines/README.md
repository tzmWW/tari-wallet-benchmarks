# Baseline Status

`esmeralda_baseline.json` is the schema-v6 profile from uncapped run
`baseline-20260730T220138Z`. It contains all 27 benchmark cells and passes strict
submission validation. Failed cells are retained as measured wallet outcomes.
`esmeralda_baseline.summary.md` is its deterministic human-readable summary.
The profile run ID is `run-1785451471350207000-66061`; its SHA-256 is
`e00d8f9ea4711f825bb81e8af3abfc3a02a9831cc3f3030483aea0b39400a024`.
These files are immutable historical publication evidence. Current schema-v7
development results stay outside `baselines/` and do not replace them.

```sh
cargo run --release -- validate-profile --profile baselines/esmeralda_baseline.json --submission
cargo run --release -- summarize-profile --profile baselines/esmeralda_baseline.json --out /tmp/esmeralda_baseline.summary.generated.md
cmp -s /tmp/esmeralda_baseline.summary.generated.md baselines/esmeralda_baseline.summary.md
```

The harness measured at commit `fc37c668b3ff1ccbe94e343d80a7065db9fa0cf6`.
Run provenance, integrity records, and the detailed scenario assessment are in
[`analysis/baseline-20260731-audit.md`](../analysis/baseline-20260731-audit.md).
