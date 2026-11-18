# columnar-db

An **embedded database** for the columnar engine: a persistent catalog, a typed write path, and a snapshot read.

The engine reads Parquet and the store writes it. This crate is the layer between them: it says which files make up
which table, it gives a row an identity, and it hands a read a snapshot.

## The shape

- A **database** is a **directory**. `catalog.json` names the tables, and `tables/` holds them — each one a **store**
  table, so each one a set of Parquet files with a manifest that names them.
- The **catalog** is replaced by a **rename**, the same primitive the store's commit uses, so a reader sees the old
  catalog or the new one and never a half-written one.
- A **table** is a **set of files**, and the engine reads a set of files as one table — `FileSetSource` is the engine's
  own name for it.

## Using it

```rust
use columnar_db::prelude::*;
use columnar_engine::ScalarValue;

let mut database = Database::open("/tmp/warehouse")?;
let schema = Schema::new(vec![
    Column::new("id", DataType::Int64, false),
    Column::new("name", DataType::Utf8, true),
])?;

let mut table = database.create_table("lineitem", schema)?;
table.insert(&[Row::filled(vec![
    ScalarValue::Int64(1),
    ScalarValue::Utf8("first".to_owned()),
])])?;
table.commit()?;

assert_eq!(table.name(), "lineitem");
assert_eq!(table.schema().len(), 2);
# Ok::<(), columnar_db::Error>(())
```

The catalog is **written when the table is created**, and it is JSON: small, readable, and diffable by a human when
something has gone wrong.

## The write path

**The rows are checked against the schema before they are buffered**: a row with the wrong number of values, a value of
the wrong type, or a `NULL` in a column that may not hold one, is refused and never reaches the buffer.

**And the rows are grouped.** The store's `append` writes one file per call, so a database that committed every insert
would grow one file per insert, and every read would open every one of them. **So the buffer is written when it reaches
the grid** — 65,536 rows, the store's own grid — and the file count then grows with the data rather than with the number
of inserts.

**And a commit is the store's five steps**: the file made durable, the manifest written, made durable and renamed over
the old one, the directory made durable. There is no write-ahead log, and the database adds none.

## The read path

**A snapshot pins the manifests it read**, so the file list each manifest named is that table for the life of the
snapshot — and a write that lands afterwards is a write the snapshot does not see. **A reader sees the old file set or
the new one, never a mixture.**

**And the query language is the engine's.** The plan is built with the engine's own `PlanBuilder`, over the engine's own
expressions, and the engine's surface is not widened to serve the database.

**And a table is a set of files, so the database resolves a join's build side to the whole table.** The engine's builder
resolves a name to one path; the engine's join takes a list of paths; so the database fills the list in — with the
snapshot's pinned files.

```rust
let snapshot = database.snapshot()?;
let table = snapshot.table("lineitem").expect("the table is there");
let plan = table.plan().filter(col("id").gt(1)).projection(["id"]).build()?;
let rows = table.rows(&plan, &ExecOptions::default())?;
```

**And the rows come back in the engine's own values** — a `ScalarValue` per column, a NULL being the absence of one.

## The rows that change

**Every row carries a hidden `row_id` and a hidden `deleted` flag**, and the application names neither: a schema may not use
either name, and a plan that names one is refused. The row's identity is the database's bookkeeping, and the application's
rows are the values and not the bookkeeping.

**And a row that changes does not change in place.** The files are append-only, so an update cannot rewrite the version it
replaces. **So the replacement is a tombstone** — a row that names the version it retires — and the new version is a new row
with a new id. **The tombstone is in the same file as the new version**, so the two land together: one file, one manifest, one
rename, and a reader sees the old version and the new one or neither.

```rust
table.update(col("id").eq(2), Row::filled(vec![
    ScalarValue::Int64(99),
    ScalarValue::Utf8("new".to_owned()),
]))?;
table.delete(col("id").eq(3))?;
```

**And the ids are read from what is on disk, never written down.** The id a new version gets is the number of versions the
table has ever had, and that number is the manifest's rows less the retired ones — so there is no counter to lose, and no
window in which two rows could share an id.

**And the compaction pays the tombstone tax back.** The tax is one predicate per retired version — the read filters on
`deleted = 0` and on `row_id != <retired id>` for each retired id — and the compaction is the rewriting that makes the retired
versions stop existing: the live rows are read, written into a new file, and the manifest is replaced by one that names it and
nothing else.

