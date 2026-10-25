# ADR-0011: The read is parallel over row groups, and the merge preserves the file's order

**Status**: accepted
**Date**: 2026-10-25
**Related**: [ADR-0007](0007-comparator-pinning.md) (the thread pinning this amends), [ADR-0008](0008-eager-parquet-decode.md) (the decode this parallelises), [spec 005](../../specs/005-parallel-decode/spec.md)

## Context

ADR-0007 pinned both incumbents to **one thread**, because the engine was single-threaded and a
comparison of a one-thread engine against a fourteen-thread one is not a comparison of the same
work. It said so explicitly, and it cost the repository something real: `PERF.md` §2c attributed
the whole remaining gap against DuckDB — 2.8× on `q6` cold — to *threads*, and named the missing
work: "parallel decode the engine has not implemented".

So the engine now reads in parallel. That decision is not hard. The one that follows from it is.

## The decision that is not hard

**Split the file's row groups across the threads.** A Parquet file is a sequence of independent row
groups; each is a self-contained column-chunk collection with its own dictionary, its own statistics
and its own pages. Nothing in one row group refers to another. So a row group is the unit of
parallel work, and the reader can decode them independently.

The unit of work is **not** the batch. Arrow's reader hands out batches of 8,192 rows, which is
*smaller* than a row group, and splitting a row group across threads means splitting its dictionary
too. Row groups are the unit the file already has.

## The decision that is hard: the merge order

`SUM` is not associative. Floating-point addition is not associative, and neither is the
`i64` sum once it is allowed to wrap. So a dataset whose rows were ordered by **completion** —
fastest thread first — rather than by **position** would make the engine's answer depend on the
scheduler. The same query would return different bits on different runs, and a user could not
tell a correct answer from an incorrect one. A query engine that returns a different answer
depending on how busy the machine is is not a query engine.

**So the parallel read preserves the file's row order, exactly.**

Three decisions follow from that, and each is cheaper than the alternative:

1. **The row groups are split into contiguous blocks**, one per thread. Not round-robin, not
   work-stealing: contiguous, so that block *k* holds exactly the row groups *k* would hold in a
   serial read.
2. **The parts are joined in block order**, not completion order. The handles are joined in the
   order they were spawned, which is block order. This is the whole mechanism: there is no
   sorting, no comparison, and no shared mutable state.
3. **The merge is a concatenation.** Each column's chunks are concatenated left to right, and the
   validity bitmaps are concatenated with a mid-word shift.

The consequence is that the parallel read produces a dataset **bit-identical** to the
single-threaded one. Not "the same answer" — the same *rows, in the same order, with the same
validity bits and the same dictionary contents*. That is checkable, so it is checked: the
differential test compares the two datasets row by row, with the dictionary resolved, and the
end-to-end test does the same over the real generated file for every built-in query.

### The test that has teeth

The aggregate-level tests do **not** catch a mis-ordered merge, because a sum does not care which
row came first. This was verified rather than assumed: reversing the merge order in the source
leaves `the_parallel_path_agrees_with_the_single_threaded_one` **passing** — the sum is the same
number either way — and fails only `the_parallel_read_holds_the_same_rows_in_the_same_order`, which
compares the rows themselves.

That test is the reason the row-order property is a property of the code and not a hope.

### The bug this found

The merge's validity concatenation was **wrong** on its first version, and the unit test found it
before the code was committed. `NullBitmap::concat` copied only the *complete* words of the first
mask and dropped its last, partial word — which is exactly the word the second mask's bits land
in. Every NULL in the first part of a merged chunk was silently lost.

A NULL lost in a validity bitmap is not a crash and not a wrong count: it is a row that the
engine believes has a value when it does not. The `columnar-core` unit test now pins the
concatenation against a single mask over the same rows, at every combination of word alignment.

## What is *not* parallel

**The operators.** The filter, the project and the aggregate still run on one thread. Only the
**read** is parallel.

This is stated rather than hidden, because it decides what the scaling curve means. The engine's
6-thread `q6` cold figure is 16.2 ms against a 1-thread 82.8 ms — 5.1× — and DuckDB's is 10.0 ms
against 32.0 ms, 3.2×. The engine's curve is *steeper* because the parallel part is a larger
fraction of its work: the engine's query is almost entirely scan, while DuckDB's is a scan plus a
parallel filter over 315,203 surviving rows.

## Consequences

**Good.**

