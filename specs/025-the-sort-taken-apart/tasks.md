# Spec 025 — The sort, taken apart — tasks

**Status**: done
**Spec**: [spec.md](spec.md)

## The instrument

- [x] `crates/columnar-engine/examples/sort_cost_probe.rs`, a new diagnostic that takes the sort's
      stage apart: the stage as the plan output reports it, the operator's own call, the rows'
      hand-over, the key column's construction, the plain read of the key column, the **control**
      that says whether the read waits for memory, and the **vectorised** comparison.
- [x] The probe runs the engine's **own** plan through the engine's own entry point and takes the
      stage from the plan output, so its stage is the engine's stage and not a copy of it.
- [x] The engine's ten rows are checked against the probe's own, **in order**, before anything is
      timed: a probe that measures a different sort is not a measurement.
- [x] Three campaigns on the same host, the reports committed as
      `bench-results/sort-cost-run{1,2,3}.json` -- and the same probe against the code this spec
      replaces, committed as `bench-results/sort-cost-before-run{1,2,3}.json`.

## The separation

- [x] The stage and the operator are measured **in the same session**, on the same data, on the same
      rows: the two published figures have never been measured against each other.
- [x] The **cache state** is measured: the operator is measured with the caches pushed out and with
      the column already in them, and the read's cost per value over a small buffer is measured
      against the same read over the whole column.
- [x] The **comparison** is measured without the read: the same column, compared in blocks.
- [x] The difference between the stage and the operator is attributed to a part, or **named** as the
      remainder.

## The change

- [x] Whatever the measurement shows is **not** the sort's own work is removed -- or, if it cannot be
      removed, it is named and published.
- [x] The plan output's stage and the operator agree, or the difference between them is named in the
      plan output itself.

## The answer does not change

- [x] The ten rows the sort keeps are the same rows, in the same order, with the same values.
- [x] The aggregate's answer is the same number, and the published query's answer is unchanged.
- [x] A test asserts the sort's rows and its tie-break, so the answer cannot rot silently again.

## The fallbacks, named

- [x] A sort **with no limit** is measured and published separately: it sorts every row, and its
      cost is not the bounded sort's cost.
- [x] A sort with **NULLs** or with **strings** is measured separately: the general path reads a key
      as an enum, and the published query's path does not.
- [x] A plan with a **join and a sort** is out of scope, and named as such.
- [x] A **scattered selection** is measured against the blocks: the sort reads a scattered
      selection's keys a row at a time, and the two shapes are held to the same rows.

## The publication

- [x] `PERF.md` §3n: the sort's stage, part by part, and the before-and-after.
- [x] `README.md`: the status table, and the next question.
- [x] `bench-results/README.md`: the new reports, and the recipe that writes them.
- [x] `scripts/medians.py` derives the new tables; `scripts/check-published-figures.py` checks every
      published figure against the committed reports.
- [x] The comparison campaigns re-measured, for the code and for the code it replaced, back to back
      on the same host.

## The gate

- [x] `cargo fmt`, clippy on arm64 and x86-64, the whole test suite, `cargo deny`, the wasm32
      check, the published-figures guard and the page checks all green.
