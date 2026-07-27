# Baseline Status

`esmeralda_baseline.json` is the promoted schema-v6 profile from uncapped run
`baseline-20260727T110514Z`. It contains all 27 benchmark cells and passes strict
submission validation. `esmeralda_baseline.summary.md` is its deterministic
human-readable summary.

```sh
cargo run --release -- validate-profile --profile baselines/esmeralda_baseline.json --submission
cargo run --release -- summarize-profile --profile baselines/esmeralda_baseline.json --out /tmp/esmeralda_baseline.summary.generated.md
cmp -s /tmp/esmeralda_baseline.summary.generated.md baselines/esmeralda_baseline.summary.md
```

The harness measured at commit `769e75c365ed470fbb3d964df4f30d3763f0439e`.
Run provenance, integrity records, and the detailed scenario assessment are in
[`analysis/baseline-20260727-audit.md`](../analysis/baseline-20260727-audit.md).
