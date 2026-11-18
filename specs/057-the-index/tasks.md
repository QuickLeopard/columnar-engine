# Spec 057 — The index — tasks

**Status**: planned
**Started**: 2026-11-18
**Spec**: [spec.md](spec.md)

The order is the house's: **the shape, then the price, then the record.** The shape comes first because ADR-0066 left the
encoding to a price that was never measured, and **a price that is not measured is not a number** — so the experiment
comes before the index, and an arm the experiment refuses is never built.

## The lookup

- [ ] The table's schema names **the key column**, and **a key column that may hold a NULL is refused** — a key that may
      be absent cannot be looked up.
- [ ] **And the database has a point lookup**: a table and a key, and the rows that hold the key — on the snapshot, so a
      lookup sees one version of the table.
- [ ] **And the lookup's answer is the rows that hold the key, whichever map serves it** — a test, and not a sentence.

## The experiment — the shape measured before it is built

- [ ] The experiment measures, over **the table the database writes**: **the key's order**, **each file's key range**, **the
      number of distinct keys**, and **the number of files each of the three maps would name** for a lookup.
- [ ] **And the three maps are measured**: the **file's own statistics**, the **per-file key range the database keeps**, and
      **the per-key map** the figure decided.
- [ ] **And an arm the experiment refuses is not built**, and the refusal is published with the figures that said so.

## The index — the price, on every axis

- [ ] The map that survives the experiment is built, **maintained by the write path** and **rebuilt at an open**, because it
      is derived from the files.
- [ ] **And its price is measured whole**: the write's cost to maintain it, the storage it holds, its peak memory, the
      rebuild's cost at an open, and the lookup's own time — back to back, on the same table, at the same moment.
- [ ] **And the row count at which the index's own memory exceeds the engine's 256 MiB budget is measured**, if it does:
      the index's memory grows with the table's keys, and that is the same shape of defect as the compaction's ceiling.
- [ ] **And the compaction is measured with the index and without it**, because the compaction rewrites the files and the
      index is rewritten with them.

## The verdict

- [ ] The **break-even**: the number of reads after which the index has paid for itself — the index's whole price against
      the saving one lookup makes.
- [ ] **And the rule is applied**: the lookup is given the map that pays for itself, **or it stays the scan and the
      database says so** — and that sentence is published with the figures that refused it.
- [ ] **And if the answer is not the shape ADR-0066 decided, that sentence is corrected where it was published**, and the
      correction is recorded in the ADR.

## The prices

- [ ] `crates/columnar-db/examples/index_cost_probe.rs` — the lookup through each map, the scan beside it, the one-file
      floor beside that, the write's cost with and without the index, the index's bytes and peak memory, the rebuild's
      cost, and the break-even.
- [ ] **Three campaigns**, interleaved, with the raw reports **committed** to `bench-results/`, and the figures derived by
      `scripts/medians.py`.

## The record

- [ ] A **new section** in `PERF.md` — **§3am** — the decision in an **ADR** (**0068**), the defect table's row if the
      measurement finds one, and the crate's own README.
- [ ] **And `STATUS.md` moves** — the index's bullet, the counts, and the open question if the measurement opens one.
- [ ] **And the guard checks the published figures inside the section that publishes them.**

## The closure

- [ ] The spec's status is `done` with the date it finished, and CI is green.

## What is not done

- **The engine's surface** — the index is the database's own, and the engine grows nothing: the engine reads whole files.
- **The store's surface** — the store already writes the statistics and the grid, and neither changes. **And the four
  crates publish at 0.1.0**, so a change to a published surface is a break in every embedder's build.
- **Row-level reads** — the index names a file, because the engine has no row-group skipping and the floor is its own read
  of one file. The bound is published with the answer.
- **The tombstone tax's shape** — open question 7, three architectures, and the measurement does not pick between them. An
  indexed lookup does not change the tax either.
- **An automatic compaction** — nothing here triggers a compaction by itself, and the tax's shape decides that.
- **The index's own durability across a restart** — the index is derived from the files, so it is rebuilt at an open and
  the rebuild's price is measured. Writing it is a later increment's question.
