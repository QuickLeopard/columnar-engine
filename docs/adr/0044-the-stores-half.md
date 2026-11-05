# ADR-0044 — The store's half

**Status**: accepted
**Date**: 2026-11-05
**Spec**: [038-the-stores-half](../../specs/038-the-stores-half/spec.md)

## Context

**The engine is a read-only query engine over Parquet, and an embedded store is made of three things it
does not have.** A **write path** — rows in, row groups out. A **commit protocol** — a table is many files,
and the set of them has to become visible atomically. And a **catalog that survives the process** —
`Catalog` is the embedder's, in memory, and its own documentation says so: *"the engine never reads one
from disk, never mutates one, and never outlives the borrow the embedder gave it"*.

**Spec 037 measured the first two and decided their shape** ([ADR-0043](0043-the-write-side.md)): the write
of the published file is **1844.195 ms**, a row group costs **306.230 ms**, the **commit** is the five steps
— the data file durable, the manifest written and durable, the `rename`, the directory durable — and it is
**7.651 ms**, which is **0.004** of the write. **So the protocol's shape is not constrained by the flush**,
and the store needs no write-ahead log and no rollback to be affordable. The encoding keeps the
**dictionary** on, because the plain file takes **1.55×** the bytes and reads back **2.60×** slower.

**And the third thing did not exist at all.** There was no catalog on disk, and there was no writer.

**And the store's half could not be read by the engine.** A store's table is **many files** — one per
commit, append-only — and the engine's only source is `ParquetSource`, which is **one file**. **So the
engine's side of that glue — a source that reads a table which is many files — is its own increment**, and
this one builds the store's half.

## Decision

1. **The store is a crate, and it is below the engine.** `columnar-store` depends on `columnar-core`'s
   absence, `parquet`, `arrow`, `serde`, `serde_json` and `thiserror` — and **not** on `columnar-engine`.
   **The engine's read path is untouched**, and its published figures do not move. The store writes the
   files the engine reads; the glue between them is a path.

2. **A table is a set of files, and the manifest names them.** A commit appends a **new** file and **never
   modifies an old one**, so the store grows by adding to the set and never rewrites it. The manifest is the
   table's truth: its files, their rows, their bytes, and the table's totals. It is small, it is JSON, and
   it is the one thing a reader has to believe.

3. **The commit is the five steps that were measured, and there is no write-ahead log.** The data file is
   made durable **first**, then the manifest that names it is written, made durable, and `rename`d over the
   old one, and the directory is made durable **last**, so the rename itself survives. **A reader that sees
   the new manifest sees the new data; a reader that sees the old one sees the old data. There is no third
   answer**, and a commit is a rename.

4. **The catalog survives the process.** The manifest is on disk, it is read back when the store is opened,
   and the tables it names are the store's tables. **The engine's in-memory `Catalog` stays the embedder's**
   — the store does not touch it and does not need it.

5. **The writer's grid is the store's, and the store checks it.** **65,536 rows**, the grid the published
   file was written in. The reason is spec 037's finding: the Parquet writer splits every batch it is
   handed into mini-batches of 1,024 values, and the dictionary's fallback to plain is decided at one of
   those boundaries, so **the file's bytes are a function of the grid**. A batch off the grid is **refused
   with an error**, because a file that comes out differently is not a bug in the store but a caller that
   batched differently — and the store would rather say so than write a file whose bytes nobody can predict.

6. **The encoding policy is the caller's.** The store's default is the dictionary on, a million rows a row
   group and SNAPPY, and `writer_properties` names the columns to write plain. **The store does not decide
   the encoding**, and a caller that writes the same rows under a different policy gets a different file —
   which is the caller's choice, and the caller's file.

7. **The acceptance is the strongest one available.** Fed the published file's own rows in 65,536-row
   batches with the file's own policy, **the store writes the published file, byte for byte** — **242,857,841**
   bytes, compared byte for byte and found equal, in every campaign. **So the store's correctness claim is
   not "it wrote something" but "it wrote the file the generator wrote, and the two are equal"**, and the
   figure is committed and checked.

8. **The scope is named, so it is a decision and not an oversight.** **No SQL. No transactions with rollback.
   No MVCC, no snapshot isolation. No concurrent writers. No row-level updates. No compaction. One writer.**
   A table grows by appending a file and committing it; it does not shrink, and it does not change in place.

9. **The crate was not published yet — and the two reasons for it were wrong.** `publish = false`, and the
   reason in the manifest was: the crate's API is new, and **the next increment changes it** — the engine has
   to read a table that is many files, and the store's own surface will follow that. **And the publish
   script's total-order check had to learn a tie**, because the store depends on the core and so does the
   kernels, so two crates are ready at once.

   **Both sentences are corrected in [ADR-0047](0047-the-stores-publish.md).** The increment that was named
   — spec 040 — is done, and **the store's own surface did not have to move for it**: `Table::files` and
   `Table::file_path` were already there, and `Table::paths` now hands the engine the list of paths it takes.
   **And the store does not depend on the core** — it depends on nothing in this workspace — so the tie is
   beside the core, not after it, and the publish script breaks it by the list rather than refusing it.
   **The crate is published with the others now**, and the publish itself is the user's act.

## Consequences

- **The store writes the published file byte for byte**, and the figure is checked by the guard inside the
  section that publishes it. The store's mechanism — the writer, the grid, the manifest and the commit — is
  proven against a file this repository already has.
- **Spec 037's finding is now a contract.** The batch size is no longer a surprise the repository explains
  after the fact; it is a rule the store enforces, with an error that says what broke.
- **The store's numbers are the ones already published.** The store introduces no new figures: its write is
  the write [§3s](../../PERF.md#3s-the-write-side) measured, and its commit is the same five steps.
- **The store cannot be read by the engine yet.** A table is many files and the engine's only source is one
  file, so **the engine's side of the glue is the next increment**, with its own spec and its own surface
  decision. Until then, the store is usable for the single-file case — a table committed once.
- **The crate is published with the others, and the publish act is still the user's.** The four crates still
  need an account and a token. **The two reasons this ADR gave for holding the store back were wrong**, and
  they are corrected in [ADR-0047](0047-the-stores-publish.md).
- **The commit's durability is still the open question spec 037 named.** The `fsync` measured on this host
  is APFS's **logical** flush, not `F_FULLFSYNC`, so the crash behaviour of the five steps is not yet proven
  anywhere.

## References

- [`constitution.md`](../../constitution.md) II — the method: measure first, and the figures are derived from
  the committed reports rather than typed.
- [`constitution.md`](../../constitution.md) V — the method's evidence rules: the medians of at least three
  campaigns, and every quoted figure agreeing with the committed report.
- [`constitution.md`](../../constitution.md) VII — explicit scope, no silent deferral.
- [`PERF.md`](../../PERF.md) §3s — the write side: the write, the commit, the bytes and the read-back.
- [`PERF.md`](../../PERF.md) §3t — the store's own write: the file it wrote, and the claim it makes.
- [`ADR-0043`](0043-the-write-side.md) — the decisions the write side's numbers make.
- [`spec 037`](../../specs/037-the-write-side/spec.md) — the write side, measured.
- [`spec 038`](../../specs/038-the-stores-half/spec.md) — the store's half.
- [`STATUS.md`](../../STATUS.md) — where the project's open questions live.
