# ADR-0045 — The file boundary's cost

**Status**: accepted
**Date**: 2026-11-05
**Spec**: [039-the-engine-reads-a-stores-table](../../specs/039-the-engine-reads-a-stores-table/spec.md)

## Context

**The store writes a table as a set of files, and the manifest names them.** A commit appends a file and
never modifies an old one, so a table that is committed three times is three files and one manifest
([ADR-0044](0044-the-stores-half.md)).

**And the engine's only source is one file.** `ParquetSource` opens one Parquet file, and `open_table`
resolves a name to one path. **So a store's table cannot be read by the engine today** — and the embedder's
only way to read it is to open each file and run the query over each one, which is **wrong three times
over**: the rows of a filtered table are split across the files, the groups of an aggregate are split
across them, and the sides of a join are split across them.

**So the engine needs a source over a set of files.** And before it builds one, the question is what the
**file boundary** costs — because another file is another open, another footer, another metadata parse, and
the source's shape is decided by what that costs, not by what it feels like.

**And the experiment was already in the tree.** The published file's **six row groups** are the same rows
as the published file, so they are the natural experiment: **the same rows, read as one file and as six**,
through the engine's own reader, in the same campaign, on the same host. **The only difference between the
two reads is the number of files.**

## Decision

1. **The boundary's cost is measured before the source is built.** The probe writes the published file's
   own six row groups as six files, reads the one file and the six files, and compares the rows **row by
   row, in order**, on the key column — so "the same rows, read as one file and as six" is checked and not
   assumed.

2. **The file boundary costs nothing measurable.** The one file reads **66.003 ms** and the six files read
   **64.114 ms**, and the difference is **−1.152 ms** — **negative**, and inside the one file's own spread
   (**64.140–66.730 ms** against **63.424–65.541 ms**). **The six files were not slower.** Six opens, six
   footers and six metadata parses cost nothing a reader can see, because the rows dominate and the file's
   own overhead is a rounding error beside them.

3. **So the source's case is correctness, not performance.** The naive way to read a store's table — open
   each file, run the query over each one — is **wrong** for a filter, an aggregate and a join, and **a
   source that reads the set as one table is what makes it right**. There is no performance prize at the
   boundary to win, and the measurement says so.

4. **And so the source has to be correct and free.** Because the boundary is free, **the source may not
   cost anything either**: its whole case is that it is *correct*. A source that read the set correctly but
   slower than reading the files one by one would be a worse answer than the one the measurement rules out.

5. **The single-file path does not move.** `ParquetSource`'s read path is the most measured code in this
   repository, its published figures do not move, and the new source is **additive** — one new name on the
   surface, not a change to the old one.

6. **The source is built in the increment that follows this one**, and the reason is recorded in the spec's
   amendment: a change to the engine's read path has its own increment and its own measurement here, every
   time. **This increment publishes the figure that decides what it has to achieve.**

## Consequences

- **The store's tables can be read correctly for free.** The naive per-file read is wrong three times over,
  and the measurement says the correct way costs nothing at the boundary — so the engine's next source is
  not a trade, it is a fix.
- **The source's requirements are now known**: correct, and free. The merged metadata, the pruning over the
  union of the files' statistics, and the batches across the file boundary are all **required for
  correctness**, and none of them may cost anything.
- **The single-file path is untouched**, and its published figures stay where they are.
- **The store's crate is still not published**, and still needs the token and a tie in the publish order —
  and now, more than before, **the store is useful**: a table that is many files is a table the engine can
  be taught to read, and the cost of being taught is nothing.

## The correction

**The sentence about "the pruning over the union of the files' statistics" above was wrong about this
engine, and it is corrected in [ADR-0046](0046-the-source-over-a-set-of-files.md)** — the increment that
built the source found the claim to be false and recorded why.

**The engine does not prune row groups by the file's statistics.** [ADR-0010](0010-pushdown-equivalence.md)
accepted that cost as a deliberate choice — "the engine does not prune, and does not pretend to" — and
[spec 028](../../specs/028-the-probes-remainder/spec.md) recorded the same about the reader's pruning: it
"is not this spec's business". The statistics are used in two places, and **both of them are estimates,
not reads**: the join's build side's range, and the bound the memory estimate puts on the build side's
rows.

**What the source owes instead is that the statistics it publishes are the table's**: the range is the
union of the files' ranges, and the row groups are numbered across the files. **And that is a correctness
requirement too** — a set that published one file's range would publish a range the other files' rows fall
outside.

**And the measurement this ADR records is not touched by the correction.** The boundary's cost is the
boundary's cost, and the figures stand.

## References

- [`constitution.md`](../../constitution.md) II — the method: measure first, and the figures are derived from
  the committed reports rather than typed.
- [`constitution.md`](../../constitution.md) V — the method's evidence rules: the medians of at least three
  campaigns, and every quoted figure agreeing with the committed report.
- [`constitution.md`](../../constitution.md) VI — one predicate, six implementations, bit-exact.
- [`PERF.md`](../../PERF.md) §3t — the store's own write, and the file it writes.
- [`PERF.md`](../../PERF.md) §3u — the file boundary's cost.
- [`ADR-0044`](0044-the-stores-half.md) — the store's shape, and the glue that is left.
- [`spec 039`](../../specs/039-the-engine-reads-a-stores-table/spec.md) — the file boundary's cost, measured.
- [`STATUS.md`](../../STATUS.md) — where the project's open questions live.
