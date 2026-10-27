# ADR-0030 — The scan that feeds the join

**Status**: accepted
**Date**: 2026-10-27
**Spec**: [spec 023](../../specs/023-the-scan-that-feeds-the-join/spec.md)
**Predecessor**: [ADR-0029](0029-the-dictionary-counted.md)

## Context

The engine's cold `join-orders` is **100.4 ms** against DuckDB's **93.0 ms**. The plan output
says where the time is:

```
scan       6000000  6000000   71.4 ms
project     216783   216783    0.8 ms
join       6000000   216783   21.9 ms
aggregate   216783        1    0.2 ms
```

Spec 021 took the join's probe and the sort apart and closed both. **The scan was never taken
apart.** The instrument that does it — `crates/columnar-engine/examples/scan_cost_probe.rs` —
measures the scan in the shapes it is made of, and the answer is one number:

| | |
|---|---|
| the crate's read of `l_orderkey` and `l_extendedprice` | 66.54 ms |
| the engine's own scan of the same two columns | 73.03 ms |
| **the engine's copy** | **6.49 ms** |

That 6.49 ms is 96 MB of values, copied out of the reader's Arrow array and into the engine's own
column chunks. It is **8.9% of the scan**, and it is the whole of the engine's share of the 71.4 ms.

And the join throws 96.4% of it away: the build side holds 216,345 keys, and the probe side's
6,001,215 rows match 216,783 of them. The engine copies every row the reader hands it, and decides
a moment later that most of them are worth nothing.

## The decision

**The scan feeds the join.** The join's probe runs **inside** the scan, over the reader's batches,
before anything is copied. Only the rows the join keeps are copied into the dataset.

### 1. One probe, two key sources

The probe must answer the same whether it reads a key out of the dataset's own encoding or out of
the reader's Arrow array. So the lookup is written **once**, against an interface:

```rust
pub trait KeySource {
    fn len(&self) -> usize;
    fn has_nulls(&self) -> bool;
    fn integer_values(&self) -> Option<&[i64]>;
    fn value_at(&self, row: usize) -> Option<KeyValue<'_>>;
}
```

The engine's `KeyColumn` implements it, and so does a new reader-side key column. The NULL rule and
the NaN rule live in the shared lookup, so the two paths cannot drift apart: a NULL key never
matches, in either, and a NaN key never matches, in either.

This is the same lesson as [ADR-0029](0029-the-dictionary-counted.md): the dictionary's map and
comparison were **one algorithm written twice**, once in the pushdown and once in the filter. The
cost of that was not only the copy-paste — it was that the two copies could disagree, and that
fixing one left the other slow.

### 2. The dataset holds what the join kept

The streamed scan's dataset holds the probe side's rows that matched, one row per matching probe
row. The join's output carries the multiplicity, exactly as it does today: a probe row that matches
two build rows is two rows of the result, and the row numbers the join and the sort carry are the
dataset's own row numbers.

The copy falls from 96 MB to 3.5 MB.

### 3. The build table is built before the scan

The probe has to happen before the copy, and the probe needs the build side's table. So when the
scan feeds the join, the table is built **before** the scan rather than after it.

The join's stage then reports the build table's construction, and the probe's cost is the scan's —
because the probe happens inside the scan. The stage table still adds up: the query's time is the
same time, split differently.

### 4. The fallbacks are named

Two plans do not take the streaming path:

- **A probe side with a predicate of its own.** The predicate would have to be evaluated over every
  row of the file rather than over the rows that survived the scan. That trade is not measured, so
  it is not taken. The streaming path runs exactly when every predicate was pushed into the reader,
  which is exactly when the filter stage has no work to do.
- **A join key whose Arrow type the probe cannot read.** The probe reads integers, doubles and
  strings. Anything else is declined.

Both fallbacks are in the spec's scope, not in a footnote, and the plan output reports the rows the
scan kept — so the numbers say which path ran.

## Consequences

**The engine's copy of the join's probe side goes from 96 MB to 3.5 MB**, and the scan stage's
elapsed falls by the difference between the copy and the probe's own cost.

**The plan output's stage table changes shape for a plan with a join.** The scan's rows out is the
rows the scan kept — the probe side's matching rows — and the join's rows in is what the scan kept,
not the file's row count. The row counts the plan output publishes for `join-orders` move, and they
move for a reason: the scan now hands the join 216,783 rows rather than 6,000,000, because the
other 5,784,432 were never copied.

**The join's key column is no longer materialised.** The probe reads it where it lies — in the
reader's array — so the dataset does not hold a copy of it at all. That is the same rule the
aggregate's plain column already follows
([ADR-0016](0016-the-aggregate-reads-in-place.md)): a column nobody reads after the scan is not
copied by the scan.

**The scan and the join are now one stage's work, and the stage table says so.** A reader who wants
to know what the join cost has to read two rows of the plan output now. That is the price of
telling the truth about where the work is, and it is the same trade [ADR-0028](0028-the-hot-paths-taken-apart.md)
made when it split the join's stage into its build table and its probe.

**The build table is built earlier.** A plan that is refused for its memory estimate is refused
before the table is built, as before; a plan that is refused for another reason now builds the
table first. The estimate still covers the table.

## What this does not do

- **The build side is not streamed.** It is 1.5 million rows, read once, and the hash table of its
  keys is what makes the probe O(1).
- **The probe side's rows are still materialised.** The dataset holds one row per matching probe
  row; it is not a stream of them.
- **The aggregate's gather over the joined rows is unchanged.** It gathers 216,783 rows, not
  6,000,000, and it is not what this ADR is about.
- **The filter stage is not fused into the scan.** The scan still applies the pushed-down
  predicates' mask; it does not take over the filter's work.

## References

| principle | how this ADR serves it |
|---|---|
| I. One Predicate, Six Implementations, One Answer | The probe's lookup is written once, over one key interface, so the streamed and materialising paths cannot answer differently. |
| III. NULL Is an Absent Bit, and It Never Matches | The key source carries the reader's validity, and the shared lookup keeps the NULL and NaN rules in one place. |
| IV. Evidence Over Claims | The 6.49 ms is measured by a committed probe, and the change's effect is measured by the published campaigns. |
| VI. The Dataset Is Named Honestly | The dataset holds what the join kept, and the plan output says so. |
| VII. Explicit Scope, No Silent Deferral | The two fallbacks are named in the decision and in the spec's scope. |
