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
