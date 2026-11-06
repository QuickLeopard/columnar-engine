# columnar-store

An **append-only store** for the columnar engine: a writer, a manifest, and a commit.

The engine reads Parquet files. This crate **writes** them, and it keeps the catalog that says which
files make up a table.

## The shape

- A **table** is a **set of files**. A commit appends a file and never modifies an old one, so the store
  grows by adding to the set and never rewrites it.
- The **manifest** is the table's truth: its files, their rows and their bytes, and the table's totals. It
  is small, it is JSON, and it is the one thing a reader has to believe.
- The **commit** is five steps — the data file made durable, the manifest written, made durable and
  renamed over the old one, and the directory made durable — and **there is no write-ahead log**: a commit
  is a rename, and the rename is the answer. A reader that sees the new manifest sees the new data; a
  reader that sees the old one sees the old data.

## Using it

```rust
use arrow::record_batch::RecordBatch;
use columnar_store::Store;

let store = Store::open("/tmp/warehouse")?;
let manifest = store.append("lineitem", &batches)?;

// The table's files, and the manifest that names them.
for file in &manifest.files {
    println!("{}: {} rows, {} bytes", file.name, file.rows, file.bytes);
}
# Ok::<(), columnar_store::Error>(())
```

The rows arrive as Arrow batches, and **every batch but the last has to be on the store's grid** —
`BATCH_ROWS`, 65,536 rows. The last may be shorter, because it is the end of the stream.

## The engine's rows

The rows the **engine** produces are the engine's own vocabulary -- a `ColumnChunk` with an encoding and
a validity mask -- and the store takes them:

```rust
use columnar_store::Store;

let store = Store::open("/tmp/warehouse")?;

// The engine's rows, in the engine's own vocabulary.
let rows = columnar_engine::execute_rows_on(&plan, &source, &options)?;
let manifest = store.append_rows("lineitem", &rows.chunks, writer_properties(&[]))?;
```

**And a table comes back the same way.** A table is **many files**, and `Table::paths` hands the list over.
The engine reads that list as one table and hands the rows out, so **a store's table can be read by the
engine and written back**:

```rust
use columnar_engine::prelude::*;

let table = store.table("lineitem")?.expect("the table is there");
let set = FileSetSource::open(&table.paths())?;
let rows = set.execute_rows(&plan, &options)?;
let manifest = store.append_rows("lineitem-copy", &rows.chunks, writer_properties(&[]))?;
```

**The rows are the file's rows**: the set's rows and one file's rows are the same rows, in the same order,
and the two write the same file. **What the round trip does not carry is the nullability** (below): the
rows say which rows are NULL and not whether a column may be, and the store's own rule decides that.
**The figures are in [PERF.md §3aa](https://github.com/QuickLeopard/columnar-engine/blob/main/PERF.md#3aa-the-sets-rows).**

**The dictionary does come back.** The engine hands out the rows that were read **as they lie** when the
plan removes nothing, so a dictionary column is still a dictionary column -- and the store's conversion
takes both shapes, the dictionary and the plain one. A plan that removes rows in the engine, a join and a
sort make new rows, and those rows are gathered: the dictionary is materialised away, and the store takes
those too. **So the shape of the rows depends on the plan, and the caller can see which shape it got**:
the encoding says so.

**The grid is still the store's**: the rows are re-batched inside the store, and the engine does not
learn the number 65,536. **The conversion is the store's**, because it cannot live in `columnar-core` (a
vocabulary that depends on a framework is not a vocabulary) and must not live in the engine (the
engine's surface would carry Arrow) -- so the store depends on the core, and on nothing else in this
workspace.

**The schema is the store's too**: a column that holds no NULL is written **REQUIRED**, and a column that
holds one is written **OPTIONAL**. The engine's rows say which rows are NULL and not whether a column
may be, and the Parquet writer makes that difference visible in the bytes.

## The grid, and why it is a rule

**The file's bytes depend on the batch size the rows arrive in.** The Parquet writer splits every batch it
is handed into mini-batches of 1,024 values, and the dictionary's fallback to plain is decided at one of
those boundaries. The same rows in a different batch size are a different file.

So the store has a grid, and it **checks** it: a batch that is off the grid is refused with an error,
rather than the store writing a file whose bytes nobody can predict.

The grid is not an arbitrary choice. `BATCH_ROWS` is the size the published file was written in, and **the
store writes that file byte for byte** when it is given the same rows in that grid.

## The policy is the caller's

The store does not decide the encoding. `writer_properties(&[])` is the store's default — the dictionary
on, a million rows a row group, SNAPPY — and `writer_properties(&["l_comment"])` names the columns that
should be written plain, which is the right call for a high-cardinality column whose dictionary page would
be large and useless.

A caller that writes the same rows under a different policy gets a different file. That is the caller's
choice, and it is the caller's file.

## What it is not

**No SQL. No transactions with rollback. No MVCC, no snapshot isolation. No concurrent writers. No
row-level updates. No compaction. One writer.** A table grows by appending a file and committing it; it
does not shrink, and it does not change in place.

## Where it sits

**Below the engine.** This crate does not depend on `columnar-engine`: it writes the files the engine
reads, and the engine's read path cannot move because of anything here. It depends on `columnar-core` --
the vocabulary, and nothing else in this workspace -- because **the engine's rows can be written**: the
conversion from the engine's own chunks to Arrow's cannot live in the core (a vocabulary that depends on
a framework is not a vocabulary) and must not live in the engine (the engine's surface would carry
Arrow), so it lives here. (Its *tests* depend on the engine, to read what the store wrote; that edge is
test-only and it is not in the published crate.)

**The glue is a list of paths.** The engine reads a table that is *many* files through one source over
a set of files, and that source takes the list of paths. `Table::paths` is that list, in the table's
order -- and the table's order is the order the rows come back in.

```rust
use columnar_store::Store;

let store = Store::open("/tmp/warehouse")?;
let table = store.table("lineitem")?.expect("the table is there");
let paths = table.paths();
```

## It publishes with the others

It was held back while its API was still moving, and the increment that settled it -- the engine's
source over a set of files -- is done. **The publish itself is a separate act**: it needs a crates.io
account and a token, and the order and the commands are in the repository's `docs/embedding.md`.

## The licence

MIT, like the rest of this workspace.
