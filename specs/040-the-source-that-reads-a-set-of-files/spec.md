# Spec 040 — The source that reads a set of files

**Status**: open
**Started**: 2026-11-05

## The problem

**The store writes a table as a set of files** — a commit appends one and never modifies an old one — and
the manifest names them. **And the engine's only source is one file.** `ParquetSource` opens one Parquet
file; `open_table` resolves a name to one path.

**So a store's table cannot be read by the engine today**, and the embedder's only way to read it is to
open each file and run the query over each one — which is **wrong three times over**: the rows of a
filtered table are split across the files, the groups of an aggregate are split across them, and the sides
of a join are split across them.

**And the boundary is free.** Spec 039 measured it: the published file's own six row groups, read as one
file and as six, gave **66.003 ms** and **64.114 ms**, and the difference is **−1.152 ms**, inside the one
file's own spread ([ADR-0045](../../docs/adr/0045-the-file-boundarys-cost.md)). **So the source's case is
correctness, not performance**, and it has to be **correct and free**: a source that read the set correctly
but slower than reading the files one by one would be a worse answer than the one the measurement rules out.

**And it is not one thing. It is four.**

1. **The metadata.** The scan is built around one file's row groups, its column index and its row-group
   ranges. A source over many files has to **merge** them, and the row groups have to be numbered across
   the files.
2. **The pruning.** The pushdown prunes row groups by the statistics in the file's metadata. **A source
   over many files has to prune against every file's statistics**, or a row group that cannot survive is
   read anyway.
3. **The batches.** The reader's batch rule crosses the file boundary, and the rows come back **in the
   files' order** — file by file, and inside a file, row group by row group.
4. **The answers.** The set's rows are the rows, in the same order, with the same `NULL` and `NaN`
   behaviour, as the same rows in one file. **The house's rule is one predicate, six implementations,
   bit-exact**, and a sixth file layout is not an exception to it.

## The requirements

### R1 — The engine gains a source over a set of files, and the single-file path is untouched

`ParquetSource`'s read path is the most measured code in this repository. **It stays exactly as it is**,
its published figures do not move, and the new source is **additive**: a new name on the surface, not a
change to the old one.

### R2 — The metadata is merged, and the schemas must agree

The schema is the files' own, and **the files have to share it**: a set whose files disagree about their
columns is not a table, and the error says so. The rows are the sum, the row groups are numbered across the
files, and the row groups' rows are the concatenation.

### R3 — The pruning sees the whole table

The pruning is over **the union of the files' row groups**, and the statistics that decide it are the
union of the files' statistics. **A row group that cannot survive is not read**, whichever file it is in —
and the engine's own predicate is applied to every file's rows, not to the first one that answers.

### R4 — The batches cross the boundary, and the rows keep the files' order

The reader's batch size is derived from the **merged** row groups, as it is derived from a file's ✓. The
rows come back **file by file**, and inside a file row group by row group, and the parallel read splits
the merged row groups into contiguous blocks and joins the parts **in block order** ✓ — the same rule the
one-file read follows.

### R5 — The answers are the engine's

A table read from six files gives **the same rows, in the same order, as the same rows in one file** — the
engine's own `NULL` and `NaN` rules, its own comparison rules, its own ordering. **And the test is the
one-file read**: the same file, written as six, read as six, and compared row by row.

### R6 — The source may not cost anything

**The set's read is level with the files read one by one**, and that is the published figure the source
has to hold. **The boundary is free, so the source may not cost anything either** — its whole case is that
it is correct.

### R7 — The surface carries the new name, and the rule holds

**One new name**, and the surface list is updated with it — the rule that every type in a public signature
is on the list holds, and the list is checked.

### R8 — The scope is named

**The source reads a set of files that share a schema.** It does not merge schemas, it does not read a
store's manifest — **the embedder hands it the paths**, and the engine does not depend on `columnar-store`
— and it does not read a file that is not Parquet.

## The plan's shape

Five phases, and the edges between them:

1. **The source** — the merged metadata, the pruning over the union, the batches across the boundary, and
   the rows in the files' order.
2. **The tests** — the set's rows against the one file's, row by row, in order; the pruning over a union
   that spans the files; the order across the boundary.
3. **The campaigns** — the source's read against the one-file read and against the files read one by one,
   the raw reports committed under their own names.
4. **The publication** — the section that publishes the source's cost, the guard checks every figure inside
   it, the surface list carries the new name, and the ADR records the source's shape.
5. **The closure** — the spec's status is `done` with the date it finished, and CI is green on the publish
   commit.

The edges: 1 → 2 → 3 → 4 → 5, and 3 → 4.

## The scope

| | |
|---|---|
| The engine's own work | **The single-file path does not move.** The new source is additive, and the published figures of the old one stay where they are. |
| The store | **Unchanged.** It writes the files; the engine reads them; the glue is a list of paths. |
| The published set | **The standing set does not slide.** The source's figures are new, in their own section, in their own family of reports. |
| The surface | **One new name**, and the surface list is updated with it. |
| The library's publish | **Not this increment's.** It is a separate act, with its own account, and it is written down as an open question. |

## The measurement

1. **Three campaigns**, on this machine, the raw reports committed under their own names. The medians are
   of three campaigns, and the spread is published.
2. **The host's state is published** with every campaign — the load average — because the answer depends on
   the host's state on the day, and spec 036 is the reason that is not optional.
3. **The comparison is like for like**: the same rows, the same columns, the same host, in the same
   campaign. **The set's read** against **the one-file read** and against **the files read one by one**.
4. **The answers are checked, not assumed**: the set's rows against the one file's, row by row, in order.

## The references

| | |
|---|---|
| [`constitution.md`](../../constitution.md) II | the method: measure first, and the figures are derived from the committed reports |
| [`constitution.md`](../../constitution.md) V | the method's evidence rules: the medians of at least three campaigns, and every quoted figure agreeing with the committed report |
| [`constitution.md`](../../constitution.md) VI | one predicate, six implementations, bit-exact |
| [`PERF.md`](../../PERF.md) §3u | the file boundary's cost: the measurement this increment builds on |
| [`ADR-0045`](../../docs/adr/0045-the-file-boundarys-cost.md) | the boundary is free, so the source has to be correct and free |
| [`ADR-0044`](../../docs/adr/0044-the-stores-half.md) | the store's shape, and the glue that is left |
| [`spec 039`](../039-the-engine-reads-a-stores-table/spec.md) | the file boundary's cost, measured |
| [`STATUS.md`](../../STATUS.md) | where the project's open questions live |