- The gap against DuckDB on `q6` cold narrows from 2.8× at one thread to **1.6×** at six. The
  architecture term ADR-0010 and spec 003 shrank; this shrinks the other half of it.
- The parallel read is bit-identical to the single-threaded one, and that is checked rather than
  asserted.
- No shared mutable state, no lock, no task pool. The threads are scoped, the parts are joined in
  order, and the merge is a concatenation.

**Bad, and accepted.**

- **The parallelism is capped by the file.** The read splits *row groups*, so a file with six row
  groups has six workers however many threads are asked for. The published sweep shows the curve
  flat from six threads on, and the report says why: the cap is the file's, not the machine's.
  A file written with larger row groups would go further.
- **The parallel read holds two copies of the read set at its peak** — every thread's part at
  once, which is one copy between them, and then the merged result, which is another. The
  single-threaded read decodes straight into one dataset and holds one. The memory budget
  therefore **doubles** for a parallel read, and the estimate says so: `estimate_lazy_read` counts
  two copies when the read has more than one worker. This is the price of the parallel path, and a
  budget that did not account for it would let through a query the machine cannot hold.
- **The estimate is now a pre-flight one.** The lazy path's working set is estimated from the
  file's schema and row count, *before* the read, and the budget is checked before a single row is
  read. A plan that does not fit is refused having done no work at all. This is a change in
  behaviour, and it is the right one: the old estimate was made over the dataset, which did not
  exist until after the read.

**Neutral.**

- The eager path is still single-threaded. It is the path the benchmark's *resident* numbers use,
  and the resident comparison is a comparison of query engines, not of read throughput.
- `DuckDB` is not capped by row groups and is not affected by the file's layout, so at fourteen
  threads it uses fourteen. The sweep runs both systems at the same thread count at every point,
  which is the only way the two curves can be compared at all.

## Amendment to ADR-0007

**Amended 2026-10-25 by spec 005.** ADR-0007 pinned both incumbents to one thread because the
engine was single-threaded, and it was right to. That reason has expired.

**The headline comparison stays pinned to one thread.** It is the like-for-like *single-threaded*
comparison, and it is unchanged — `just compare` still runs everything at one thread. But it is no
longer the only table, because a one-thread table on its own is a misleading picture of what a user
sees: `DuckDB` uses every core by default, so a user running `DuckDB` gets 10.0 ms on `q6` cold,
not 32.0 ms.

**A second table is published: the thread-count sweep.** The engine and both incumbents, at the
same thread counts, on the same file, in the same harness. The principle that has not changed is
symmetry — the incumbents run at the engine's thread count at every point of the sweep, exactly as
they run at the engine's one thread in the headline table. What has changed is that there is now
more than one thread count to be symmetric about.

The two tables answer different questions, and the documents say which is which:

| table | question |
|---|---|
| the headline | *which is faster when both are given one thread* |
| the sweep | *how does each system's number move with the thread count* |

## Alternatives considered

**Merge the parts in completion order — fastest thread first.** Rejected. It would be marginally
faster and it would make the answer depend on the scheduler. `SUM` is not associative, so the
same query would return different bits on different runs.

**Split the batches rather than the row groups.** Rejected. Batches are smaller than row groups,
so splitting them means splitting a row group's dictionary across threads and merging the pieces
-- more work, more contention, and no benefit, since a row group is already an independent unit.

**Use a task pool with work-stealing.** Rejected for now. It would balance the load better when
the row groups are unevenly sized, but it hands the merge a set of parts in *completion* order,
which is the thing this ADR exists to prevent. The balanced version is to sort the parts by their
row group's index before merging -- which is cheap, because the parts know their own position --
and that can come later, when a measurement says the load imbalance matters.

**Parallelise the operators too.** Not yet. The operators take a `&Dataset` and return owned
values, so the data path is already shaped for a morsel loop, but there is no morsel driver and
no per-thread scheduling. Doing it before the operators are measured against a parallel baseline
would be a guess rather than an increment.

**Publish only the sweep, and drop the one-thread table.** Rejected. The one-thread table is the
like-for-like single-threaded comparison, and it is the one a reader who wants to compare *this
engine* with *DuckDB* as single-threaded systems wants. Dropping it would lose a claim that is
true to publish a curve that is more flattering.

## Revisit when

- The operators are parallelised, and the scan stops being the whole of the work.
- The dataset generator writes larger row groups, and the file's cap stops being the binding one.
- The eager path is parallelised, and the *resident* comparison starts being a comparison of read
  throughput rather than of query engines.
