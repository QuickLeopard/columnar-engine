# ADR-0053: The build side takes a set

**Status**: accepted
**Date**: 2026-11-06
**Reviewers**: TBD

## Context

Spec 040 gave the engine a source over a **set** of files: `FileSetSource` reads several Parquet files as
one table, and the file boundary was measured as **free** — the set's read is −1.416 ms against the same
rows in one file.

The **join's build side** was not part of that. `JoinSpec.build_path` was one `PathBuf`, the build side
opened one `ParquetSource`, and every rule the build side followed was a rule **one file** publishes:
the key's index, the key's range, the row groups' ranges, the number of workers. So a store's table — which
is several files — could be on the **probe** side of a join and not on the **build** side, and ADR-0046
named that as the next increment: *"that is a change to the plan's own shape, which is its own increment."*

The increment had to answer four questions, and the answers decided the shape:

1. **The cost.** Building the table from six files against one file.
2. **The identity.** The same rows, the same output rows, the same answer.
3. **The shape.** The direct-addressed table's width: the key's range, and the union of the files' ranges.
4. **The estimate.** What the budget is charged for a set, against what the build uses.

And one question about the public shape: `JoinSpec` is on the surface, so **the build side's shape is a
public break** — and the crates had never been published, so the break was free.

## Decision

1. **The build side takes a set, and one path is still one path.** `JoinSpec.build_path: PathBuf` becomes
   `JoinSpec.build_paths: Vec<PathBuf>`. A plan that names one path opens a `ParquetSource` and is read,
   charged and reported exactly as it was before the set existed; a plan that names several opens a
   `FileSetSource` and is read as one table. **The break is one field, on one type, and it was made while
   the crates were unpublished.**

2. **The source is the seam, and the rules are the source's.** `EngineSource` — the engine's own trait,
   crate-private — gains three methods: `column_index`, `column_range` and `row_group_ranges`. The build
   table, the key's type and the estimate are now written **once**, over `EngineSource`, and the one-file
   path and the set path are the same code. `BuildSide` holds a source rather than a file, and the source
   is the engine's own: `BuildSource::File` or `BuildSource::Set`.

3. **The set's range is the union, and the union of the parts is the whole.** A file's range is the file's
   own; a set's range is the **union** of the files' ranges. So the direct-addressed table's width does not
   change when a file becomes a set of its own row groups — and the estimate does not change either,
   because the three facts it takes (the rows, the range, the workers) are the same.

4. **The plan's own shape is the only thing that changed.** The build side's *source* is now a set; the
   **plan** still names it as a list of paths, and the engine opens what the plan names. The store's table
   reaches the build side because `Table::paths` hands out the list, and the list is what the plan carries.

## Consequences

**What becomes easier.** A store's table can be on the build side of a join, and the set is read as one
table — file by file, and inside a file row group by row group — rather than file by file with the rows
split across them. The estimate, the key's type and the build table are one implementation, not two.

**What becomes harder.** `JoinSpec` changed shape, and every plan that named one file now names a list of
one. The engine's own `join_cost_probe` measures the one-file build side and was changed to take the
first path of the list; **the measurement is not touched** — the one-file path is the same path.

**What we gave up.** The build side's path is no longer a single `PathBuf`, so a plan that wants a set
cannot be built with one path. That is the price of the set, and it was free.

## Measurement

**The cost, the identity, the shape and the estimate** are in [PERF.md §3y](../../PERF.md) — the table's
construction, one file against six, in the same campaign; the same rows and the same answer; the key's
range, one file and the union; and the estimate, one file and six.

The headline: **the set costs +1.62 ms to build** (60.96 ms against 62.58 ms, 3 campaigns), **+0.18 ms to
open** (0.141 ms against 0.317 ms — six footers against one), **the answer is the same to the bit**, and
**the estimate is the same to the byte** (103,875,021 bytes both ways).

The identity was measured, not assumed: the same 1,501,215 rows and the same sum, 79,530,106,032.52351,
from one file and from six.

## Confidence

**high** — the change is small (one field on the surface, three methods on a crate-private trait, and one
generic implementation of each rule), the measurement is three campaigns with the pairs measured together,
and the identity is exact. The only thing that could change the decision is a set whose files' ranges do
**not** union to the whole — which cannot happen, because the union of a partition's parts is the
partition.
