# Spec 057 — The index — tasks

**Status**: planned
**Started**: 2026-11-18
**Spec**: [spec.md](spec.md)

The order is the house's: **the shape, then the price, then the record.** The shape comes first because ADR-0066 left the
encoding to a price that was never measured, and **a price that is not measured is not a number** — so the experiment
comes before the index, and an arm the experiment refuses is never built.

## The lookup

- [x] The table's schema names **the key column**, and **a key column that may hold a NULL is refused** — a key that may
      be absent cannot be looked up.
- [x] **And the rules hold wherever a schema comes from**: a catalog that says two columns are keys, or that a key may
      hold a NULL, is **refused rather than read** — and a catalog written before the key existed is still read.
- [x] **And the database has a point lookup**: a table and a key, and the rows that hold the key — on the snapshot, so a
      lookup sees one version of the table, and the rows it answers with are the **live** ones.
- [x] **And the lookup's answer is the rows that hold the key, whichever map serves it** — a test, and not a sentence:
      the lookup and the plan that asks the same question answer the same rows, and a deleted row is not a row.

## The experiment — the shape measured before it is built

- [x] The experiment measures, over **the table the database writes**: **the key's order**, **each file's key range**, **the
      number of distinct keys**, and **the number of files each of the maps would name** for a lookup.
- [x] **And the maps are measured**: the **file's own statistics**, the **per-file key range the database keeps**, and
      **the per-key map** the figure decided. **And the two range maps name the same files, because the file's own
      statistics are that range.**
- [x] **And an arm the experiment refuses is not built**, and the refusal is published with the figures that said so:
      **the range map is refused** — it names **all sixteen files**, for **every one** of the sixty-four sampled keys —
      **and the per-key map is the index**: **1.156** files on average, **1** at the smallest and **2** at the largest.
- [x] **And the reason is the data's order**: **499,701** descents over a million rows, so **every one of the sixteen
      files holds a range that spans nearly the whole key space** — and the smallest of the sixteen minimums is **13** and
      the largest of the sixteen maximums is **5,999,999**. **And that is published as a condition, because it is one**:
      on ordered data the range map would narrow, and it would be free.
- [x] **And the map's own size is measured**: **921,129** distinct keys over a million rows, and **78,871** rows that
      repeat a key — so the index holds **an entry a key**, and a key's entry can name **two** files.
- [x] **Three campaigns**, with the raw reports **committed** to `bench-results/`, and the figures derived by
      `scripts/medians.py`.
- [x] **And the section publishes them** — **§3am** — **and the guard checks every one of them inside it**.

## The index — the price, on every axis

- [x] The map that survives the experiment is built, and **it is the snapshot's**: the write path maintains nothing, and
      the reader pays — once, at the snapshot's first lookup — because **a snapshot pins an older file set, and an index
      the writer maintained would name a file the snapshot does not have**.
- [x] **And its price is measured whole**: the build's cost, the storage it holds, its peak memory, and the lookup's own
      time — back to back, on the same table, at the same moment — **and the write's cost, which is nothing**: **27.333 ms**
      with no index alive and **29.122 ms** with one, **and the difference is inside the spread**.
- [x] **And the row count at which the index's own memory exceeds the engine's 256 MiB budget is measured**, if it does:
      the index's memory grows with the table's keys, and that is the same shape of defect as the compaction's ceiling.
      **And it does** — the index holds **126.1 MiB** over a million rows and passes the budget between **2,379,393** and
      **2,381,835** rows.
- [x] **And the compaction is measured with the index and without it**, because the compaction rewrites the files and the
      index is rewritten with them. **And the compaction's cost does not move** — **285.976 ms** against **298.622 ms**, **and
      the difference is inside the spread** — **and the compacted table's own lookups are measured too**, where the answer is
      that **a compacted table is one file and the index cannot narrow it**.

## The verdict

- [x] The **break-even**: the number of reads after which the index has paid for itself — the index's whole price against
      the saving one lookup makes. **And it is 2.935 lookups.**
- [x] **And the rule is applied**: the lookup is given the map that pays for itself, **or it stays the scan and the
      database says so** — and that sentence is published with the figures that refused it. **And on a compacted table no map
      pays for itself**, so **no index is built** and the lookup is the scan — **43.281 ms**, where it was **181.172 ms**.
- [x] **And if the answer is not the shape ADR-0066 decided, that sentence is corrected where it was published**, and the
      correction is recorded in the ADR. **And the answer is the shape ADR-0066 decided**, so nothing is corrected.

## The prices

- [x] `crates/columnar-db/examples/index_probe.rs` — the scan beside the lookup, the one-file floor beside that, the
      write's cost with and without the index, the compaction's cost in the same two arms, and the compacted table's three
      lookups.
- [x] `crates/columnar-db/examples/index_memory_probe.rs` — the index's own bytes, its peak, and the row count at which it
      passes the engine's budget — **with a counting allocator in it, and no time measured beside it**.
- [x] **Three campaigns**, interleaved, with the raw reports **committed** to `bench-results/`, and the figures derived by
      `scripts/medians.py` — **and the set that measured the defect is committed too**, taken back to back with the arms
      interleaved, so the machine's movement cannot land on one side of the difference.

## The record

- [x] A **new section** in `PERF.md` — **§3am** for the shape, **§3an** for the price and **§3ao** for the memory — the
      decision in an **ADR** (**0068** and **0069**), **the defect table's row** the measurement found, and the crate's own
      README.
- [x] **And `STATUS.md` moves** — the index's bullet, the counts, and **two** open questions the measurement opened.
- [x] **And the guard checks the published figures inside the section that publishes them.**

## The closure

- [x] The spec's status is `done` with the date it finished, and CI is green.

## What is not done

- **The engine's surface** — the index is the database's own, and the engine grows nothing: the engine reads whole files.
- **The store's surface** — the store already writes the statistics and the grid, and neither changes. **And the four
  crates publish at 0.1.0**, so a change to a published surface is a break in every embedder's build.
- **Row-level reads** — the index names a file, because the engine has no row-group skipping and the floor is its own read
  of one file. The bound is published with the answer.
- **The tombstone tax's shape** — open question 7, three architectures, and the measurement does not pick between them. An
  indexed lookup does not change the tax either.
- **An automatic compaction** — nothing here triggers a compaction by itself, and the tax's shape decides that.
- **The index's own durability across a restart** — the index is derived from the files, so it is built at the snapshot's
  first lookup and its price is the build's price. Writing it is a later increment's question.
- **The index's memory is not refused** — it passes the engine's 256 MiB budget between 2,379,393 and 2,381,835 rows, and
  nothing refuses it. That is an open question, and the figure that asks it is published.
- **The cheaper encodings are not measured** — the build costs 125.889 ms against the 45.689 ms scan it replaces, and the map
  holds a `Vec` a key. That is an open question too.
