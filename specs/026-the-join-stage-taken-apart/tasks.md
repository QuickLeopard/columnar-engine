# Spec 026 — The join's stage, taken apart — tasks

**Status**: in progress
**Spec**: [spec.md](spec.md)

## The instrument

- [ ] `crates/columnar-engine/examples/join_cost_probe.rs`, a new diagnostic that takes the join's
      stage apart: the stage as the plan output reports it on **both** paths, the build side's
      **open**, the build side's **read**, the build side's **keys**, the **table**, the **probe**
      measured both ways, and the **output**.
- [ ] The probe runs the engine's **own** plan for `join-orders` through the engine's own entry
      point and takes the stage from the plan output, so its stage is the engine's stage and not a
      copy of it.
- [ ] The engine's 216,783 rows are checked against the probe's own, **in order**, before anything
      is timed: a probe that measures a different join is not a measurement.
- [ ] The **controls**: the probe key column read plain, the floor the probe cannot go below; the
      same read over a buffer small enough to be in the caches, which says whether a read waits for
      memory; and the build side's columns read on their own.
- [ ] Three campaigns on the same host, the reports committed as
      `bench-results/join-cost-run{1,2,3}.json` -- and the same probe against the code this spec
      replaces, committed as `bench-results/join-cost-before-run{1,2,3}.json`.

## The separation

- [ ] The stage and its parts are measured **in the same session**, on the same data, on the same
      rows: the stage's figure and the parts' figures have never been measured against each other.
- [ ] The **two paths** are measured the same way: the cold path, where the scan feeds the join,
      and the resident path, where the join probes the dataset itself.
- [ ] The **2.9 ms** between the two paths' stages is attributed to a part, or **named** as the
      remainder.
- [ ] The two paths' row counts are made comparable, or the difference between them is named in the
      plan output itself.

## The decomposition

**The review, bottom-up and top-down, found five rules with two owners each, a stage that reports
two different things for the same work, and an estimate that is not an upper bound.** The fixes come
first, in steps small enough to build, test and measure on their own, and the instrument measures
the stage **after** them — so the published numbers are the numbers of the code that ships.

- [x] **Step 1 — the stage reports the rows the probe consumed.** The reader reports how many rows
      the filter examined, and the join's stage reports that. The two paths then say
      `6000000 → 216783` alike.
- [x] **Step 2 — the key's interface says how long the column is.** `KeySource` gains `len()`, and the
      range check is made **once**, so "the row is in range" is one answer on both paths instead of
      `None` on one and a panic on the other.
- [x] **Step 3 — the two key sources are compared.** A differential test holds `KeyColumn` and
      `ArrowKeys` to the same columns — NULLs, strings, dictionaries, the plain-integer fast path,
      and a row past the end — and they must agree, value for value.
- [x] **Step 4 — the type check is made once, from one source.** The two keys' types are resolved
      where both are known, and an absent column is **refused** rather than silently typed `Int64`.
- [x] **Step 5 — the read decision is made once.** The reader owns it — `ParquetSource::decide_read`
      — and every read asks it: the probe side's scan with the plan's materialised set, and the
      build side with its own key as its materialised set. The operator no longer re-derives the
      rules, and it no longer reaches up into `crate::scan`; the pushdown's list of the types it
      evaluates exactly moved into the pushdown's own module. The decision moved **down** to the
      reader rather than staying in `scan.rs`, because the build side and the estimate are below the
      scan and nothing below may depend on anything above it; the reason is recorded in the spec and
      in ADR-0034.
- [x] **Step 6 — the estimate is charged the way the read happens.** The build side's estimate counts
      the file's rows, the read set the **reader** decides to read, the copies that read holds, the
      keys the join collects, and the filter's own mask — and it is **tested**, the way the other
      estimates are. The estimate asks the reader the same question the join asks, so the two cannot
      drift apart.
- [ ] **Step 7 — the probe's type support is published by the key module.** The scan asks
      `ArrowKeys` what the probe can read, instead of borrowing the pushdown's list, so a wrong "yes"
      is refused at planning time rather than failing mid-read.
- [x] **Step 8 — the mapping is the plan's.** The join's copy of "the file's column is the dataset's
      column" went with the read decision: the decision carries the plan's `ColumnMapping`, and the
      silent `unwrap_or` fallback that spec 027 removed from the plan's copy is gone for good.
- [ ] **Step 9 — the join's helpers take the context.** `ordered_stage`, `join_outcome` and
      `build_streamed_table` are handed the pipeline's context, the way the other stages are.
- [ ] **Step 10 — the documents say what the code does, and the dead surface goes.** The two stale
      documents are corrected, and `KeyColumn::from_decoded`, `KeySlice`'s unread dictionary variants
      and `KeySlice::integers` are removed.

## The change

- [ ] Whatever the measurement shows is **not** the join's own work is removed -- or, if it cannot be
      removed, it is named and published.
- [ ] The build side's read is named **inside** the join's stage, on both paths, so the asymmetry
      with the incumbents is visible rather than assumed.
- [ ] The plan output's stage and its parts agree, or the difference between them is named in the
      plan output itself.

## The answer does not change

- [ ] The 216,783 rows the join produces are the same rows, in the same order, with the same
      values.
- [ ] A NULL key never matches, on either side, and the multiplicity -- one output row per match --
      does not change.
- [ ] The aggregate's value is the same number, bit for bit.

## The publication

- [ ] `PERF.md`: the join's stage, part by part, the two paths beside each other, and the
      before-and-after.
- [ ] `README.md`: the status table, and the next question.
- [ ] `bench-results/README.md`: the new reports, and the recipe that writes them.
- [ ] `scripts/medians.py` derives the new tables; `scripts/check-published-figures.py` checks every
      published figure against the committed reports, **in the section that publishes it**.
- [ ] The comparison campaigns re-measured, for the code and for the code it replaced, back to back
      on the same host.

## The gate

- [ ] `cargo fmt`, clippy on arm64 and x86-64, the whole test suite, `cargo deny`, the wasm32
      check, the published-figures guard and the page checks all green.
