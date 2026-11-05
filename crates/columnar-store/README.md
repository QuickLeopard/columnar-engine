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
reads, and the engine's read path cannot move because of anything here. The glue between them is a path.

## It is not published yet

The crate's API is new, and the increment that teaches the engine to read a table that is *many* files
changes it. A first published version is a promise, and this one has not been reviewed by an embedder --
so the crate carries `publish = false` for now. It is in the tree, and the three published crates are the
ones a `cargo add` can reach.

## The licence

MIT, like the rest of this workspace.