**And the compaction reads a file at a time, so the table's size is not its ceiling.** The live rows of each file come back
in that file's own plan and are written as they arrive, so the rows in flight are a file's rows — and the engine's memory
budget's verdict is about a file's rows and not about the table's. **6,000,000** rows compacted and **0** refused, where
**5,375,000** compacted and **6,000,000** were refused. The price is in
[`PERF.md` §3al](../../PERF.md#3al-the-compaction-reads-a-file-at-a-time), and the decision is in
[ADR-0067](../../docs/adr/0067-the-compaction-reads-a-file-at-a-time.md).

**And the table can be looked up by its key.** The schema names one of its columns the key — **and a key that may hold a
`NULL` is refused**, because a key that may be absent cannot be looked up — and the snapshot answers a point lookup with
the rows that hold it. **And the lookup is a scan today**: the rows are found by filtering the table, and the map that
will make it cheap was **measured before it was built** — the per-key map, and not the free one, because the free one
names every file on this data. The price is in
[`PERF.md` §3am](../../PERF.md#3am-the-indexs-shape-measured-before-it-is-built), and the decision is in
[ADR-0068](../../docs/adr/0068-the-indexs-shape-and-its-price.md).

**And the tax has a price, and the price is the retired versions and the table's size together.** The engine's pushdown
runs each comparison on a column as one pass, so the tax is one pass over the id column for every retired version:
**0.119 ns** a retired version a row, measured on a million rows and again on a table fifteen times smaller, where it is
**0.132 ns** — where it was **0.115 ns** and **0.120 ns** when spec 055 measured it, and **the difference is the
machine's**, which was busier in the second set of campaigns. **And every read pays it, and not only the compaction**: a
table of a million rows read with the live filter alone in **44.401 ms** and with the full live filter in
**13,942.178 ms**, where it was **44.637 ms** and **13,515.470 ms**. The prices are in
[`PERF.md` §3ak](../../PERF.md#3ak-the-databases-own-price) and
[`PERF.md` §3al](../../PERF.md#3al-the-compaction-reads-a-file-at-a-time), and the decisions are in
[ADR-0066](../../docs/adr/0066-the-databases-own-price.md) and
[ADR-0067](../../docs/adr/0067-the-compaction-reads-a-file-at-a-time.md).

**And the old files go when no reader holds them.** A reader that pinned them still has to read them, so they stay until it is
gone. The database owns the file naming — the highest index in the table's directory plus one — so a compaction does not reset
the numbering and land the next write on a file that is still there.

**And a database written by an earlier build of this crate has to be recreated**: the files carry the hidden columns now, and
a file that does not carry them is not a file of the table.

## The vocabulary

**The engine's, and only the engine's.** A column's type is one of `Int64`, `Float64` or `Utf8`, and the values are the
engine's `ScalarValue`. The database adds no type of its own, because a second vocabulary is a conversion at every
boundary and a place for the two to disagree.

## The query language

**The engine's typed plan.** A read is a `QueryPlan` — built by the engine's own `PlanBuilder`, over the engine's own
`ColumnExpr` — run against the table's files. **There is no SQL parser**, and the decision is recorded: SQL promises
subqueries, CTEs, window functions and arbitrary join graphs, and this engine refuses all four, so a SQL front end
would have to carry the engine's limits as a rule set just to say "not supported" honestly. A front end is a separate
increment over the same plan.

## What it is not

**No SQL. No rollback. No second writer.** One writer and many readers: a read sees one version of a table, and the
writer is alone.

## The increments

**What is here now** is the database, the persistent catalog, the schema, the typed write path, the snapshot read, the hidden
row id, and the compaction that pays for the append-only model — **and the compaction reads a file at a time, so the table's
size is not its ceiling**. **What is not here yet** is the index, whose shape the scan's figure decided — **and whose
encoding is now decided too, by a measurement**: the key does not rise in the table's rows, so every file holds a range that
spans nearly the whole key space, and the range map names every file. **The per-key map is the index**, and its price is
unmeasured because there is no index to measure.

**And every price was measured before it was built.** The gap between the engine and the store was priced first, so that the
index's shape is decided by a figure and not by an opinion — that is spec 055. **And the compaction's own ceiling was priced
too**, and the price found it — that is spec 056. **And the index's encoding was priced before the index**, and the price
refused an arm — that is spec 057.
