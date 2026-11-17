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

**What is here now** is the database, the persistent catalog, the schema and the table handle. **What comes next** is
the typed write path, the snapshot read, the hidden row id, and the compaction that pays for the append-only model.
The whole of it is spec 055, and the gap's prices are measured before the parts that depend on them are built.
