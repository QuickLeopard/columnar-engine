# ADR-0032 — The sort, taken apart

**Status**: accepted
**Date**: 2026-10-28
**Spec**: [spec 025](../../specs/025-the-sort-taken-apart/spec.md)
**Predecessor**: [ADR-0031](0031-the-time-the-plan-output-does-not-name.md)

## Context

The engine's cold `sort-topk` is **40.1 ms** against DuckDB's **37.0 ms** and the peer's **37.7 ms** —
a gap of **3.1 ms**, the largest one left. In resident mode the engine's whole query is **5.7 ms**,
of which the sort's stage is **5.3 ms**, against DuckDB's **3.0 ms** for the same query.

**The sort had never been taken apart.** Spec 021 measured the sort's *operator* — **3.7 ms** against
a **3.6 ms** plain-read floor — and closed it: the floor was the read, so the sort was at the floor
and no algorithm would beat it. That measurement was **two increments ago**, on a different code
version, and the floor it measured was a **plain read**: a scalar loop that counts the values one at
a time.

The instrument that took the sort apart this time — `crates/columnar-engine/examples/sort_cost_probe.rs` —
measured the stage and its operator in the same session, and it found three things.

The figures it produced are the **medians of three campaigns**, run back to back on the same host,
with the old code in its own worktree; the raw reports are
`bench-results/sort-cost-run{1,2,3}.json` and `bench-results/sort-cost-before-run{1,2,3}.json`.

**The stage is the operator.** The stage (**4.18 ms** cold, **4.30 ms** resident) and the
operator (**3.23 ms**) are the same work. The key column's construction is a nanosecond and the
hand-over is a range. There is no engine overhead in the stage, and there is no cache state in it
either: the operator costs the same with the caches pushed out as with them warm, and the read
costs the same per value over four megabytes as over forty-eight (**0.50 ns** against **0.53 ns**) —
**the read was never waiting for memory**. It was counting.

**The operator is not at the floor.** The comparison, compared in blocks against the threshold,
costs **0.95 ms** for the same column — under a third of the operator's **3.23 ms**. The sort's
own work was **2.3 ms** of the operator's 3.2 ms, and it was the **comparison**, made a row at a
time.

**The scan that feeds the sort is worse.** The top-k over the reader's own Arrow arrays — where the
values are hot — costs **6.3 ms** on top of the decode, against the stage's **4.18 ms** for the
whole of it. Reading the keys where they lie is not the lever. **How they are compared** is.

## The decision

**The sort compares the keys in blocks.**

### 1. One door, and the row source says whether it can

The sort is handed a **source** of rows, not a list, and the source now says whether it holds a
**contiguous run**:

```rust
pub trait RowSource: IntoIterator<Item = u32> {
    fn run(&self) -> Option<std::ops::Range<u32>>;
}
```

A run's keys are a slice of the column, and a slice can be compared in blocks. A scattered
selection's keys are not, and it is read a row at a time, as it always was. The door is the same
door — `sort_rows` — so the instrument measures the engine's operator and the engine's stage
through the same call, on both code versions.

### 2. The block comparison, and the threshold that drives it

A block is compared against the **threshold** — the heap's own worst key, which is the row the heap
would drop next — and only a block that holds a row that beats the threshold is walked a row at a
time. The comparison is a reduction over a comparison, which is the shape a compiler vectorises; the
row at a time walk then touches a handful of rows out of six million.

The comparison is over the values' **total keys** — the same transform `total_cmp` uses, so a NaN is
above every number and a negative zero is below a positive one. The threshold is compared in the
sort's **direction**, so the comparison is always "at least as good" and the vectorised loop has no
branch in it.

### 3. The order does not change

The heap, the order and the tie-break are the ones the row at a time walk uses: the same bounded
heap of the best `limit` rows, the same comparison, the same tie-break by the row's own number, the
same NULL rule, and the same final order. A block that is walked is walked by the **same** code that
walks every row when the rows are not a run. The two keep the same rows in the same order, and a
test holds them to a reference that says so.

### 4. The fallbacks are named

- **A scattered selection** is read a row at a time. The blocks need a run, and a selection of six
  million row numbers is not one.
- **The general path** — strings, dictionaries, and any column with a NULL in it — is unchanged. It
  reads a key as an enum, and the published query's path does not.
- **The sort with no limit** is unchanged. It sorts every row, and its cost is not the bounded
  sort's cost: **535 ms** against the bounded sort's 1 ms.

## Consequences

**The sort's stage falls from 4.18 ms to 1.10 ms** cold, and **from 4.30 ms to 1.03 ms** resident,
and the engine's resident `sort-topk` — the whole query, over the engine's own entry point — falls
from **3.92 ms** to **1.05 ms**. The published campaign over the file, cold and lazy, falls from
**42.6 ms** to **37.2 ms**. In that campaign the engine's resident `sort-topk` is **2.7 ms** against
DuckDB's **3.0 ms** — **ahead of DuckDB on this query for the first time** — and the cold query
is **37.2 ms** against DuckDB's **39.0 ms**.

**The published floor moves, and the reason is recorded.** The floor was **3.6 ms** — a scalar read
of the column — and it is **0.95 ms** now: the same column, compared in blocks. The floor was never
a floor: it was the cost of counting the values one at a time, and the sort's own comparison was
counting them one at a time too. Every figure that quoted the old floor is re-derived and published
old-and-new.

**The sort's stage is now the whole of the resident query, and it is at the comparison's floor.**
The stage is **1.10 ms** against the block comparison's **0.96 ms**, so what is left in the
stage is the heap's own updates and the row at a time walk of the few rows that beat the
threshold.

**The plan output's stage table does not change shape.** The sort is still a stage, it still reports
its rows in and its rows out, and the tie-break it reports is the same.

## What this does not do

- **The scan's decode is not touched.** It is the scan's business, and it has been measured three
  times and refused every time.
- **The scan that feeds the sort is not built.** It was measured, and it is worse: the top-k over the
  reader's arrays costs **6.3 ms** against the stage's **4.18 ms**.
- **The general path is not touched.** A string column, a dictionary column, and a column with a
  NULL in it are read a row at a time, as they always were.
- **The sort with no limit is not touched.** It sorts every row, and the blocks are no help to it.
- **The join is not touched.** A plan with a join and a sort is not published.

## References

| principle | how this ADR serves it |
|---|---|
| I. One Predicate, Six Implementations, One Answer | The block comparison and the row at a time walk are the same sort: the same heap, the same order, the same tie-break, and a test that holds them to a reference. |
| III. NULL Is an Absent Bit, and It Never Matches | The blocks are only used by the path with no NULLs in it; the general path keeps the NULL rule, and the total order keeps the NaN rule. |
| IV. Evidence Over Claims | The **2.3 ms** of comparison, the **0.95 ms** floor, and the refused **6.3 ms** fusion are measured by a committed instrument, and the change's effect is measured by the published campaigns. |
| VI. The Dataset Is Named Honestly | The rows are handed over as a run or as a selection, and the sort says which it was given. |
| VII. Explicit Scope, No Silent Deferral | The three fallbacks are named in the decision and in the spec's scope, and each of them is measured. |
