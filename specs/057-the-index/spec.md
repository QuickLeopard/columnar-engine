# Spec 057 — The index

**Status**: planned
**Started**: 2026-11-18
**Predecessor**: [spec 056 — the streaming compaction](../056-the-streaming-compaction/spec.md)

## The problem

**The index's shape is decided by a figure, and the index is not built.** The rule is written down and it is the
database's own: *a point lookup that a scan already answers more cheaply is not given an index, and the database says so*
([ADR-0066](../../docs/adr/0066-the-databases-own-price.md)). **And the figure that decided the shape is published**
([PERF.md §3ak](../../PERF.md#3ak-the-databases-own-price)): the point lookup costs **43.319 ms** as a scan in the
engine's own read, the same lookup over **one file** — the floor an index would reach — costs **2.657 ms**, so the scan is
**17.20×** more expensive and **the most an index could save is 40.828 ms**. **And the gap is the data and not the files**:
the open of the table's sixteen files, with nothing read, is **0.461 ms** of the **43.319 ms** — **1.1%** — **so an index
does not win by avoiding the files' open, and it wins by not reading the data of the fifteen files it does not need.** The
newer campaigns agree ([§3al](../../PERF.md#3al-the-compaction-reads-a-file-at-a-time)): **47.529 ms** against **2.638 ms**,
**19.535×**, and **45.104 ms** to save.

**And what is missing is the index's own price.** ADR-0066 says it in its own words: *"the index's own cost — its build,
its storage and its lookup — is unmeasured, because there is no index to measure"*. **So the shape is decided and the
price is not**, and the price is the only thing that decides whether the index is worth having at all.

**And the database has no point lookup yet.** The probe measures the point lookup as a **scan with a filter** — the whole
table, read and filtered — so the lookup the rule talks about is not an API the database offers, it is a way of using the
one it has. **So this increment builds the lookup, and the map that serves it, and then prices them.**

**And there is more than one map to choose from, and they are not the same size.** The store writes its files with
Parquet's **chunk-level statistics on**
([`crates/columnar-store/src/writer.rs`](../../crates/columnar-store/src/writer.rs), `EnabledStatistics::Chunk`), so the
key column's own **minimum and maximum are already in the file's footer** — and the database writes on the grid
(**65,536** rows a file) while a row group holds **1,000,000** rows, so **a file is one row group and its footer holds
the key's range for that file**. **So there are three maps, and they differ by orders of magnitude**: the **file's own
statistics** (two numbers a file, and nothing to write), the **per-file key range the database keeps** (two numbers a
file, and a write's worth of work to keep), and **the per-key map** the figure decided (an entry a key, over the grid).

**And the cheap maps only narrow the files when the data's key is ordered — and this data's is not.** The published
lineitem file's own row-group statistics say so — **every row group's key range spans the whole key space**, and that is
readable out of the committed file's own metadata with any Parquet reader. **So the choice between the three maps is not
an opinion about which is elegant** — it is decided by what each one can narrow on the data the database actually writes,
**and the increment measures that on the table the database writes and publishes it.** **And an arm that cannot narrow is
not worth building.**

**And the shape was decided with the price missing.** ADR-0066 chose *"a map from the key to the file that holds it"*, and
it chose it **for the granularity** — the file, because that is where the floor was measured — **and not for the
encoding**. **So the encoding is what this increment measures**, and the price that was missing is the price that
decides it. **And if the answer is not the shape ADR-0066 decided, that sentence is corrected where it was published**,
with the correction recorded in the ADR.

## The requirements

### R1 — The lookup exists, and the schema names the key

- [x] The table's schema names **the key column** — one column, and the database refuses a schema that names a key
      column which **may hold a NULL**, because a key that may be absent cannot be looked up. **And the rules hold
      wherever a schema comes from**: a schema that is read out of a catalog is checked the same way a schema that is
      written is, **so a catalog that says two columns are keys, or that a key may hold a NULL, is refused rather than
      read** — and a catalog written before the key existed names no key, and is still read.
- [x] **And the database has a point lookup**: a table and a key, and **the rows that hold the key**. **And it is the
      database's own** — the snapshot's, so a lookup sees one version of the table, and the rows it answers with are the
      **live** ones.
- [x] **And the lookup's answer is the rows that hold the key, whichever map serves it** — and that is a **test**, and
      not a sentence in a document: the lookup and the plan that asks the same question answer the same rows, and a
      deleted row is not a row.

### R2 — The shape is **measured before it is built**

- [x] An experiment over **the table the database writes**: **the key's order**, **each file's key range**, **the number of
      distinct keys**, and **the number of files each of the maps would name** for a lookup.
      **The instrument is `crates/columnar-db/examples/index_probe.rs`**, and the facts are in
      [§3am](../../PERF.md#3am-the-indexs-shape-measured-before-it-is-built).
- [x] **And the maps are the ones the question has**: the **file's own statistics** — the range map, at the home the store
      already pays for — the **per-file key range the database keeps** — the range map, at the home a write pays for — and
      **the per-key map** the figure decided. **And the two range maps name the same files, because the file's own
      statistics are that range**: the experiment asks the shape's question once.
- [x] **And an arm the experiment refuses is not built**, and the refusal is published: **a map that cannot narrow the
      files on this data saves nothing, and a map that saves nothing is not built.** **And the range map is refused**: it
      names **all sixteen files**, for **every one** of the sixty-four sampled keys. **And the per-key map is the index**:
      **1.156** files on average, **1** at the smallest and **2** at the largest.

### R3 — The index's price is measured, **on every axis**

- [ ] The **write cost** — the time and the bytes a write pays to keep the index — **the storage** the index holds, **the
      peak memory** it takes, and **the lookup's own time**.
- [ ] **And the index's memory grows with the table's keys**, which is **the same shape of defect as the compaction's
      ceiling** — where the database's own memory was the table's size and the budget's verdict was the table's size
      ([spec 056](../056-the-streaming-compaction/spec.md)). **So the row count at which the index's own memory exceeds
      the engine's 256 MiB budget is measured**, if it does.
- [ ] **And the index's price is counted whole**: the write path maintains it, and an open rebuilds it, and **both are
      part of the price** — not one of them and not the other.

### R4 — The compaction and the index are **one thing**

- [ ] The compaction rewrites the files, so **the index is rewritten with them** — and **the compaction's cost with the
      index is measured against the compaction's cost without it**, back to back, on the same table, at the same moment.
- [ ] **And the index is measured on the compacted table too**, because a table that has been compacted is the table the
      index will live in most of the time.

### R5 — The rule is applied, and **the answer is published either way**

- [ ] The **break-even**: **the number of reads after which the index has paid for itself** — the index's whole price
      against the saving one lookup makes.
- [ ] **And if no map pays for itself, the database says so** — the lookup stays the scan, the index is not built, and
      **that sentence is published** with the figures that refused it.
- [ ] **And if the answer is not the shape ADR-0066 decided, that sentence is corrected where it was published** — in
      the ADR and in the status file — and the correction is recorded in the ADR.

### R6 — The record

- [ ] A **new section** in `PERF.md` — **§3am** — the decision in an **ADR** (**0068**), the defect table's row if the
      measurement finds one, and the crate's own README.
- [ ] **And `STATUS.md` moves** — the index's bullet, the counts, and the open question if the measurement opens one.
- [ ] **And the guard checks the published figures inside the section that publishes them.**

### R7 — The closure

- [ ] The spec's status is `done` with the date it finished, and CI is green.

## The scope

- **In**: the key column in the schema, the point lookup, the three maps' experiment, the map that survives it, the
  index's write cost, storage, memory and lookup, the compaction's cost with the index, the break-even, the rule's
  verdict, and the record.
- **Out**: **the engine's surface** — the index is the database's own, and the engine grows nothing: **the engine reads
  whole files**, and the index names files and not rows.
- **Out**: **the store's surface** — the store already writes the statistics and the grid, and neither changes. **And the
  four crates publish at 0.1.0** ([spec 054](../054-the-crates-are-published/spec.md)), so a change to a published surface
  is a break in every embedder's build.
- **Out**: **row-level reads** — the index names a file, because **the engine has no row-group skipping and the floor is
  its own read of one file** — **2.657 ms**, published with the index's answer as the bound.
- **Out**: **the tombstone tax's shape** — open question 7, three architectures, and the measurement does not pick
  between them. **An indexed lookup does not change the tax either**: it reads one file, and that file's retired
  versions still cost a predicate each.
- **Out**: **an automatic compaction** — nothing here triggers a compaction by itself, and the tax's shape decides that.
- **Out**: **the index's own durability across a restart** — the index is **derived from the files** (A3), so it is
  rebuilt at an open and the rebuild's price is measured. **Writing it** would be a second thing to keep consistent with
  the files, and that is a later increment's question.

## The instrument

**`crates/columnar-db/examples/index_cost_probe.rs`** — a new probe, beside the one that measured the scan: the lookup
through each map, the scan beside it, and the one-file floor beside that; the write's cost with and without the index;
the index's own bytes and its peak memory; the rebuild's cost at an open; and the break-even.

**And the committed reports** — three campaigns, interleaved, in `bench-results/`, and the figures derived from them by
`scripts/medians.py` and checked by `scripts/check-published-figures.py` **inside the section that publishes them**.

## The references

- [`crates/columnar-db/src/database.rs`](../../crates/columnar-db/src/database.rs) — the write path, the flush, and the
  compaction that rewrites the files.
- [`crates/columnar-db/src/snapshot.rs`](../../crates/columnar-db/src/snapshot.rs) — the snapshot, the table's files, and
  where the lookup belongs.
- [`crates/columnar-db/src/schema.rs`](../../crates/columnar-db/src/schema.rs) — the schema, and the nullability a key
  may not have.
- [`crates/columnar-store/src/writer.rs`](../../crates/columnar-store/src/writer.rs) — the grid, the row group, and the
  statistics the files already carry.
- [`crates/columnar-engine/src/memory.rs`](../../crates/columnar-engine/src/memory.rs) — the budget, and the row-group
  statistics the engine already uses **within** a file.
- [`crates/columnar-db/examples/db_cost_probe.rs`](../../crates/columnar-db/examples/db_cost_probe.rs) — the probe that
  measured the scan, the engine's own read, and the one-file floor.
- [PERF.md §3ak](../../PERF.md#3ak-the-databases-own-price) and
  [§3al](../../PERF.md#3al-the-compaction-reads-a-file-at-a-time) — the prices, and the rule's figures.
- [ADR-0066](../../docs/adr/0066-the-databases-own-price.md) — the rule, and the shape.
- [constitution.md](../../constitution.md), principle **IV — Evidence Over Claims**: the maps are measured before any of
  them is built, and the index's price is published after it is.
- [constitution.md](../../constitution.md), principle **VII — Explicit Scope, No Silent Deferral**: the engine's surface,
  the store's surface, the row-level reads, the tax's shape, the automatic compaction and the index's durability are
  **named** as out of scope, rather than left as omissions.

## The amendments

### A1 — The lookup is **the database's own**, and the schema names the key

**Why**: the rule is about a point lookup, and **the database has none** — the probe's point lookup is a scan with a
filter, which is a way of using the one API the database has. **So the lookup is built, and it is the database's own**, on
the snapshot, so that a lookup sees one version of the table and not a mixture.

**And the schema names the key**, because an index is over a key and the database cannot know which column is the key
without being told. **And a key column may not be nullable**, because a key that may be absent cannot be looked up — and
that is a rule the database enforces at the schema, and not a hope.

### A2 — The three maps are **measured before any of them is built**

**Why**: the house's rule is that a thing is not built before it is measured, and **the thing measured here is the map,
not the index**. ADR-0066 decided the granularity — the file — and left the encoding open, and **the price that was
missing is the price that decides the encoding**. **So the experiment comes first**: the key's order, each file's range,
the distinct keys, and **the number of files each map would name**. **And a map that cannot narrow the files on this
data saves nothing, and a map that saves nothing is not built** — and that is published, with the figures that said so.

**And the experiment says the range map is refused, and the per-key map is the index.** The key does not rise in the
table's rows — **499,701** descents over a million rows — **and so every one of the table's sixteen files holds a range
that spans nearly the whole key space**: the file that holds the smallest key of all holds a range from **13** to
**5,999,999**, and the one that holds the largest holds a range from **434** to **5,999,206**. **And a range that spans
the key space excludes nothing**: the range map names **all sixteen files**, for **every one** of the sixty-four sampled
keys — the mean is **16.000** and the smallest and the largest are **16** — **so the range map saves nothing, and it is
refused, in both of its homes.**

**And the per-key map narrows**: **1.156** files on average, **1** at the smallest and **2** at the largest — a key whose
rows fall in two files. **And the map holds an entry a key**: **921,129** distinct keys over a million rows, and
**78,871** rows that repeat a key. **So ADR-0066's shape is confirmed by a measurement and not by an assumption** — and
the shape that is built is the one that narrows.

**And the reason the range map fails is the data's order, and not the map**, **and that is published as a condition,
because it is one**: the published lineitem file's rows are in the file's order and not in the key's order, so **a range
cannot exclude a file on this data**. **On ordered data the range map would narrow, and it would be free** — the file's own
statistics are already written — **so the range map is not refused as a shape, and it is refused on this data, and the
figure that says so is published with it.**

### A3 — The index is **derived from the files**: the write maintains it, and an open rebuilds it

**Why**: the files are the database's truth — the manifest's truth about the files, the catalog's truth about the tables
— and **the index is derivable from them**, so **an index that is written is a second thing to keep consistent with the
files**, and that is exactly the kind of consistency that breaks silently. **So the index is derived**: the write path
maintains it as it writes, and an open rebuilds it from the files the snapshot names.

**And both prices are measured**, because both are paid: the write's cost to maintain it, and the open's cost to rebuild
it. **And the break-even counts both** — a map that costs more to build than it saves is not a map, and the number of
reads that says so is the number the record publishes.

### A4 — The granularity is the file, and **the bound is published**

**Why**: the engine reads whole files and has no row-group skipping, so **the floor an index can reach is the engine's own
read of one file** — **2.657 ms**, measured. **So the index names a file and not a row**, and **that bound is published
with the answer**, so that a reader who expects a row-level index knows exactly what was and was not built.
