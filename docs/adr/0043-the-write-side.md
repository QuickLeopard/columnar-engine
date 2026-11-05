# ADR-0043 — The write side

**Status**: accepted
**Date**: 2026-11-05
**Spec**: [037-the-write-side](../../specs/037-the-write-side/spec.md)

## Context

Every published figure in this repository is a figure about **reading**. The scan's decode, the join's probe,
the sort's heap, the drift's own parts — thirty-four specs of them. **Nothing in the tree writes a table.** The
one writer is `columnar-testdata`'s dataset generator: a benchmark fixture, `publish = false`, with a fixed
TPC-H schema and a scale factor as its only parameter. It is not a write path, and it was never measured as
one.

**And an embedded store is made of three things the engine does not have.** A **write path** — rows in, row
groups out. A **commit protocol** — a table is many files, and the set of them has to become visible
atomically. And a **catalog that survives the process** — `Catalog` is the embedder's, in memory, and its own
documentation says so: *"the engine never reads one from disk, never mutates one, and never outlives the borrow
the embedder gave it"*.

**None of the three had a measurement**, and this repository measures before it builds. The question was
therefore not "should there be a store" — that is answered elsewhere — it was **what the write side costs**,
because three numbers decide the store's shape: how long a row group takes to write against the read's own
cost, what a commit costs and above all what an `fsync` costs on this host, and what the write does to the
file's bytes and therefore to the read that follows.

**And the third one had a known shape already.** §3h measured that the encoding is a **measurement, not a
rule**: the dictionary pays **4.8–5.7×** on the two predicate columns and *loses* by **8%** on
`l_extendedprice`. That was measured on the **read** side. The **write** side *chooses* the encoding, and had
never been measured at all.

## Decision

1. **The write side is measured before the store is built.** The probe reads the published file's own rows
   once, writes them back out through the Parquet writer in the file's own row groups of a million rows with
   the file's own writer properties, commits the result the way an append-only store commits it, reads it
   back through the engine's own reader, and compares it with the original **byte for byte**. The write is
   timed without the read that fed it: the store's caller brings the rows, and the caller pays for bringing
   them. **No engine path is touched, and no write path is built here.**

2. **The store's files are Parquet, with the dictionary on.** The same rows written with the dictionary off
   take **1.55×** the bytes, cost **1.08×** the write time, and **read back 2.60× slower** — **1427.515 ms**
   against **548.445 ms** for the whole file. The dictionary wins on the read by more than it loses on the
   write, and §3h's lesson generalises: **the encoding is a measurement, and this is it.**

3. **The commit is a manifest commit of five steps, and nothing in its shape is forced by the flush here.**
   The data file is made durable first, then the manifest that names it is written, made durable, and
   `rename`d over the old one, and the directory is made durable last so the rename itself survives. The
   whole sequence is **7.651 ms** — **0.004** of the **1844.195 ms** write — and the `fsync`s are **0.910**
   of it. **No write-ahead log and no rollback** are needed to make that work, and neither is built.

4. **The row groups are a million rows** — the file's own — and one of them costs **306.230 ms** to write.

5. **The batch size is fixed at 65,536, and it is a property of the format's bytes, not of the data.** The
   Parquet writer splits **every batch it is handed** into mini-batches of `DEFAULT_WRITE_BATCH_SIZE` —
   **1,024** values — and the dictionary's fallback to plain is decided at one of those boundaries. **65,536 is
   64 × 1,024**, so the boundaries land on the same value indices every time: at that batch size the probe's
   file **is** the published file, **242,857,841** bytes, compared byte for byte and found equal. **125,000 is
   122 × 1,024 + 192**, so they drift, the fallback lands later, the dictionary page holds **131,685** values
   instead of **131,616**, and the file comes out **242,856,197** bytes — different **from byte 7**. **So the
   writer's output is a function of the caller's batch size**, and a store that promises reproducible files
   has to fix it.

6. **The `fsync` measured here is the platform's `fsync`, not a drive's flush, and the commit protocol has to
   be checked on the platform it ships on.** On APFS the two are different: `fsync` makes the file's data
   durable with respect to the operating system, while `F_FULLFSYNC` additionally flushes the drive's write
   cache. Rust's `File::sync_all` is the former. The **2.118 ms** above is the **logical** flush's cost, and
   the durability it buys on this platform is weaker than the number suggests.

7. **The write costs 3.36× a full read of the file and 24.04× a two-column read of it**, so *write once, read
   many* pays for itself after about **four** full reads or about **twenty-five** scans. **For a bulk-loaded
   analytical table that is read many times, it does** — and the read side of this repository is the half that
   is measured to a tenth of a millisecond.

## Consequences

- **The store's shape is decided by figures, and the figures are committed.** The row-group size, the commit's
  shape, the encoding policy and the batch size are four decisions, and each is a number in
  [§3s](../../PERF.md#3s-the-write-side) that the guard checks against the committed reports.
- **The batch-size finding is a constraint on the store's callers, and it is new.** The store's API takes
  rows in batches, and the batch size changes the bytes the store writes. **A store that promises reproducible
  files has to fix the batch size**, and the value that reproduces the published file is **65,536**.
- **The engine's read path is unchanged.** The probe is an instrument, and the write path it measures is not
  built in this increment. The store's code is the next increment, written against what this one measured.
- **The commit protocol is portable in shape and not yet proven in durability.** It is the simple one, and the
  simple one is affordable here — but the `fsync` on APFS is a logical flush, so the protocol's **crash**
  behaviour has to be checked on the platform it will ship on.
- **The next question is written down where this one was**: the README's Next row and `STATUS.md`'s last
  section. It is **the store's own code** — and, beside it, the work the repository cannot take on its own:
  **the library's publish**.

## References

- [`constitution.md`](../../constitution.md) II — the method: measure first, and the figures are derived from
  the committed reports rather than typed.
- [`constitution.md`](../../constitution.md) V — the method's evidence rules: the medians of at least three
  campaigns, and every quoted figure agreeing with the committed report.
- [`constitution.md`](../../constitution.md) VII — explicit scope, no silent deferral.
- [`PERF.md`](../../PERF.md) §3f — owning the Parquet decode is not worth it: the format is not the lever.
- [`PERF.md`](../../PERF.md) §3h — the materialised column, and the dictionary as a measurement rather than a
  rule.
- [`PERF.md`](../../PERF.md) §3s — the write side, the commit, the bytes and the read-back.
- [`ADR-0018`](0018-the-scan-reads-the-parquet-decode.md) — the scan reads the Parquet decode, and the rewrite
  that was refused.
- [`spec 037`](../../specs/037-the-write-side/spec.md) — the write side, measured.
- [`STATUS.md`](../../STATUS.md) — where the project's open questions live.
