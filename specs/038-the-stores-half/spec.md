# Spec 038 — The store's half

**Status**: done
**Started**: 2026-11-05
**Finished**: 2026-11-05

## The problem

**The engine is a read-only query engine over Parquet, and an embedded store is made of three things it
does not have.** A **write path** — rows in, row groups out. A **commit protocol** — a table is many files,
and the set of them has to become visible atomically. And a **catalog that survives the process** —
`Catalog` is the embedder's, in memory, and its own documentation says so: *"the engine never reads one from
disk, never mutates one, and never outlives the borrow the embedder gave it"* (`crates/columnar-engine/src/facade.rs:485`).

**Spec 037 measured the first two and decided their shape** ([ADR-0043](../../docs/adr/0043-the-write-side.md)):
the write of the published file is **1844.195 ms**, a row group costs **306.230 ms**, the **commit** is the
five steps — the data file durable, the manifest written and durable, the `rename`, the directory durable —
and it is **7.651 ms**, which is **0.004** of the write. **So the protocol's shape is not constrained by the
flush**, and the store needs no write-ahead log and no rollback to be affordable. The encoding keeps the
**dictionary** on, because the plain file takes **1.55×** the bytes and reads back **2.60×** slower.

**And the third thing does not exist at all.** There is no catalog on disk, and there is no writer.

**And the store's half cannot be read by the engine yet.** A store's table is **many files** — one per
commit, append-only — and the engine's only source is `ParquetSource`, which is **one file**;
`open_table` resolves a name to one path. **So the read side of a multi-file table is the engine's own next
increment**, and this one builds the store's half: the writer, the manifest, the commit and the catalog.

**And the acceptance is the strongest one available.** Fed the published file's own rows in batches of
**65,536** — the grid [ADR-0043](../../docs/adr/0043-the-write-side.md) fixed, because the writer's output is
a function of it — the store writes **the published file, byte for byte**. **So the store's correctness claim
is not "it wrote something" but "it wrote the file the generator wrote, and the two are equal"**, checked
byte for byte against a committed figure.

## The requirements

### R1 — The store is a crate, and it is below the engine

`columnar-store` depends on `columnar-core`, `parquet`, `arrow`, `serde`, `serde_json` and `thiserror` — and
**not** on `columnar-engine`. **The engine's read path is untouched**, and its published figures do not move.
The store writes files; the engine reads files; the glue between them is a path.

### R2 — A table is a set of files, and the manifest names them

A commit appends a **new** file and **never modifies an old one**. The manifest is the table's truth: its
files, their row counts, their bytes, and the table's own totals. It is JSON, it is small, and it is the one
thing a reader has to believe.

### R3 — The commit is the five steps that were measured

The data file is made durable **first**, then the manifest that names it is written, made durable, and
`rename`d over the old one, and the directory is made durable **last**, so the rename itself survives. **A
reader that sees the new manifest sees the new data; a reader that sees the old one sees the old data.**
There is no third answer.

### R4 — The catalog survives the process

The manifest is on disk, it is read back when the store is opened, and the tables it names are the store's
tables. **The engine's in-memory `Catalog` stays the embedder's** — the store does not touch it, and does not
need it.

### R5 — The writer's grid is the store's, not the caller's

**65,536 rows**, the grid that reproduces the published file. The store checks the grid and says so when the
caller breaks it, because **the bytes depend on it**: the Parquet writer cuts every batch it is handed into
mini-batches of 1,024, and the dictionary's fallback to plain is decided at one of those boundaries. **A file
that comes out differently is not a bug in the store; it is the caller batching differently.** The store says
so, in an error, rather than writing a file whose bytes nobody can predict.

### R6 — The store reproduces the published file

Fed the published file's own rows in 65,536-row batches, the store writes the published file, **byte for
byte**. The acceptance is checked against a **committed figure**, and the guard checks that figure in the
section that publishes it.

### R7 — The store introduces no new numbers

The store's write is the write spec 037 measured: the same writer properties, the same row groups, the same
commit. **The store uses the figures; it does not publish new ones.** The one new figure is the **identity
verdict** — the file the store wrote, and the byte count that says so.

### R8 — The scope is named

**No SQL. No transactions with rollback. No MVCC, no snapshot isolation. No concurrent writers. No
row-level updates. No compaction. One writer.** A table grows by appending a file and committing it; it does
not shrink, and it does not change in place. **The scope is in this spec so that it is a decision, not an
oversight.**

## The plan's shape

Five phases, and the edges between them:

1. **The crate** — the manifest, the catalog, the writer and the commit, with the grid rule and the errors
   that carry it.
2. **The verification** — the store writes the published file's own rows, and the file it wrote is compared
   with the published file **byte for byte**; the commit is exercised, and the manifest is read back after
   every step.
3. **The publication** — the crate's own README, the section that publishes the store's file, the guard's
   check, and the embedding guide's new section.
4. **The ADR and the questions** — the ADR records the store's shape, and the next question is written down
   where the last one was.
5. **The closure** — the spec's status is `done` with the date it finished, and CI is green on the publish
   commit.

The edges: 1 → 2 → 3 → 4 → 5, and 2 → 3 (the published figure comes from the store's own run).

## The scope

| | |
|---|---|
| The engine's own work | **This increment changes none of it.** The store does not depend on `columnar-engine`, so the engine's read path cannot move. |
| The write path | **Built.** This is the increment: the writer, the manifest, the commit and the catalog. |
| The read path | **Not built.** The engine reads one file; a store's table is many files, and the source that spans them is the **next** increment, with its own spec. |
| The published set | **The standing set does not slide.** The store's figures are the ones spec 037 published, and the one new figure is the identity verdict. |
| The library's publish | **Not this increment's.** It is a separate act, with its own account, and it is written down as an open question. |
| The `parquet` bump | **Not this increment's.** It is written down as an open question, and it moves the published figures. |

## The measurement

1. **Three campaigns of the store's own probe**, on this machine, the raw reports committed under their own
   names. The medians are of three campaigns, and the spread is published.
2. **The host's state is published** with every campaign — the load average — because the answer depends on
   the host's state on the day, and spec 036 is the reason that is not optional.
3. **The identity is exact.** The two files are compared byte for byte, and the verdict is what the comparison
   says. **There is no spread in a boolean**, and the guard checks the byte count and the verdict in the
   section that publishes them.

## The references

| | |
|---|---|
| [`constitution.md`](../../constitution.md) II | the method: measure first, and the figures are derived from the committed reports |
| [`constitution.md`](../../constitution.md) V | the method's evidence rules: the medians of at least three campaigns, and every quoted figure agreeing with the committed report |
| [`constitution.md`](../../constitution.md) VII | explicit scope, no silent deferral |
| [`PERF.md`](../../PERF.md) §3h | the dictionary is a measurement, not a rule |
| [`PERF.md`](../../PERF.md) §3s | the write side: the write, the commit, the bytes and the read-back |
| [`ADR-0043`](../../docs/adr/0043-the-write-side.md) | the decisions the write side's numbers make |
| [`spec 037`](../037-the-write-side/spec.md) | the write side, measured |
| [`STATUS.md`](../../STATUS.md) | where the project's open questions live |
