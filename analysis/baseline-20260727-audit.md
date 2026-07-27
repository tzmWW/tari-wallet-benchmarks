# Baseline 2026-07-27 Audit

## Verdict

The uncapped `baseline-20260727T110514Z` run was promoted with explicit,
hash-bound reporting reconciliation. The promoted schema-v6 profile is
`baselines/esmeralda_baseline.json` and passes strict submission validation.

## Integrity

- Run window: `2026-07-27T11:06:57Z` to `2026-07-27T15:05:15Z`.
- Measurement commit: `769e75c365ed470fbb3d964df4f30d3763f0439e`.
- Export commit: `ce86e0bc5a287edd3510ac35c810ff4185aa42b9`.
- Raw checkpoint SHA-256:
  `8d5650d7fdf360b3f7e0a625c680fc9ca64e5c530926eb069c18f8ea5b157017`.
- Promoted profile SHA-256:
  `5f43e00a30292ed6301c45e7b9c0f02ea26f8e426f73f057d9b9658344c9425d`.
- Reconciliation manifest SHA-256:
  `67a49451c6410e46744ab4b59aac0a13b69c99f4fe3b880beaaabe0546fdc5bf`.
- Sanitized evidence SHA-256:
  `45349fa7dbddaf5c5d75e28fe5707fed9425fc858eaf5a4f674078e819294467`.
- Final anchor: height `785232`, hash
  `1f188d6ee47b990b5d086792c987043821335bc54f63a4c976074cdb55a1520b`.
  The final companion scan DB persisted this header at `2026-07-27 15:05:13`,
  and the independent remote authority returns the same immutable header.
- The run used the canonical uncapped configuration: 512 S1 outputs, six
  doubling rounds plus fan-out, S4 `[8,16,32,64,128]`, 900-second arm budget,
  S5 `M=100/K=10`, `C_min=3`, and `5 uT` fee rate.

## Reporting Reconciliation

1. Mode 1 CoinSplit returned a transaction ID but no transfer-result timing
   vector. Shared enrichment incorrectly interpreted the empty vector as an API
   rejection, although all 127 transactions subsequently passed independent
   C-min verification. The reconciliation joins the ordered IDs to timestamped
   accepted-submit logs, status-6 console DB rows, one spent input, 63 two-output
   and 64 eight-output shapes, 638 output commitments, and timestamped wallet tip
   observations. Derived dispatch-to-C-min durations range from 178,690 ms to
   319,028 ms. Scenario wall time and all chain values remain unchanged.
2. Mode 1 and Mode 2 S1 split wallet-owned inputs into wallet-owned outputs.
   Their confirmed observation amounts are gross internal transaction values,
   not external debits. Balance reconciliation therefore uses zero external
   outgoing plus confirmed paid fees. The original balance observations and
   fee totals are unchanged.
3. Mode 1 S4 returned 224 transaction-identified `NotFound ... within timeout`
   gRPC responses that later independently confirmed. The API errors and API
   acceptance count remain recorded; validation now permits only this pinned
   Mode 1 reconciliation shape. The eight unreconciled attempts remain rejected.
4. The final stage checkpoint was written before finalization failed. The
   reconciliation marks it final and restores the end anchor from the persisted
   final scan and matching authority header.

## Scenario Audit

| Mode | Scenario | Outcome | Wall ms | Confirmed | Failures | Audit conclusion |
|---|---:|---|---:|---:|---:|---|
| Old | B0 | success | 414007 | 0 | 0 | Empty genesis scan completed. |
| Old | S0 | success | 1060 | 0 | 0 | Exact shared `10000 T` funding state. |
| Old | S1 | success | 1928820 | 127 | 0 | Exact 512-output plan; observation linkage reconciled. |
| Old | S2 | failure | 363018 | 0 | 1 | Genuine recovery mismatch: 639 spendable vs 512 expected. |
| Old | S3 | failure | 5014 | 0 | 1 | Same birthday-scan spent-state mismatch. |
| Old | S4 | failure | 1490613 | 240 | 8 | 224 false NotFound responses reconciled; 8 genuine failures. |
| Old | S5 | failure | 174382 | 98 | 2 | Two genuine SQLite lock failures. |
| Old | S6 | failure | 361008 | 0 | 1 | Genuine recovery mismatch: 977 spendable vs 512 expected. |
| Old | S7 | failure | 6008 | 0 | 1 | Same post-S5 spent-state mismatch. |
| New | B0/S0 | success | 77979/198 | 0 | 0 | Empty scan and exact funding passed. |
| New | S1 | success | 1247945 | 127 | 0 | Full self-directed 512-output plan. |
| New | S2/S3 | success | 82858/883 | 0 | 0 | Both fixed-target scans passed. |
| New | S4 | failure | 4500023 | 148 | 100 | 99 timed out and one duplicate-pending rejection. |
| New | S5 | success | 128839 | 100 | 0 | All distinct-recipient sends confirmed. |
| New | S6/S7 | success | 86674/1014 | 0 | 0 | Both post-S5 scans passed. |
| PP | B0/S0 | success | 78454/2219 | 0 | 0 | Companion scan and exact funding passed. |
| PP | S1 | success | 1430483 | 127 | 0 | Full 512-output batch plan. |
| PP | S2/S3 | success | 78470/944 | 0 | 0 | Both fixed-target scans passed. |
| PP | S4 | failure | 1338282 | 82 | 166 | Genuine payment-processor HTTP 500 contention. |
| PP | S5 | success | 232059 | 10 | 0 | Ten batches, 100 payments, all confirmed. |
| PP | S6/S7 | success | 78597/972 | 0 | 0 | Both post-S5 companion scans passed. |

The promoted profile contains 1,059 confirmed top-level transaction rows. Failed
cells retain their measured all-run wall times and structured failure evidence.
