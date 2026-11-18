# Spec 056 — The streaming compaction — tasks

**Status**: planned
**Started**: 2026-11-18
**Spec**: [spec.md](spec.md)

The order is the house's: **the shape, then the price, then the record.** The shape comes first because the ceiling is a
defect that is already published, and a defect that is published is a defect that is waiting to be fixed.

## The streaming compaction

- [x] The compaction reads **one file at a time** — one query plan a file — so the engine's estimate is a fraction of the
      table's and **the budget stops being the table's ceiling**.
- [x] **And the database's own memory is bounded by one file's live rows**: the rows in flight are the file's, and the
      batches are flushed as they fill.
- [x] **And it writes exactly one file** — the manifest ends up naming one file and nothing else, the retired versions stop
      existing, and the table's versions are renumbered from zero.
- [x] **And the engine's surface is not widened**, and **the budget is not raised**: a file set of one is a file set, and the
      streaming compaction fits inside **256 MiB** by reading less at a time.

## The output schema

- [x] Every file the database writes — **the append path and the compaction alike** — takes its Arrow schema from **the
      database's own rule**: the application's columns are optional, because a tombstone holds no value of the
      application's, and the hidden `row_id` and `deleted` columns are required, because they never hold one.
- [x] **And the store's row-writing API grows the nullability as an explicit input** — the caller decides, and the store does
      not derive it from the data — **and the store's surface change is recorded**, because the store publishes at **0.1.0**.
- [x] **And the bytes change, and the change is published**: the old figure and the new one are both given, with the reason
      named — **2,091 bytes** and the space amplification did not move at all.

## The output and the disagreement

- [x] The compaction writes **the database's own rule**, so the table's files agree **by construction** — and the
      compaction **repairs** the files written before the rule.
- [x] **And the engine's file-set check does not grow a nullability comparison**: a file set whose files disagree about a
      column's optionality **reads correctly**, and the check's silence is published with the test that proves it
      (`crates/columnar-engine/tests/nullability.rs`).
- [x] **And the database does not learn to read a file's nullability**, because it does not need to.

## The prices

- [x] `crates/columnar-db/examples/db_cost_probe.rs` — the streaming compaction's cost a row, **clean and taxed**, and the
      old figure and the new one side by side, with the machine's own movement named.
- [x] **And the ceiling is measured again** — the row count at which the compaction is refused, **before and after** — and
      **the table the probe refused at 6,000,000 rows is the table the streaming compaction compacts**: **6,000,000**
      compacted and **0** refused.
- [x] **And the tax's total is measured**: streaming does not change the tax's total work and only its peak, **and the
      measurement says so** — **14,328** ns a row where it was **13,088**, and the machine's own drift accounts for it.
- [x] **Three campaigns**, interleaved, with the raw reports **committed** to `bench-results/`, and the figures derived by
      `scripts/medians.py`.

## The record

- [x] A **new section** in `PERF.md` — **§3al** — the decision in an **ADR** (**0067**), the defect table's two rows, and
      the crate's own README.
- [x] **And the guard checks the published figures inside the section that publishes them.**
- [x] **And `STATUS.md`'s open question 8 moves** — the compaction's ceiling is answered, and what the answer cost is
      published.

## The closure

- [ ] The spec's status is `done` with the date it finished, and CI is green.

## What is not done

- **The tombstone tax's shape** — streaming does not change the tax a read pays; it only makes the compaction able to run at
  all. The three ways out are three architectures, and the measurement does not pick between them.
- **The index** — its shape is decided by the scan's figure, and its write cost is not measured, because there is no index to
  measure.
- **An automatic compaction** — nothing here triggers a compaction by itself, and the tax's shape decides that.
- **The engine's file-set check** — it does not compare nullability, and it does not have to: the engine's vocabulary has
  no nullability, and a file set whose files disagree about it reads correctly. Whether the engine's check should compare it
  anyway is a later increment's decision.
- **The budget** — it stays at **256 MiB**, and the estimate is not changed. A budget that is raised to fit a query is a
  budget that no longer says anything.
- **A second output file** — the compaction writes one file, because that is what it promises.
- **The publish** — spec 054 publishes the four crates, and this increment settles the store's surface **before** that
  publish, so that the surface is settled when it is frozen.
