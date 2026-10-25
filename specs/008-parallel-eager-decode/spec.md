# Spec 008 — The parallel eager decode

**Status**: accepted
**Date**: 2026-10-25
**Depends on**: [spec 005](005-parallel-decode/spec.md) (the parallel lazy read), [spec 007](007-parallel-operators/spec.md) (the parallel operators)
(the parallel operators)

---

## 1. The problem, measured

The eager path decodes **every** column of the file into the engine's own chunk format, and it did
it on one thread, with the Parquet reader's default batch size.

**The eager load was the largest number in the repository**, and the engine was *behind* both
incumbents at it:

| system | load (file → queryable) |
|---|---|
| **engine** (eager, `Dataset::read_parquet`) | **1277.8 ms** |
| duckdb (`CREATE TEMP TABLE ... AS SELECT *`) | 1218.0 ms |
| polars (`pl.read_parquet`) | 1015.5 ms |

The engine's load was 144× the cost of everything spec 007 parallelised, and it was the one place
where the engine was slower than the systems it is compared against.

**Where the time went.** The probe separated the two halves of the decode:

| | ms | share |
|---|---|---|
| Arrow's own decode, with no engine conversion | 376.1 | 31% |
| **the engine's Arrow→chunk conversion** | **823** | **69%** |

So the bottleneck was never Arrow. It was the engine's own conversion — and the conversion's
dominant cost is **per batch, per column**: each batch sets up its readers, allocates its arrays,
and rebuilds each dictionary column's table of merged dictionary slots.

**That last part is the defect.** The lazy path reads two columns and uses 8,192-row batches; the
eager path reads **sixteen** columns and used the reader's default of **1,024**-row batches. So the
eager path rebuilt sixteen dictionary tables 5,860 times instead of 733.

## 2. What this increment does

1. **The eager path uses its own batch size**, measured rather than inherited: 32,768 rows.
2. **The eager decode is parallel.** The file's row groups are split into contiguous blocks, one
   per worker, and the parts are merged afterwards **in block order**.
3. **The merge is rewritten**: one pass instead of a fold, moving the parts' values instead of
   copying them, and one allocation for the validity masks instead of one per part.

## 3. Requirements

**R1 — The eager path's batch size is measured, not inherited.**
The batch size is a tuning constant, and the eager path wants a different one from the lazy path:
the per-batch cost scales with the number of columns, and the eager path reads all sixteen. The
chosen value is in `EAGER_BATCH_ROWS`, and the table that chose it is in its documentation.

**R2 — The eager decode is parallel over the file's row groups.**
One block per worker, the parts joined **in block order**, exactly as the lazy read does it
(spec 005). The workers are capped by the file's row groups, because a row group is the unit the
file already has.

**R3 — The merged dataset is bit-identical to the serial one's.**
Not "the same answer": the same rows, in the same order, with the same values, the same validity
bits and the same dictionary — including the dictionary's **order**, and including the indices of
the rows that are NULL.

**R4 — The merge does not grow with the square of the number of parts.**
A merge that re-merges the accumulated result with every part copies the accumulated result once
per part. The merged validity mask is allocated **once**, and the parts' values are **moved** into
the result rather than copied into it.

**R5 — One thread means one thread.**
The parallel machinery adds nothing at one thread: the one-worker path is the serial path, with the
same rows in the same order. The one-thread *figure* does move, and that is R1's batch size
rather than the parallelism — and both the old figure and the new one are published.

**R6 — The load is measured at every thread count, and the incumbents are measured beside it.**
A load table with only the engine's column would be a table about the engine. `DuckDB` and `Polars`
load the same file at the same thread counts, in the same harness.

## 4. Out of scope, stated rather than deferred

- **The eager path still decodes every column.** That is what the eager path *is*: the point of
  the lazy path is to avoid it, and the comparison publishes both.
- **The merge is still serial.** It has to be: the merged dictionary's order depends on the parts
  being merged in order, and the parts' dictionaries are tiny (a few thousand entries) next to the
  rows.
- **The morsel driver** — overlapping the read with the operators — is not here. See spec 007's
  out-of-scope table.
- **The x86-64 measurement** stays blocked: there is no x86-64 machine.

## 5. What would make this wrong

- **The parts are merged in completion order.** Then the rows would be in the scheduler's order,
  and `SUM` would not be associative over them.
- **The merge repoints a NULL row's index.** A NULL row's index is undefined — the engine's
  convention is slot zero, and the validity mask is what says the row is NULL. Repointing it
  would give a NULL row a value, and the chunk would no longer be the chunk the serial read
  produced.
- **The merged dictionary's order differs from the serial read's.** The dictionary is part of the
  chunk, and a chunk with the same values in a different order is a different chunk.
