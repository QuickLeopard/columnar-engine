# ADR-0058: The join-orders query, on both sides, taken apart

**Status**: accepted
**Date**: 2026-11-06
**Reviewers**: TBD

## Context

The engine's cold `join-orders` query is behind `DuckDB`'s — **100.0 ms** against **94.0 ms** — and
[STATUS.md](../../STATUS.md)'s last section has carried the question since spec 026:

> **What would answer it:** the parts, not the total.

**And the file has been right about what it wanted.** Spec 026 took the join's **stage** apart and found
the build side's read, and `Arrow`'s decode of `orders.parquet` — and that analysis was sound **about the
engine's own stage**. What it could not do was answer the question, because **a part cannot be compared
with a whole**. Every figure in that analysis was the engine's own account of itself, set beside
`DuckDB`'s **94.0 ms** — one part beside one whole, with nothing to line up.

**And there was a second thing nobody had asked.** A join's cost is a cost *per row*. The engine's
probe runs over **every** row the reader hands it; a join that only ever looked up the rows that can
match would be a different query's cost. **How many rows each side's join saw** was not measured
anywhere, and it is the kind of thing that decides a mechanism.

**And the parts have to be measured in one session.** The house has spent six increments learning what
the machine's own movement does to a difference: spec 033 measured the two sides of every difference in
the same iteration, back to back, with the order alternated, because a difference of two medians lets
the machine's drift land on both sides. **That method was not available here** — there was no instrument
that put both sides in the same iteration.

## Decision

1. **Both sides are taken apart, in one session, and each side's parts are that system's own account of
   itself.** The engine's parts come from **the engine's own plan output**: the stage table, and the join
   report's own parts — the build side's whole, the probe, the build side's open. `DuckDB`'s parts come
   from **`DuckDB`'s own profiler** (`PRAGMA enable_profiling='json'`), whose operator tree carries a
   timing per operator. **Neither side's parts are inferred from the other side's total.**

2. **The parts are named the same way on both sides, and each part says what it is.** The probe side's
   read, the build side's read, the build side's open, the join, the projection, the aggregate, the
   whole. **The parts that have no counterpart on the other side say so on their own row** — `DuckDB`'s
   file opens are inside its scan, and the engine's build side's open is its own row.

3. **The mechanism is measured, not assumed.** `DuckDB` is measured a **third** time with its **join
   filter pushdown disabled** — a change of exactly one thing, and therefore a counterfactual that names
   the mechanism instead of guessing at it. **The row count travels with it**, because the rows are the
   mechanism.

4. **The differences are the paired ones.** Both sides are measured back to back in the same iteration
   and the order is alternated between the iterations, so the machine's drift cannot land on one side.
   **The iteration count is even**, for the reason spec 026 gave: an odd count would leave the two
   orders' groups different in size, and the median would inherit the difference.

5. **The profiler is not free, so its price is published.** A fourth arm — `DuckDB` **unprofiled**, the
   way the comparison harness runs it — carries the whole the parts are not measured against, and the
   difference between the two wholes is the profiler's cost.

6. **The answer is checked before anything is timed.** Both sides' answers are checked against the
   **published** value of the cold `join-orders` query. A probe that measured a different query would be
   a measurement of nothing.

7. **No engine work changes.** The measurement decides what is built, and the decision belongs to the
   increment that follows. **The engine's public surface does not move**: the probe lives in
   `columnar-bench`, which is not published, and it adds no dependency — `DuckDB` is driven as the
   subprocess the comparison harness already drives.

## Consequences

**What became knowable.** The engine's join is **14.625 ms** against `DuckDB`'s **7.706 ms** — slower in
**every one of the eighteen iterations** — and `DuckDB`'s own join, over the same six million rows, costs
**+51.422 ms** more than its own filtered join and **+44.190 ms** more than the engine's. **The engine's
lookup is the faster of the two, four times over**, and the engine is behind because it looks up every
row where `DuckDB` tests a bit.

**And the gap was never one thing.** The engine's parts are **+10.668 ms** more than `DuckDB`'s;
`DuckDB`'s whole carries **6.860 ms** outside its operator tree — its planning, its blocked threads and
its I/O — that the engine's whole does not pay; and the net is the whole's **+3.290 ms**. **The
decomposition closes to 0.0000 ms on every iteration.**

**What the increment did not do.** It did not close the gap, and it does not pretend to: the published
comparison's figures stand — they are the comparison harness's, taken by a different campaign — and what
this increment publishes is the **parts**, which are the same on both sides whichever way the whole
falls. **The next increment is the filter**, and its prize is now a number rather than a hope.

**What we gave up.** The instrument measures **one query on one host**, and its arms are the engine's
plan output and `DuckDB`'s profiler — **two systems' own accounts of themselves**, which is the best
available and is not a third party's. The engine's **bytes read are not measured**: the engine's read is
named by the columns it reports, and the file's own column sizes are the file's, not a measurement of
the engine's read. **That is stated in the section that publishes the figures, and not glossed.**

## Confidence

**high** — the two sides are measured in one session, the counterfactual changes exactly one thing and
the profile shows that it does, the answers are checked before anything is timed, the decomposition
closes exactly, and the figures are derived from committed reports and checked by the guard inside the
section that publishes them.

**And the finding does not depend on the gap.** The claim is that the engine's lookup is the cheaper of
the two **row for row**, and that the engine looks up six million rows where `DuckDB` looks up 216,783.
Both are measured directly, both are far outside their own spreads, and **the finding would stand even if
the engine's whole were ahead.**
