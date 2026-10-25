# Spec 007 — The parallel filter and projection

**Status**: accepted
**Date**: 2026-10-25
**Depends on**: [spec 005](005-parallel-decode/spec.md) (the parallel read), [spec 006](006-arithmetic-aggregate/spec.md)
(the arithmetic aggregate)

---

## 1. The problem, measured

The read is parallel. The operators behind it are not, and the plan output says how much that
costs.

**The resident path** — the data is already in memory, so this is the pure query:

| query | filter | project | evaluate | aggregate | total |
|---|---|---|---|---|---|
| `q6` | **6.6 ms** | 0.0 ms | — | 0.0 ms | 9.6 ms |
| `q6-revenue` | **7.1 ms** | 1.3 ms | 0.2 ms | 0.1 ms | 15.1 ms |

**The operators are 69% of the resident `q6` and 58% of the resident `q6-revenue`.** They are the
query.

**The cold path** — the file is read as part of the measurement:

| query | scan | filter | project | evaluate | aggregate | total |
|---|---|---|---|---|---|---|
| `q6-revenue`, six workers | 31.3 ms | 0.2 ms | 0.7 ms | 0.2 ms | 0.1 ms | 32.5 ms |

**The operators are 3.8% of the cold `q6-revenue`.** The reader did the work: it pushed two of
the five predicates down and returned 407,082 rows, and the engine's filter only had to apply
the predicate on `l_discount`, which the expression also reads and which therefore could not be
pushed down.

So this increment is worth different amounts in the two paths, and the spec says which:

- **The resident path scales.** This is the query-engine claim — the engine answering a query
  with the data in memory — and it currently cannot use a second core.
- **The cold path gains a few percent**, and the cold gap against `DuckDB` is **not** here. That
  gap is the scan's throughput, and §5 says so.

## 2. What this increment does

It splits the **filter** and the **projection** across the workers, and leaves the other two
operators alone.

**Why the filter and the projection, and not the others:**

| operator | resident cost | parallel? | why |
|---|---|---|---|
| filter | 6.6–7.1 ms | **yes** | it is the query |
| projection | 0.0–1.3 ms | **yes** | it is a copy, and copies parallelise |
| evaluation | 0.2 ms | no | 1.3% of the query, and parallelising it would need the columns sliced |
| aggregate | 0.1 ms | **no, and never** | a floating-point sum is not associative — see below |

**The aggregate stays serial, and it stays serial forever.** `SUM` is not associative: summing
the rows in a different order gives a different answer in the last bits. A parallel aggregate
would split the rows into blocks, fold each block, and combine the partials — and the combined
answer would depend on the block boundaries. Spec 005 made the *read* order-preserving so that
the engine's answer could not depend on the scheduler, and that property is not going to be
traded away for 0.1 ms.

## 3. Requirements

**R1 — The filter is split into contiguous, word-aligned row blocks.**
One block per worker. The blocks are **contiguous** and their starts are multiples of 64 rows,
so each block's match mask is word-aligned and the masks concatenate without a shift.

**R2 — The masks are concatenated in block order.**
So the combined mask describes exactly the rows the serial filter's mask describes. The result
is **bit-identical**, not merely equivalent.

**R3 — The projection is split over the source rows, not over the survivors.**
The source rows are split into contiguous blocks, each block's survivors are gathered independently,
and the parts are concatenated in block order.

The split is over the **source** rows because the survivors are not spread evenly over them: a
predicate concentrates the survivors where it matches, so a split over the survivors would give the
workers unequal work and a worker holding no survivors would still have to be scheduled. Splitting
the source rows makes the work even by construction — every worker scans the same number of source
rows and gathers whatever survived in them.

The projection's blocks are at least 4,096 rows, which is coarser than the filter's 64. A gather is
a copy rather than a mask, and a copy does not need the same alignment discipline — the parts are
concatenated with a validity-bitmap shift, which is exactly what `NullBitmap::concat` does and what
spec 005 fixed.

**R4 — The answers are identical to the serial ones.**
On every built-in query, on the real file, at every thread count. The same property spec 005
established for the read, established for the operators.

**R5 — The plan output reports the workers the operators used.**
The same honesty as the read's: the workers that ran, not the threads that were asked for.

**R6 — One thread means exactly what it means today.**
The serial path is the one-block path, not a parallel path with one worker. There is no
second implementation to keep in agreement.

## 4. Out of scope, stated rather than deferred

- **The aggregate is not parallelised, and will not be.** See above.
- **The expression evaluation is not parallelised.** It is 0.2 ms of a 15 ms resident query, and
  parallelising it would mean slicing the columns into blocks — a copy per column — to save a
  fraction of a millisecond.
- **The eager path's decode is not parallelised.** It is **1,277.8 ms** — the largest number in
  the repository, and 144 times the cost of the operators this increment parallelises. It is the
  next increment, and it is a bigger one: the engine's load is currently *behind* `DuckDB`'s
  (1277.8 ms against 1218.0 ms), and six workers would put it roughly five times ahead.
- **The operators do not run while the read runs.** The read's workers and the operators'
  workers are the same budget, used in sequence. A pipeline that overlaps them would need the
  operators to consume the read's output as it arrives, which is the morsel driver, and that is
  a different increment.

## 5. What would make this wrong

- **The blocks are merged in completion order.** Then the selection would depend on the
  scheduler, and the same query would return different answers on different runs.
- **The aggregate is parallelised.** Then a floating-point sum would depend on the block
  boundaries, and the engine's answer would stop being reproducible.
- **The filter's blocks are not word-aligned.** Then the masks would need a shift to
  concatenate, and a bug in the shift would silently drop or duplicate rows — the same class of
  bug as the `NullBitmap::concat` defect spec 005 found.
