---
name: tari-wallet-benchmarks-implementation-review
description: Inspect, implement, independently review, and deterministically validate the Tari wallet benchmark plan; use for bounded repository implementation reviews.
workflow: tari-wallet-benchmarks-implementation-review
---

Use this manually triggered workflow to inspect the repository and `FIXES_PLAN.md`, preserve schema-v6 work, implement feasible changes, independently review them, and rerun deterministic quality gates. Reach for it when repository changes need bounded review-loop iteration without funded live benchmarks, commits, pushes, GitHub changes, or secret exposure.

Inputs are `prompt` (string, the task context), `repositoryPath` (string, default `/Users/andrea/Desktop/dev/tari-wallet-benchmarks`), `fixesPlanPath` (string, default `/Users/andrea/Desktop/FIXES_PLAN.md`), `baseCommit` (string, default `9dfe8cf`), and `maxFixIterations` (number, default `3`).

Start `.smithers/workflows/tari-wallet-benchmarks-implementation-review.tsx` with:

```sh
bunx smithers-orchestrator workflow run tari-wallet-benchmarks-implementation-review --prompt "Review and implement the feasible FIXES_PLAN.md changes."
```

For structured inputs, use `--input '{"prompt":"...","repositoryPath":"...","fixesPlanPath":"...","baseCommit":"9dfe8cf","maxFixIterations":3}'`; alternatively run `smithers up .smithers/workflows/tari-wallet-benchmarks-implementation-review.tsx`.

Run detached by adding `-d`, then watch it with `smithers ps`, `smithers logs <runId> -f`, and `smithers inspect <runId>`.

Visualize it with `bunx smithers-orchestrator graph .smithers/workflows/tari-wallet-benchmarks-implementation-review.tsx`; add `--interactive` for the TUI. The workflow has a custom UI, so open a run with `smithers ui <runId>`.

For blocked states, use `smithers approve <runId>` for approval gates, `smithers why <runId>` for signal waits, and `smithers cancel <runId>` to stop.

Suggest next: run it, watch it in the custom UI, and iterate by re-running `create-workflow` with a follow-up prompt.
