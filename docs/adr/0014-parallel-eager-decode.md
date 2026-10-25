# ADR-0014: The eager decode is parallel over row groups, and it uses its own batch size

**Status**: accepted
**Date**: 2026-10-25
**Related**: [ADR-0011](0011-parallel-decode.md) (the parallel lazy read this extends), [ADR-0008](0008-eager-parquet-decode.md) (the eager decode this parallelises), [ADR-0013](0013-parallel-operators.md) (the parallel operators), [spec 008](../../specs/008-parallel-eager-decode/spec.md)

## Context

ADR-0011 parallelised the engine's **lazy** read and said, in print, what was left: *"the eager
path is still single-threaded. It is the path the benchmark's resident numbers use."*

ADR-0013 parallelised the **operators**. What was left after that was the eager path's **decode**,
and it was the largest number in the repository:

| system | load (file → queryable) |
|---|---|
| **engine** (eager) | **1277.8 ms** |
| duckdb | 1218.0 ms |
| polars | 1015.5 ms |

The engine was **behind** both incumbents at the one thing it was supposed to be good at, and the
gap was 144× the cost of everything spec 007 had parallelised.

## The measurement that decided the shape of the work

The first question is not "how do we parallelise it" but "**what is actually slow**", because the
answer decides whether parallelising is worth anything at all.

A probe split the decode in two: Arrow's own decode with no engine conversion, and the engine's
Arrow→chunk conversion on top of it.

| | ms | share |
|---|---|---|
| Arrow's decode, alone | 376.1 | 31% |
| **the engine's conversion** | **823** | **69%** |

**The bottleneck was never Arrow.** It was the conversion — and the conversion's dominant cost is
**per batch, per column**: every batch sets up its readers, allocates its arrays, and rebuilds each
dictionary column's table of merged dictionary slots.

That is a cost that parallelises *well* (it is spread over sixteen columns and thousands of
batches), so the parallel decode was worth doing. But it is also a cost that scales with the
**number of batches**, and that is where the first defect was.

## The decision that was not hard: the batch size

**The eager path used the Parquet reader's default batch size — 1,024 rows.** The lazy path had
already moved to 8,192, with a documented measurement behind it, but the eager path was never
touched: it read **sixteen** columns, and it rebuilt sixteen dictionary tables 5,860 times instead
of 733.

The controlled comparison — the same binary, the same process shape, one change at a time:

| | one thread |
|---|---|
| the eager path as it was | **1145.6 ms** |
| the eager path with a measured batch size | **587.0 ms** |

**1.95×, from one constant.** And the constant was then *measured* rather than guessed, over the
real file and all sixteen columns:

| rows per batch | one thread | six threads |
|---|---|---|
| 1,024 | 1292.4 ms | 393.5 ms |
| 4,096 | 704.2 ms | 295.8 ms |
| 8,192 | 623.5 ms | 286.8 ms |
| **32,768** | **564.5 ms** | **274.6 ms** |
| 131,072 | 577.0 ms | 271.9 ms |
| 262,144 | 636.2 ms | 273.3 ms |

The optimum is broad, and 32,768 is its middle. **The eager path has its own constant**, because
the per-batch cost scales with the column count and the two paths read a different number of
columns.

## The decision that was hard: the merge

The parallel decode produces one dataset per worker, and those have to be merged. Three things
about that merge are decisions.

### 1. The parts are merged in block order, and the merge is one pass

The parts are joined in the order the workers were spawned, which is block order. That is
ADR-0011's rule applied one layer up, and it has the same reason.

The **one pass** is new, and it was a defect. The merge used to be a *fold*: each part was merged
into the accumulated result, and each merge **copied the accumulated result**. So *n* parts cost
*n* copies of the accumulated one, and the work grew with the **square** of the number of parts.

For the validity masks that is not a copy, it is an **allocation storm**: a mask of six million
rows is 750 KB, and each concatenation allocated a fresh one. The measurement:

| parts | the merge |
|---|---|
| 2 | 85 ms |
| 6 | **205 ms** |

So the merge got *more* expensive as the decode got more parallel. `NullBitmap::concat_all` does
it in one pass — one allocation, one copy of each mask — and the six-thread read went from
**366.7 ms to 276.4 ms**.

### 2. The merge moves the parts' values rather than copying them

The old merge cloned each part's vectors and extended the clone. The new merge takes the parts'
vectors and **moves** their contents into the result.

That is not a micro-optimisation: it means the peak memory of a parallel eager decode stays at
about **one** copy of the data rather than two. The merged result grows *out of* the parts instead
of alongside them, and a part's megabytes are freed as it is consumed.

### 3. A NULL row's index stays undefined

