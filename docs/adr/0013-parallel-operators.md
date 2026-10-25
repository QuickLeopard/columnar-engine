# ADR-0013: The filter and the projection are parallel; the aggregate is not, and never will be

**Status**: accepted
**Date**: 2026-10-25
**Related**: [ADR-0011](0011-parallel-decode.md) (the parallel read this extends), [ADR-0007](0007-comparator-pinning.md) (the thread pinning, as amended), [spec 007](../../specs/007-parallel-operators/spec.md)

## Context

ADR-0011 parallelised the engine's **read** and left the **operators** on one thread, and said so
in print: "the filter, the project and the aggregate still run on one thread". It also said what
would change that: *"the operators are parallelised, and the scan stops being the whole of the
work."*

That is what this increment does — but the *scale* of what it is worth is not the same in both
paths, and the documents say which is which rather than quoting the flattering one.

## The measurement that motivated it

The operators were 58–69% of the **resident** query — the data is already in memory, so this is the
pure query. These are the comparison harness's resident figures ([`PERF.md` §2a](../../PERF.md)):

| query | filter | project | evaluate | aggregate | total |
|---|---|---|---|---|---|
| `q6` | 6.6 ms | 0.0 ms | — | 0.0 ms | 9.6 ms |
| `q6-revenue` | 7.1 ms | 1.3 ms | 0.2 ms | 0.1 ms | 15.1 ms |

and 3.8% of the **cold** one, because the cold path's scan dominates it and the pushdown has
already applied most of the predicates.

(The totals here are the comparison harness's, which re-reads the file inside every iteration. The
resident sweep in [`PERF.md` §2f](../../PERF.md) measures the same stages with the file loaded
once, and its numbers are lower — 6.6 ms for `q6` — because the harness, not the engine, is the
difference. Both are published.)

So this increment is worth a lot in one path and a few percent in the other, and the documents have
to say which is which rather than quoting the flattering one.

## The harness defect that had to be found first

**The `--threads` flag never reached the operators.** The measurement harness passed
`ExecOptions::default()` on the resident path, whose `threads` field is `None`, and `None` means
one thread. The flag was wired to the *read* and to the lazy path, and nowhere else.

This was found by measuring, not by reading: the first attempt to parallelise the filter showed
**no scaling at all** — 13.5 ms at one thread and 13.8 ms at six — and the natural conclusion
("the kernel does not scale") was wrong. A probe that called the filter directly, with the thread
count as an argument, showed 6.9 ms at one thread falling to 2.0 ms at six: **3.5×**, from the
same code, on the same data, in the same process.

The lesson is worth recording, because it is the same shape as a measurement bug in ADR-0006: **a
number that does not move is a claim about the measurement until proven otherwise.** The fix is a
probe that exercises the thing directly, with the parameter under test as an explicit argument.

## The decisions

### 1. The filter is split into contiguous, word-aligned row blocks

One block per worker. The blocks are contiguous and their starts are multiples of 64 rows, so each
block's match mask is a whole number of words and the masks **concatenate without a shift**.

The 64-row alignment is not an arbitrary constant. A match mask is a bit per row; a mask that is a
whole number of words can be concatenated with another one by copying words, and a mask that is
not would need a shift — and a bug in that shift would silently drop or duplicate rows. That is
the same class of defect the `NullBitmap::concat` bug in ADR-0011 was, and the same reason it is
worth the constraint.

### 2. The masks are joined in block order, not completion order

The handles are joined in the order they were spawned, which is block order. There is no sorting
and no shared mutable state.

This is ADR-0011's rule applied one layer up, and it has the same reason: the selection is the set
of rows the rest of the query sees, and a selection that depended on the scheduler would make the
answer depend on the scheduler.

### 3. The projection is split over the **source** rows, not the survivors

The gather splits the source rows into blocks and gathers the survivors in each block. The
alternative — splitting the *selection* — would give each worker a share of the survivors, which
sounds more even but is not: the survivors are concentrated wherever the predicate says they are,
and a worker holding no survivors would still have to be paid for.

The split is over the source rows, so the work is even by construction: every worker scans the
same number of source rows and gathers whatever survived in them. The gather's blocks are at least
4,096 rows, which is a coarser granularity than the filter's 64 — a gather is a copy, and a copy
does not need the same alignment discipline a mask does.

### 4. The aggregate is not parallelised, and will not be

**`SUM` is not associative.** Floating-point addition is not associative, so a parallel aggregate
that folded each block separately and combined the partials would return an answer that depends on
the block boundaries — and the block boundaries depend on the thread count. The same query would
return different bits at one thread and at six.

ADR-0011 made the *read* order-preserving for exactly this reason, and this ADR does not trade
that property away for 0.1 ms. The aggregate is 0.1 ms of a 15 ms resident query; the answer
being reproducible is worth more than the 0.1 ms.

