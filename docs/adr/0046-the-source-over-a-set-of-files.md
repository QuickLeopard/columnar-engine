# ADR-0046 — The source over a set of files

**Status**: accepted
**Date**: 2026-11-05
**Spec**: [040-the-source-that-reads-a-set-of-files](../../specs/040-the-source-that-reads-a-set-of-files/spec.md)

## Context

**The store writes a table as a set of files** — a commit appends one and never modifies an old one — and the
manifest names them ([ADR-0044](0044-the-stores-half.md)). **And the engine's only source was one file.**
`ParquetSource` opens one Parquet file, and `open_table` resolves a name to one path.

**So a store's table could not be read by the engine at all**, and the embedder's only way to read it was to
open each file and run the query over each one. **That is wrong three times over**: the rows of a filtered
table are split across the files, the groups of an aggregate are split across them, and the sides of a join are
split across them.

**And the file boundary is free.** Spec 039 measured it: the published file's own six row groups, read as one
file and as six, came to **66.003 ms** and **64.114 ms**, and the difference is **−1.152 ms** — negative, and
inside the one file's own spread ([ADR-0045](0045-the-file-boundarys-cost.md)). **So the source's case is
correctness, not performance**, and it has to be **correct and free**: a source that read the set correctly but
slower than reading the files one by one would be a worse answer than the one the measurement rules out.

**And the increment that built it found a wrong sentence of its own.** Spec 039's R2 and spec 040's R3 said
that the pushdown prunes row groups by the file's statistics, and that a row group which cannot survive is not
read. **The engine does neither.** [ADR-0010](0010-pushdown-equivalence.md) accepted that cost as a deliberate
choice — "the engine does not prune, and does not pretend to" — and [spec 028](../../specs/028-the-probes-remainder/spec.md)
recorded the same about the reader's pruning: it "is not this spec's business". The statistics are used in two
places, and **both of them are estimates, not reads**: the join's build side's range, and the bound the memory
estimate puts on the build side's rows.

## Decision

1. **The source is a new type, `FileSetSource`, and it is one new name on the surface.** `execute_on` takes a
   `ParquetSource` exactly as it did, and the set has its own entry point, `FileSetSource::execute`. **The
   single-file path did not move**: its published figures stay where they are, and the new source is additive.

2. **The files' readers are chained, and the chain is decoded in one pass.** The set builds each file's reader
   with the engine's own projection, the engine's own batch size and the engine's own pushdown, and hands the
   **chain** to the engine's own decoder. **The rows are therefore copied once, into the table's own columns.**
   **A merge would copy them twice** — once into each file's chunk, and once into the merged chunk — and that
   second copy was worth **8.747 ms**, **13%** of the read, when the first implementation did it that way. **The
   chained reader is what makes the source free**, and it is the reason the source has no read path of its own:
   it calls the engine's decoder, and the engine's decoder is the only one.

3. **The metadata is merged, and the schemas have to agree.** The files have to agree about their columns,
   their order, their types **and how their values are stored** — the engine's merge combines a
   dictionary-encoded column with a dictionary-encoded one and a plain column with a plain one, and a mix would
   drop rows. **A set whose files disagree is not a table, and the error says which file disagreed and how.**

4. **The statistics a set publishes are the table's statistics.** The range of a column over a set is the
   **union** of the files' ranges, and the row groups are the files' row groups **numbered across the files**,
   with the files' own ranges and the files' own rows. **A set that published one file's range would publish a
   range the other files' rows fall outside** — and that is a correctness requirement, not a tidiness one.

5. **The engine's own predicate reaches every file.** The rows that survive are the rows of the whole table,
   not the rows of the first file that answers.

6. **The batch size is derived from the merged row groups**, so no row group anywhere ends with a partial batch.
   And the parallel read splits the **merged** row groups into contiguous blocks and joins the parts **in block
   order** — the same rule the one-file read follows.

7. **The join's build side is one file, still.** `JoinSpec.build_path` is one path, and the build side opens one
   file. **So a store's table can be on the probe side of a join, and not on the build side** — and that is a
   change to the plan's own shape, which is its own increment.
   **That was true when it was written, and it is not true now**: spec 042 gave the build side a set, and
   `JoinSpec.build_paths` is a list of paths. It is corrected in
   [ADR-0053](0053-the-build-side-takes-a-set.md).

8. **The measurement is the acceptance.** The source is **free**: the set's read is **−0.599 ms** against the
   one file's — negative, and inside the one file's own spread — and in parallel **0.133 ms** against
   **35.123 ms**, a share of **0.004**. The six files' open is **0.187 ms** against one file's **0.064 ms**. And
   the rows are checked **row by row, in order**, against the one file's, and the same filtered plan counts the
   same **3,184,637** rows over the file and over the set.

9. **The correction is recorded here, and the sentence it corrects is corrected where it was published.** The
   pruning claim was wrong about this engine, and it was wrong in **spec 039's R2**, in **spec 040's R3** and in
   **ADR-0045's consequences**. All three now say what was true of this engine all along: the statistics a set
   publishes are the table's, and the engine's own predicate reaches every file. **The measurement ADR-0045
   records is not touched by the correction** — the boundary's cost is the boundary's cost.

## Consequences

- **The store's tables can be read by the engine, correctly, for free.** The naive per-file read was wrong three
  times over, and the measurement says the correct way costs nothing — so the source is not a trade, it is a
  fix. The glue between the store and the engine is still a **list of paths**, and the engine still does not
  depend on `columnar-store`.
- **The source is one new name, and the surface grew by one.** `execute_on` is unchanged, and the one-file read
  path is unchanged — every method of the engine's internal source trait delegates to the file's own method, so
  the code that reads one file is the code that ran before.
- **The store's table cannot be on the build side of a join yet.** That is the plan's shape, not the source's,
  and it is named rather than left to be discovered.
- **The engine's statistics were never a pruning decision, and the correction says so.** A reader of the
  specs 039 and 040 would have believed the engine skipped row groups it could not use. **It does not**, and
  that is now said where the claim was made.
- **The store's crate is still not published**, and the publish still needs the account, the token and a tie in
  the order.

## References

- [`constitution.md`](../../constitution.md) II — the method: measure first, and the figures are derived from the
  committed reports rather than typed.
- [`constitution.md`](../../constitution.md) V — the method's evidence rules: the medians of at least three
  campaigns, and every quoted figure agreeing with the committed report.
- [`constitution.md`](../../constitution.md) VI — one predicate, six implementations, bit-exact.
- [`PERF.md`](../../PERF.md) §3u — the file boundary's cost: the measurement this increment builds on.
- [`PERF.md`](../../PERF.md) §3v — the source's cost, and the claim it makes.
- [`ADR-0010`](0010-pushdown-equivalence.md) — the engine does not prune, and does not pretend to.
- [`ADR-0044`](0044-the-stores-half.md) — the store's shape, and the glue that was left.
- [`ADR-0045`](0045-the-file-boundarys-cost.md) — the boundary is free, so the source has to be correct and free
  — and the correction.
- [`spec 039`](../../specs/039-the-engine-reads-a-stores-table/spec.md) — the file boundary's cost, measured.
- [`spec 040`](../../specs/040-the-source-that-reads-a-set-of-files/spec.md) — the source over a set of files.
- [`STATUS.md`](../../STATUS.md) — where the project's open questions live.