**A NULL row's dictionary index is undefined.** The engine's convention is slot zero, and the
validity mask is what says the row is NULL — the same convention the plain columns follow, where a
NULL row's value slot holds a zero.

The merge got this wrong at first. It treated the placeholder index as a live reference into the
part's dictionary and repointed it — so a NULL row ended up pointing at a real dictionary slot.
**No answer changed**, because the validity mask still said the row was NULL. But the chunk's bytes
were no longer the bytes the serial read produced, and the "bit-identical" claim would have been
false.

The fix is to consult the part's validity mask: a NULL row's index is set to zero, not remapped.

## The result

The load sweep — the engine and both incumbents, at the same thread counts, on the same file, in
the same harness:

| threads | engine's workers | **engine** (load) | duckdb (load) | polars (load) |
|---|---|---|---|---|
| 1 | 1 | **550.2 ms** | 1207.0 ms | 995.5 ms |
| 2 | 2 | 378.6 ms | 607.0 ms | 513.4 ms |
| 4 | 4 | 304.6 ms | 412.0 ms | 349.5 ms |
| 6 | 6 | **226.2 ms** | 221.0 ms | 189.3 ms |
| 8 | 6 | 223.7 ms | 227.0 ms | 186.6 ms |
| 14 | 6 | 223.0 ms | 224.0 ms | 187.9 ms |

**The engine goes from 0.95× DuckDB to 2.2× DuckDB at one thread**, and it is level with DuckDB at
six. Every figure is the median of three full sweeps of five timed iterations.

The engine's load is capped by the file's **six row groups**, exactly as the lazy read is: the
workers column shows six from eight threads on, and the curve is flat there.

## The tests, and what they have teeth

The property is **bit-identical**: the same rows, in the same order, with the same values, the same
validity bits, and the same dictionary — including the dictionary's order, and including the
indices of the rows that are NULL.

`crates/columnar-engine/tests/parallel_eager.rs` compares the parallel read against the serial
read over a file whose row groups hold **different** dictionary values. That detail is the point
of the fixture: if the row groups held the same values, every group's dictionary would be
identical, the merge's remap would be the identity, and **a merge that forgot to repoint the
indices would still produce the right answer**.

That is not hypothetical. The first version of the fixture did have identical dictionaries, and
the mutation below — a merge that does not repoint the indices at all — **passed**. The fixture
was wrong, and it was fixed rather than the mutation being excused.

The tests are **mutation-verified**:

| mutation | caught by |
|---|---|
| the parts merged in reverse order | 2 tests |
| the NULL rows' indices repointed like live references | 2 tests |
| the indices not repointed at all | 4 tests |
| the validity concat's mid-word shift dropped | 3 engine tests and 2 core tests |

## Consequences

**Good.**

- The engine's largest number became a small one: **1277.8 ms → 550.2 ms**, 2.32×, and the engine
  is now **ahead** of both incumbents at the load instead of behind.
- The parallel eager decode's peak memory is about **one** copy of the data, not two.
- The merge is a single pass, so it does not get more expensive as the decode gets more parallel.

**Bad, and accepted.**

- **The batch size is a constant that has to be measured again when the column count changes.**
  The eager path and the lazy path now have different ones, and a future column-heavy file would
  want a third.
- **The merge is serial.** It has to be: the merged dictionary's order depends on the parts being
  merged in order. At six parts it is about a tenth of the decode, and it is the price of the
  order.
- **The eager path still decodes every column.** That is what it is for.

**Neutral.**

- The **lazy** path is unchanged, apart from sharing the rewritten merge.
- The **x86-64** measurement is still blocked. There is no x86-64 machine.

## Alternatives considered

**Parallelise the conversion inside a row group rather than across row groups.** Rejected. A row
group is the unit the file already has, and the lazy read already splits on it; splitting inside
one would mean splitting its dictionary across workers and merging the pieces — more work, more
contention, and no benefit.

**Give every worker the whole file and let them race.** Rejected, and not because it is slow: the
answer would depend on the scheduler, which is the one thing this repository will not trade.

**Merge the parts in completion order and sort afterwards.** Rejected. The sort is more work than
the merge, and the merge is what the order is decided by.

**Keep the eager path's batch size and rely on the parallelism alone.** Rejected. The batch size
was worth 1.95× on its own, and the parallelism was worth another 2.46× — and the first is free
once it is measured.

**Make the eager path read only the columns a query needs.** That is the lazy path. It exists, it
is measured, and the comparison publishes both.

## Revisit when

- The morsel driver arrives, and the read and the operators overlap.
- The merge's serial share becomes the bottleneck — it is about a tenth of the decode now, and a
  tenth is not worth attacking.
- A file with many more row groups arrives, and the read's row-group cap stops being the binding
  one.
