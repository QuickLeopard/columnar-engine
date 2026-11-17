# Spec 055 — The embedded database — tasks

**Status**: planned
**Started**: 2026-11-17
**Spec**: [spec.md](spec.md)

The order is the house's: **the shape, then the price, then the one part the price decides, then the record.** The shape
comes first because the probe measures the database's own parts, and a probe that measures a shape that does not exist
measures nothing.

## The crate

- [x] `columnar-db` is a workspace member, and it depends on `columnar-engine` and `columnar-store` — **and on nothing
      else in this workspace**.
- [x] **And the four crates are not changed to make it work.** A change to any of them is a change to a published
      surface: it is made only if a defect is found, and then it is published.
- [x] **And its public surface is small and it is named** — the crate's front page lists the names, the way the engine's
      does.

## The catalog

- [x] A table's **name**, **schema** and **path** live on disk, and the catalog is replaced **atomically** — written to a
      temporary file and renamed over the old one.
- [x] **And a reopened database sees the same tables**, with the same schemas.

## The write path

- [x] A row is **typed values** — the engine's own types, and a nullability per column.
- [x] **And the rows land on the store's grid** — every batch but the last is **65,536** rows — and a write that cannot be
      on the grid is **refused**.
- [x] **And the commit is the store's five steps**: no write-ahead log, no rollback, no second recovery path.

## The read path

- [x] A **snapshot** pins the manifest it read, and the file list that manifest names is the table for the life of the
      snapshot.
- [x] **And a reader sees the old file set or the new one, never a mixture.**
- [x] **And the query language is the typed plan** — the engine's `QueryPlan`, `PlanBuilder` and `ColumnExpr`, with the
      engine's surface **not widened**.
- [x] **And the database's values are the engine's `ScalarValue`.**

## The rows that change

- [ ] Every table carries a hidden **`row_id`** column.
- [ ] **An update is an append** of the new version and a **tombstone** of the old; **a delete is a tombstone**.
- [ ] **And compaction** rewrites the live rows into a new file and drops the dead ones.

## The prices

- [ ] `crates/columnar-db/examples/db_cost_probe.rs` — the probe: the catalog's open, the write on the grid, the snapshot
      over one file and over many, the scan of a point lookup, the compaction, and the space amplification.
- [ ] **Three campaigns**, interleaved, with the raw reports **committed** to `bench-results/`.
- [ ] **And the figures are derived** by `scripts/medians.py`, and the **index's shape is decided by the scan's figure**.
- [ ] **And a point lookup that a scan already answers more cheaply is not given an index**, and the database says so.

## The record

- [ ] A **new section** in `PERF.md`, the decisions in an **ADR** — the storage model, the query language, the index's
      shape — and **STATUS.md**'s bullet moves.
- [ ] **And the guard checks the published figures inside the section that publishes them.**

## The closure

- [ ] The spec's status is `done` with the date it finished, and CI is green.

## What is not done

- **SQL** — the query language is the typed plan, and the decision is recorded in the ADR so that it is not asked again.
  A front end is a later increment over the same plan.
- **The publish of `columnar-db`** — spec 054 publishes the four; this crate publishes when its own surface is frozen.
- **A transaction over more than one append** — this increment's transaction is one append, and the store's commit is one
  file and one rename.
- **Blobs** — a blob is a file and a pointer, and it is its own increment.
- **Encryption, a general optimizer, and a second writer** — each is named in the spec's scope, and each would change the
  store's shape.
