# Spec 056 — The streaming compaction — tasks

**Status**: planned
**Started**: 2026-11-18
**Spec**: [spec.md](spec.md)

The order is the house's: **the shape, then the price, then the record.** The shape comes first because the ceiling is a
defect that is already published, and a defect that is published is a defect that is waiting to be fixed.

## The streaming compaction

- [ ] The compaction reads **one file at a time** — one query plan a file — so the engine's estimate is a fraction of the
      table's and **the budget stops being the table's ceiling**.
- [ ] **And the database's own memory is bounded by one file's live rows**: the rows in flight are the file's, and the
      batches are flushed as they fill.
- [ ] **And it writes exactly one file** — the manifest ends up naming one file and nothing else, the retired versions stop
      existing, and the table's versions are renumbered from zero.
- [ ] **And the engine's surface is not widened**, and **the budget is not raised**: a file set of one is a file set, and the
      streaming compaction fits inside **256 MiB** by reading less at a time.

## The output schema

- [ ] Every file the database writes — **the append path and the compaction alike** — takes its Arrow schema from **the
      table's schema**: nullable is optional, not nullable is required, and the hidden `row_id` and `deleted` columns are
      plain and never NULL.
- [ ] **And the store's row-writing API grows the nullability as an explicit input** — the caller decides, and the store does
      not derive it from the data — **and the store's surface change is recorded**, because the store publishes at **0.1.0**.
- [ ] **And the bytes change, and the change is published**: the old figure and the new one are both given, with the reason
      named.

## The refusal

- [ ] The compaction checks each file's schema against **the table's schema** — **the nullability included**, which the
      engine's own file-set check does not compare.
- [ ] **And a file that disagrees is refused, and the refusal names the file and the column.**

## The prices

- [ ] `crates/columnar-db/examples/db_cost_probe.rs` — the streaming compaction's cost a row, **clean and taxed**, beside the
      one-shot compaction's cost, **back to back on the same table at the same moment**.
- [ ] **And the ceiling is measured again** — the row count at which the compaction is refused, **before and after** — and
      **the table the probe already refuses at 6,000,000 rows is the table the streaming compaction has to compact**.
- [ ] **And the tax's total is measured**: streaming does not change the tax's total work and only its peak, **and the
      measurement says whether it does**.
- [ ] **Three campaigns**, interleaved, with the raw reports **committed** to `bench-results/`, and the figures derived by
      `scripts/medians.py`.

## The record

- [ ] A **new section** in `PERF.md`, the decision in an **ADR**, the defect table's row, and the crate's own README.
- [ ] **And the guard checks the published figures inside the section that publishes them.**
- [ ] **And `STATUS.md`'s open question 8 moves** — the compaction's ceiling is answered, and what the answer cost is
      published.

## The closure

- [ ] The spec's status is `done` with the date it finished, and CI is green.

## What is not done

- **The tombstone tax's shape** — streaming does not change the tax a read pays; it only makes the compaction able to run at
  all. The three ways out are three architectures, and the measurement does not pick between them.
- **The index** — its shape is decided by the scan's figure, and its write cost is not measured, because there is no index to
  measure.
- **An automatic compaction** — nothing here triggers a compaction by itself, and the tax's shape decides that.
- **The engine's file-set check** — it does not compare nullability, and after the schema is decided the database's own files
  agree by construction. Whether the engine's check should compare it is a later increment's decision.
- **The budget** — it stays at **256 MiB**, and the estimate is not changed. A budget that is raised to fit a query is a
  budget that no longer says anything.
- **A second output file** — the compaction writes one file, because that is what it promises.
- **The publish** — spec 054 publishes the four crates, and this increment settles the store's surface **before** that
  publish, so that the surface is settled when it is frozen.
