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
2. **The statistics.** **The engine does not prune row groups by the file's statistics** — [ADR-0010]
   accepted that cost, and the reader reads every page of the columns it needs — **so there is nothing to
   prune across the files.** What the source owes is that the statistics it publishes are the **table's**:
   the range is the union of the files' ranges, and the row groups are numbered across the files. **And the
   engine's own predicate is applied to every file's rows**, not to the first one that answers.
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

### R3 — The statistics are the table's, and the predicate reaches every file

**The engine does not prune row groups by the file's statistics.** [ADR-0010] accepted that cost: the
reader reads every page of the columns it needs, and the engine's own predicates are evaluated over the
decoded batch. **So there is nothing to prune across the files, and nothing to merge for a pruning
decision.** What the source owes instead is two things.

**The statistics it publishes are the table's.** The range of a column over a set is the **union** of the
files' ranges, and the row groups are the table's: the files' row groups, numbered across the files, with
the files' own ranges and the files' own rows. A set that published one file's range would publish a
range that the other files' rows fall outside.

**And the engine's own predicate reaches every file.** The rows that survive are the rows of the whole
table, not the rows of the first file that answers.

### R3a — The amendment to R3, and why

**R3 was published as "The pruning sees the whole table", and it was wrong about this engine.** It said
that the pushdown prunes row groups by the file's statistics, and that a row group which cannot survive
is not read. **The engine does neither.** ADR-0010 recorded the cost as a deliberate choice — "the engine
does not prune, and does not pretend to" — and spec 028 recorded the same thing about the reader's
pruning: it "is not this spec's business". The statistics are used in two places, and **both of them are
estimates, not reads**: the join's build side's range, and the bound the memory estimate puts on the
build side's rows.

**So the requirement was amended, and the amendment is recorded here.** The requirement was not "prune
across the files" but "the statistics are the table's, and the predicate reaches every file" — and the
second half of that is a **correctness** requirement, which the original also had, and the first half is a
correctness requirement too: a set that published one file's range would publish a range the other
files' rows fall outside.

**And the amendment does not change the increment's work.** The source merges the metadata, the predicate
reaches every file, and the answers are the engine's. What changed is the name of the thing being merged
and the reason it has to be merged.

**The correction is recorded in [ADR-0046](../../docs/adr/0046-the-source-over-a-set-of-files.md)**, and
the sentence it corrects is corrected where it was published: in [spec 039](../039-the-engine-reads-a-stores-table/spec.md)
and in [ADR-0045](../../docs/adr/0045-the-file-boundarys-cost.md).

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

**The yardstick is the one-file read**: the same rows, in one file, is the ideal, and the set's read is
measured against it. **The boundary is free, so the source may not cost anything either** — its whole
case is that it is correct.

**And the files read one by one are not the yardstick**, although the plan first named them as one. The
naive way's read is **not the same answer**: it produces six answers, and the rows of a filtered table
are split across them. It is measured anyway, because it is what the source replaces, and what it costs
is published — but the figure that decides whether the source is free is the one against the one file's
read.

### R7 — The surface carries the new name, and the rule holds

**One new name**, and the surface list is updated with it — the rule that every type in a public signature
is on the list holds, and the list is checked.

### R8 — The scope is named

**The source reads a set of files that share a schema.** It does not merge schemas, it does not read a
store's manifest — **the embedder hands it the paths**, and the engine does not depend on `columnar-store`
— and it does not read a file that is not Parquet.

### R9 — The amendment to the scope, and why

**The scope said the store is unchanged, and it was wrong about the glue.** The store's `Table::files` and
`Table::file_path` were already there, so the embedder *could* build the list of paths -- but the glue is
**a list of paths**, and the engine's source takes exactly that, so the store handing it over is one call
rather than a loop in every embedder.

**So the scope was amended: the store gains `Table::paths`, and nothing else.** It writes the files, it
names them, and it hands the engine the list.

**And the amendment does not change the increment's shape.** The source is the engine's, the measurement is
the engine's, and the store's one method is the last inch of the glue.

### R10 — The store's publish, and the record that was wrong about it

**The store's crate could not be packaged**, and the reason was a **versioned dev-dependency** on a crate
that is not on crates.io: a versioned dependency has to resolve there, and a **path-only** one is dropped
from the published manifest. **And the package carried no licence file**, which every other published crate
carries.

**And ADR-0044's reason for holding the store back was wrong in two ways.** It said the store depends on
the core -- **it depends on nothing in this workspace** -- and it said the increment that would change the
store's API is the one that reads a table of many files -- **that increment did not change the store's API
at all**, and it is this one.

**So the store publishes with the others**, the publish order is computed from the manifests with the tie
the graph cannot break broken by the list, and **the publish itself is the user's act**. The record is
[ADR-0047](../../docs/adr/0047-the-stores-publish.md), and the two wrong sentences are corrected where
they were published.

## The plan's shape

Five phases, and the edges between them:

1. **The source** — the merged metadata, the merged statistics, the batches across the boundary, and the
   rows in the files' order.
2. **The tests** — the set's rows against the one file's, row by row, in order; the statistics over a
   union that spans the files; the order across the boundary.
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
| The store | **One method.** It writes the files; the engine reads them; the glue is a list of paths. **And the list is the store's to hand over**: `Table::paths` is it, in the table's order. The scope said "unchanged", and the **amendment** is recorded in [R9](#r9--the-amendment-to-the-scope-and-why). |
| The published set | **The standing set does not slide.** The source's figures are new, in their own section, in their own family of reports. |
| The surface | **One new name**, and the surface list is updated with it. |
| The join's build side | **One file, still.** `JoinSpec.build_path` is one path, and the build side opens one file. **So a store's table can be on the probe side of a join, and not on the build side** — and that is a change to the plan's own shape, which is its own increment. |
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
