# Spec 039 — The engine reads a store's table

**Status**: open
**Started**: 2026-11-05

## The problem

**The store writes a table as a set of files, and the manifest names them.** That is the store's half, and
spec 038 built it: a commit appends a file and never modifies an old one, and the manifest is the table's
truth.

**And the engine's only source is one file.** `ParquetSource` opens one Parquet file, and `open_table`
resolves a name to one path. **So a store's table cannot be read by the engine today** — and the embedder's
only way to read it is to open each file and run the query over each one, which is **wrong three times
over**: the rows of a filtered table are split across the files, the groups of an aggregate are split
across them, and the sides of a join are split across them.

**So the engine needs a source over a set of files, and the set has to be read as one table.** That is not
one thing, it is four:

1. **The metadata.** The scan is built around one file's row groups, its column index and its row-group
   ranges. A source over many files has to **merge** them, and the row groups have to be numbered across
   the files.
2. **The pruning.** The pushdown prunes row groups by the statistics in the file's metadata. **A source
   over many files has to prune against every file's statistics**, or a row group that cannot survive is
   read anyway.
3. **The batches.** The reader's batch rule crosses the file boundary: the last batch of one file and the
   first of the next are one batch, or two — and the answer must be the same either way.
4. **The cost.** The file boundary is not free: another file is another open, another footer, another
   metadata parse. **How much is it** is a measurement, and until it is measured, the source's shape is an
   argument.

**And the measurement comes first**, the way it always does here. The published file's **six row groups**
are the same rows as the published file, so they are the natural experiment: **the same rows, read as one
file and as six**, and the difference between the two is the file boundary's cost, measured.

## The requirements

### R1 — The engine gains a source over a set of files, and the single-file path is untouched

`ParquetSource` opens one file today, and its read path is the most measured code in this repository. **The
single-file path stays exactly as it is**, its published figures do not move, and the new source is
**additive**: a new name on the surface, not a change to the old one.

### R2 — The pushdown sees the whole table

The pruning is over **the union of the files' row groups**, and the statistics that decide it are the
union of the files' statistics. **A row group that cannot survive is not read**, whichever file it is in —
and the engine's own predicate is applied to the rows of every file, not to the first one that answers.

### R3 — The answers are the engine's

A table read from six files gives **the same rows, in the same order, as the same rows in one file** — the
engine's own `NULL` and `NaN` rules, its own comparison rules, its own ordering. The house's rule is
**one predicate, six implementations, bit-exact**, and a sixth file layout is not an exception to it.

### R4 — The store's manifest is the input

The source takes **the files**, and the embedder hands it the manifest's. **The engine does not depend on
`columnar-store`** — it never has, and this increment does not change that: the glue is the list of paths,
and the store is one way to produce it.

### R5 — The cost of the file boundary is a figure

**The same rows, read as one file and as six**, on the same host, in the same campaign, and the difference
between the two published. **So the source's shape is decided by a measurement** — whether the boundary is
worth a new source at all, and what it has to achieve to be worth having.

### R6 — The answer is published either way

If the many-files read is within reach of the one-file read, the source is built and the figure says how
much it costs. If it is not, **the refusal is published**, the store's tables are read file by file, and
that is written down as what they are.

## The plan's shape

Five phases, and the edges between them:

1. **The instrument** — the probe: the published file's six row groups written as six files, and the
   engine's read of the one and of the six, in the same campaign.
2. **The campaigns** — three campaigns of the probe, the raw reports committed under their own names.
3. **The source** — the engine's source over a set of files, with the merged metadata, the pruning over
   the union, and the batches across the boundary.
4. **The publication** — the section that publishes the boundary's cost and the answers that did not
   change, the guard checks every figure inside it, and the ADR records the source's shape.
5. **The closure** — the spec's status is `done` with the date it finished, and CI is green on the publish
   commit.

The edges: 1 → 2 → 3 → 4 → 5, and 1 → 4 (the boundary's cost is published whether or not the source is
built).

## The scope

| | |
|---|---|
| The engine's own work | **The single-file path does not move.** The new source is additive, and the published figures of the old one stay where they are. |
| The store | **Unchanged.** It writes the files; the engine reads them; the glue is a list of paths. |
| The published set | **The standing set does not slide.** The probe is a new instrument with a new family of reports. |
| The surface | **One new name**, and the surface list is updated with it — the rule that every type in a public signature is on the list holds. |
| The library's publish | **Not this increment's.** It is a separate act, with its own account, and it is written down as an open question. |

## The measurement

1. **Three campaigns of the probe**, on this machine, the raw reports committed under their own names. The
   medians are of three campaigns, and the spread is published.
2. **The host's state is published** with every campaign — the load average — because the answer depends on
   the host's state on the day, and spec 036 is the reason that is not optional.
3. **The comparison is like for like**: the same rows, in the same batches, on the same host, in the same
   campaign. The only difference between the two reads is **the number of files**.
4. **The answers are checked, not assumed**: the six-files read gives the same rows as the one-file read,
   in the same order, and the check is part of the probe.

## The references

| | |
|---|---|
| [`constitution.md`](../../constitution.md) II | the method: measure first, and the figures are derived from the committed reports |
| [`constitution.md`](../../constitution.md) V | the method's evidence rules: the medians of at least three campaigns, and every quoted figure agreeing with the committed report |
| [`constitution.md`](../../constitution.md) VI | one predicate, six implementations, bit-exact |
| [`PERF.md`](../../PERF.md) §2d | where the cold time goes: the read the boundary's cost is measured against |
| [`PERF.md`](../../PERF.md) §3t | the store's own write, and the file it writes |
| [`ADR-0043`](../../docs/adr/0043-the-write-side.md) | the decisions the write side's numbers make |
| [`ADR-0044`](../../docs/adr/0044-the-stores-half.md) | the store's shape, and the glue that is left |
| [`spec 038`](../038-the-stores-half/spec.md) | the store's half |
| [`STATUS.md`](../../STATUS.md) | where the project's open questions live |
