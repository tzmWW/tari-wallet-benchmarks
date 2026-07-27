# Baseline Status

`esmeralda_baseline.json` is the promoted schema-v6 profile from uncapped run
`baseline-20260727T110514Z`. It contains all 27 benchmark cells and passes strict
submission validation:

```sh
cargo run --release -- validate-profile --profile baselines/esmeralda_baseline.json --submission
cargo run --release -- summarize-profile --profile baselines/esmeralda_baseline.json --out /tmp/esmeralda_baseline.summary.generated.md
cmp -s /tmp/esmeralda_baseline.summary.generated.md baselines/esmeralda_baseline.summary.md
```

The harness measured at commit `769e75c365ed470fbb3d964df4f30d3763f0439e`.
Reporting corrections were exported with commit
`ce86e0bc5a287edd3510ac35c810ff4185aa42b9`.

## Correction Artifacts

- `esmeralda_baseline.raw.json`: byte-exact final stage checkpoint.
- `esmeralda_baseline.correction-evidence.json`: sanitized Mode 1 S1 DB/log
  evidence and end-anchor evidence; it excludes seeds, private keys, and
  serialized transactions.
- `esmeralda_baseline.correction.json`: SHA-256-bound JSON Pointer correction
  manifest.
- `scripts/correct-profile.py`: generic manifest applicator.
- `scripts/build-mode1-s1-correction.py`: fail-closed evidence extractor and
  manifest generator.
- `analysis/baseline-20260727-audit.md`: correction rationale and scenario audit.

The correction links the 127 Mode 1 CoinSplit IDs to their raw accepted-submit,
console DB, shape, commitment, and C-min evidence; treats Mode 1/2 S1 as
self-directed zero-net-outgoing splits; and restores the final chain anchor.
No scenario wall time, fee, transaction result, scan result, or wallet failure
was changed. Genuine recovery mismatches, lock failures, timeouts, and HTTP 500
responses remain failures in the promoted profile.