The same argument covers the **expression evaluation**, and it is stronger there: the evaluation
happens over the rows that survived the filter, which is 110,915 of 6,000,000, and it costs
0.2 ms. There is nothing to win.

### 5. One thread is the serial path, not a parallel path with one worker

`filter_all_with_workers(.., 1)` runs the original serial code. There is no second
implementation to keep in agreement, and the one-thread figure is bit-for-bit the figure the
previous increments published.

### 6. The plan reports the workers that ran

The plan output now carries `operator workers: N filter, M projection`. The two are **separate
numbers** because they are separate: the filter's blocks are 64-row aligned and the projection's
are at least 4,096 rows, so on a 20,000-row dataset with 64 threads the filter runs 63 workers and
the projection runs 5.

This is the same honesty as the read's worker count: the workers that ran, not the threads that
were asked for.

## The measurement

Two sweeps, because the two paths answer different questions.

- The **cold** sweep: the file is read and the query answered in one go. The scan dominates, so
  the operators' parallelism moves the number by a few percent.
- The **resident** sweep: the query is answered against data that is already in memory. The
  operators *are* the measurement.

The incumbent comparison is symmetric in both: `DuckDB` and `Polars` run at the same thread count
as the engine at every point, in the same mode, on the same file.

## The tests, and what they have teeth

The property is **bit-identical**, not "equivalent": the same rows, in the same order, with the
same values and the same validity bits. `crates/columnar-engine/tests/parallel_operators.rs`
compares the parallel result against the serial one at every thread count, on data that exercises
every encoding the filter has a path for — plain integers, plain floats, dictionary-encoded
integers, dictionary-encoded strings, and NULLs in every column.

The tests were **mutation-verified**, because a test that cannot fail is not a test:

| mutation | caught by |
|---|---|
| the filter's blocks concatenated in reverse order | 5 tests |
| the gather's parts concatenated in reverse order | 3 tests |
| the filter's blocks one row too long, so they are not word-aligned | 4 tests |

The first two are the merge-order bugs this ADR exists to prevent, and they are caught by the
row-level comparisons rather than by the aggregate-level ones — a sum does not care which row came
first, so only a test that looks at the rows can tell.

## Consequences

**Good.**

- The resident query now uses the cores it was measured on.
- The answer does not depend on the thread count, and that is checked rather than asserted.
- The aggregate stays serial, so the engine's answer stays reproducible.

**Bad, and accepted.**

- **The thread count now reaches the operators, so the one-thread comparison is the only
  single-threaded claim left.** It is unchanged, and it is still the headline.
- **The resident sweep is a third table.** Two tables were already published; the resident sweep
  is a third, and it is the one that shows what this increment did.
- **The filter's block size is a constraint.** 64 rows is small enough that the masks concatenate
  cleanly and large enough that the per-block overhead does not show. A smaller block would be
  more parallel and more overhead; a larger one would be the opposite.

**Neutral.**

- The **cold** path gains a few percent. The operators are 3.8% of it, and the scan is what the
  cold path is.
- The **eager path's decode** is still serial. It is 1,277.8 ms — the largest number in the
  repository, and 144× the cost of the operators this increment parallelised. It is the next
  increment, and a bigger one: the engine's load is currently *behind* DuckDB's (1277.8 ms against
  1218.0 ms), and six workers would put it roughly five times ahead.

## Alternatives considered

**Parallelise the aggregate as well.** Rejected, permanently. `SUM` is not associative; a parallel
aggregate's answer would depend on the block boundaries. The engine's answer being reproducible is
not a performance feature that can be traded away.

**Split the selection rather than the source rows for the projection.** Rejected. The survivors are
not spread evenly over the source rows, so the workers would get unequal work, and a worker with
no survivors would still have to be scheduled. Splitting the source rows makes the work even by
construction.

**Use a task pool with work-stealing for the operators.** Rejected for now. It would balance the
load better when the blocks are unevenly sized, but it hands the merge a set of parts in
*completion* order, which is the thing this ADR exists to prevent. The balanced version is to sort
the parts by their block index before merging — cheap, because the parts know their own position —
and that can come later, when a measurement says the imbalance matters.

**Parallelise the expression evaluation.** Rejected. It is 0.2 ms of a 15 ms query, and
parallelising it would mean slicing the columns into blocks — a copy per column — to save a
fraction of a millisecond.

**Overlap the operators with the read.** Rejected for now. The read's workers and the operators'
workers are the same budget, used in sequence. A pipeline that overlaps them would need the
operators to consume the read's output as it arrives, which is the morsel driver, and that is a
different increment.

## Revisit when

- The eager path's decode is parallelised, and the load column stops being the engine's worst
  figure.
- A measurement shows the operators' block sizes are wrong — too small, so the per-block overhead
  shows, or too large, so the load is uneven.
- The morsel driver arrives, and the read and the operators overlap.
