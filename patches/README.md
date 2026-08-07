# Tracked Patches

This directory contains the three source patches used to build the benchmark's
pinned runtime artifacts. The fetch scripts verify each patch's SHA-256, the
resulting Git tree, and the complete source diff before building. Runtime
preflight then verifies the built artifacts against `tools/build-manifest.json`.

None of these patches changes wallet input selection or engineers around
contention, locking, transaction-shape failures, or other behavior that the
benchmark is intended to measure.

## `minotari-fixed-range-scan.patch`

**Source:** `tari-project/minotari-cli@360c4848a54d65fd710266233cc9277b0f785e74`

**Why it exists:** The upstream HTTP scanner can publish its completion marker
when block download finishes, before queued block batches have been processed
and persisted. A fixed-target scan may therefore report completion while
downloaded blocks remain unconsumed. Upstream also calculates the inclusive end
height of a partial scan incorrectly.

**What it changes:**

- Moves the completion marker from the downloader to the processing task, after
  queued batches have been consumed.
- Routes downloader errors through the scan-result channel.
- Stops adding blocks once the requested range end is reached.
- Calculates an inclusive `N`-block range as `start + N - 1`, with saturating
  arithmetic and focused tests.

**Benchmark impact:** This is a correctness patch required for deterministic
fixed-range B0 and recovery scans. It does not change transaction construction,
selection, signing, or broadcasting.

**If removed:** A scan can finish at a batch boundary before all downloaded
blocks are persisted, so the harness cannot reliably prove the requested scan
anchor or wallet state.

## `minotari-wallet-password-env.patch`

**Source:** the same pinned Minotari CLI source, applied after the fixed-range
scan patch for the runtime binary only.

**Why it exists:** Upstream requires the wallet password as `--password`. The
harness manages the Mode 3 payment-receiver process and should not expose that
password in its process arguments.

**What it changes:**

- Enables Clap's `env` feature.
- Allows `MINOTARI_WALLET_PASSWORD` to satisfy the required password argument.
- Hides the environment value from CLI help output.

**Benchmark impact:** This is an operational secret-handling patch. It does not
change scanning, transaction construction, selection, signing, or broadcasting.
The harness uses it in `src/payment_processor.rs` when starting the Mode 3
payment receiver.

**If removed:** The current payment-receiver command fails because it
intentionally omits `--password`. Passing the password in argv would restore
functionality but make the secret visible in ordinary process listings and
command diagnostics.

## `payment-processor-fee-rate.patch`

**Source:**
`tari-project/minotari_payment_processor@f0572c98cbfac7377412dc6d4094c7d7dfc5de2c`

**Why it exists:** Upstream hard-codes a fee rate of `5` microTari per gram. The
bounty requires the harness to expose and record a pinned `fee_rate`, and to pass
that rate explicitly to every wallet mode. A recorded setting that Mode 3
ignores would not satisfy that requirement, even when it happens to equal the
upstream constant.

**What it changes:**

- Requires `FEE_PER_GRAM` when constructing ordinary payment transactions.
- Uses the same value for payment-processor self-spend consolidation fees.
- Rejects a missing or non-unsigned-integer value instead of silently falling
  back to a different rate.

The harness sets `FEE_PER_GRAM` from `benchmark.fee_rate` in
`src/payment_processor.rs`.

**Benchmark impact:** Canonical baseline configuration pins the rate to `5`, so
this patch does not numerically change the published baseline relative to the
upstream constant. It makes the exposed configuration authoritative and also
supports noncanonical diagnostic runs with another explicitly selected rate.

**If removed:** Canonical transactions would currently retain the same fee rate
because upstream also uses `5`, but Mode 3 would ignore the harness setting and
the claim that one explicit fee-rate parameter controls all modes would be
false.

## Review And Provenance

The historical canonical application order, expected hashes, and result trees are in:

- `scripts/fetch-minotari-cli.sh`
- `scripts/fetch-payment-processor.sh`
- `build.rs`

Rolling development runs use `scripts/fetch-dev-stack.sh`. That script resolves
moving refs first, then records the actual patch hashes, result trees, complete
diff hashes, resolved commits, and built artifact hashes. A patch that no longer
applies is reported as a development compatibility break rather than disguised
as a canonical provenance mismatch.

The generated runtime manifest records the exact upstream revisions, ordered
patches, result trees, complete-diff hashes, and artifact hashes. Removing or
editing a patch requires regenerating all corresponding provenance values and
rebuilding the artifacts. Existing baseline profiles must continue to describe
the patched binaries with which they were measured.
