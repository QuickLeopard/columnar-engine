# ADR-0031 — The time the plan output does not name

**Status**: accepted
**Date**: 2026-10-28
**Spec**: [spec 024](../../specs/024-the-time-the-plan-output-does-not-name/spec.md)
**Predecessor**: [ADR-0030](0030-the-scan-that-feeds-the-join.md)

## Context

The plan output is the engine's own account of its work: a stage table, the rows each stage was
handed and produced, and the time each stage took. On **six of the eight** published queries that
account was complete — the stages added up to the query's time to within a fifth of a millisecond.

The two that name a **sort** and a **join** did not. The instrument's own numbers, the old code,
measured back to back on this host:

| | the query | the stages | what the stages did not name |
|---|---|---|---|
| `join-orders`, resident | 27.03 ms | 22.36 ms | **4.684 ms** |
| `sort-topk`, resident | 8.95 ms | 4.26 ms | **4.324 ms** |

The plan output did not say what the missing time was. It was not a rounding error, and it was not
the machine: it was **17%** of the join's query and **half** of the sort's. On the cold path the
join's hole was **0.257 ms** — the streamed join reads the selection as it goes — while the sort's
was **4.299 ms**: the hole was where the code asked.

The instrument that answers it — `crates/columnar-engine/examples/query_cost_probe.rs` — runs the
engine's **own** plans through the engine's own entry point, checks each answer against the
published one before anything is timed, and measures the query in the shapes the plan output does not
name. The answer is one number:

| | the old code |
|---|---|
| the selection expanded into row numbers | **4.13 ms** |
| the whole query | 27.03 ms |
| the plan output's own stage table | 22.36 ms |
| **the difference** | **4.684 ms** |

**The expansion was the whole of it.** The join and the sort asked the filter's selection for its
row numbers — `selection.rows()` — **before either stage's clock started**. A plan with no filter
has the selection "every row", so that is **6,000,000 row numbers**, **24 MB**, built outside every
stage and handed over as a `Vec<u32>`. The six queries that name neither a join nor a sort never
ask, and their accounts were complete — which is why the defect hid in two queries out of eight.

## The decision

**The plan output accounts for the whole query, and the selection that holds every row is the
range.**

### 1. The selection that holds every row is the range

`SelectionVector` had two representations: an index list of the surviving rows, and a bitmap of one
bit per row. A selection that holds **every** row was a bitmap with every bit set — 750 KB for six
million rows, and a bitmap to walk.

It is now a third representation, `Repr::All`, and it is **the range**: `0..total`, with no bits
and no list. A plan with no predicate hands the join and the sort a selection whose rows cost
nothing to read, because there is nothing to read.

This also fixes a latent defect: the core's `SelectionVector::all` left the padding bits of its
last word set, so walking it visited rows past the end of the batch. The engine had a second
constructor (`operators::all_rows`) that avoided it, and the filter's own no-predicate path did not.
There is one constructor now, and it is right.

### 2. The stages are handed the rows, not a vector of them

The join's probe and the sort are handed the filter's selection, and they read
the rows as they need them: the sort's bounded heap consumes them and keeps
ten rows; the join's probe consumes them and keeps the rows that match.
**Nothing is materialised**, and the 24 MB vector is gone rather than renamed.

**And the range is handed over as a range.** The first version of this handed
the selection's *iterator* over, and the iterator decides which representation
it is walking **on every row**. That is not free: the decision is a branch and
a state update that cannot be hoisted out of the loop, and over six million
rows it cost **2.6 ms** — the sort's stage grew by **4.5 ms**, and the query
that the change was meant to make faster got **slower**.

So the call site asks the selection what it holds. A selection that holds
**every** row is the range, and the range is a concrete iterator: the probe's
own loop keeps its state where it can be kept, and there is no branch per row.
A selection that holds *some* of its rows is handed its own iterator, and
pays the decision per row — which is still cheaper than the list of row
numbers the old code built.

The rows the join and the sort are handed are the same rows, in the same
order, with the same values: the sort's order is total — equal keys are broken
by the row's own number — so the rows it keeps do not depend on the order
they arrived in.

### 3. The plan output says what it accounted for

The engine now measures the **whole** query, and the plan output carries the account:

```
stage      rows in  rows out  elapsed
scan       6000000   216783    71.0 ms
project     216783   216783     0.5 ms
join        216783   216783    25.1 ms
aggregate   216783        1     0.2 ms
accounted: 96.81 ms of the query's 96.82 ms (0.01 ms outside the stages)
```

The last line is the point. A stage table that adds up to less than the query is an account with a
hole in it, and the hole is where the engine's work goes to hide. The hole is now **0.01 ms**, and
it is **named**.

### 4. The build side's file is the join's work

The one piece of real work that was outside every stage was the join's build side's file being
opened. It is now reported in the join's stage, because it is the join's own work.

What is left outside the stages is the plan's resolution (**0.0003 ms**) and the memory estimate
(**0.00004 ms**) — the engine's own bookkeeping, at the clock's resolution.

## Consequences

**The account is complete, and the test holds it there.** `crates/columnar-engine/tests/attribution.rs`
runs the query several times, takes the **smallest** gap — the gap can only grow, because a run the
machine interrupted has a gap in it — and fails if the stages do not account for the query.

**The query got faster by exactly what left the account.** The resident `sort-topk` fell from
**8.95 ms** to **3.90 ms** on the same host, back to back, and the unnamed time fell from
**4.324 ms** to **0.004 ms**. The resident `join-orders` fell from **27.03 ms** to **22.61 ms**, and
its unnamed time fell from **4.684 ms** to **0.016 ms**. The two numbers are the same number, seen
twice.

**The selection has a third representation**, and the core's doc says why. The density dispatch
still decides between the index list and the bitmap; the range is not a density decision, it is the
truth about a plan with no predicate.

**The plan output carries a new line.** Every plan output now ends its stage table with the
account. The figures in the committed comparison reports carry it too, so the gate can read it from
the reports rather than from the prose.

## What this does not do

- **The sort's operator is not touched.** Spec 021 measured it: **3.7 ms** against a **3.6 ms**
  plain-read floor. This ADR is about the time the plan output did not name.
- **A dense selection is still walked.** A plan with a predicate that keeps most of its rows has a
  bitmap, and the bitmap is walked a set bit at a time — through the iterator, so the decision
  costs a branch a row. The walk is now **inside** the stage that does it, and it is measured: the
  bitmap's walk is published beside the range's, **2.83 ms** against **0.56 ms**.
- **The join's build table and the join's probe are not touched.** They are the join stage's own
  work, and they were already accounted for.
- **The plan output does not name every operator.** It names the stages, and now it says what the
  stages did not cover. That is an account, not a trace.

## References

| principle | how this ADR serves it |
|---|---|
| I. One Predicate, Six Implementations, One Answer | The rows the join and the sort are handed are the same rows, in the same order; the sort's order is total, so it does not depend on how they arrived. |
| II. The Answer Is the Same | No answer changes. The probe checks each answer against the published one before it times anything, and the differential suites hold the row counts. |
| IV. Evidence Over Claims | The unnamed time is measured by a committed instrument, the account is held by a test, and every published figure is derived from the committed reports. |
| VI. The Dataset Is Named Honestly | The plan output is the engine's own account of its work, so it accounts for the whole query — the rows it names and the time it spends. |
| VII. Explicit Scope, No Silent Deferral | The exclusions are named in the spec's scope, each with its reason. |
