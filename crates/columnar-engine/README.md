# columnar-engine

**A typed query plan over Parquet, as a library.** The engine reads a Parquet file into columns
and executes a plan over them: `scan → filter → project → [join] → [sort] → aggregate`. The plan
is a Rust struct, so there is no SQL parser standing in front of it.

The crate root is the public surface, and `columnar_engine::prelude::*` carries the same list.
The **full surface list is in the crate's rustdoc**, and every name on it is reachable from both
paths.

---

## A runnable example

The example writes its own Parquet file, so it runs anywhere:

```rust
use columnar_engine::prelude::*;

use std::sync::Arc;

use arrow::array::Int64Array;
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A one-column Parquet file with four rows.
    let path = std::env::temp_dir().join("columnar-engine-readme.parquet");
    {
        let schema = Arc::new(Schema::new(vec![Field::new("l_quantity", DataType::Int64, false)]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int64Array::from(vec![1_i64, 2, 3, 4]))],
        )?;
        let file = std::fs::File::create(&path)?;
        let mut writer = ArrowWriter::try_new(file, schema, None)?;
        writer.write(&batch)?;
        writer.close()?;
    }

    // Open the file, and plan over it.
    let source = open_source(&path)?;
    let plan = QueryPlan::new(
        vec![col("l_quantity").gt(2)],
        Vec::new(),
        Some(AggregateSpec { group_by: Vec::new(), aggregates: vec![Aggregate::CountStar] }),
    );

    // Run it.
    let result = execute_on(&plan, &source, &ExecOptions::default())?;

    // Two of the four rows are greater than two.
    assert_eq!(result.groups[0].values[0], AggregateValue::Int(2));
    Ok(())
}
```

`execute_on` is the **lazy** path: it reads only the columns the plan needs, and it resolves the
plan against the file's schema before a row is read. `execute` is the **eager** path: the same
plan over a dataset that is already decoded.

A **named** table goes through a catalog, and the name is resolved **when the plan is built** —
before any data is read:

```rust
use columnar_engine::prelude::*;

let catalog = Catalog::new()
    .with_table("lineitem", "data/lineitem.parquet")
    .with_table("orders", "data/orders.parquet");

let plan = catalog
    .plan()
    .filter(col("l_discount").gt(0.04))
    .join(TableAddress::Named("orders"), "l_orderkey", "o_orderkey")
    .aggregate(AggregateSpec {
        group_by: Vec::new(),
        aggregates: vec![Aggregate::CountStar],
    })
    .build()?;

// The name is a path now, and the plan carries the path.
let join = plan.join.as_ref().expect("the plan has a join");
assert_eq!(join.build_paths, vec![std::path::PathBuf::from("data/orders.parquet")]);
```

---

## The rows, and the store that writes them

The engine reads files and it does not write them, but **it can hand its rows to something that does**:
`execute_rows` and `execute_rows_on` return the plan's **projection**, gathered, as a `Dataset`.

```rust
use columnar_engine::prelude::*;

let source = open_source("data/lineitem.parquet")?;
let rows = execute_rows_on(&plan, &source, &ExecOptions::default())?;
```

**And a set's rows come out the same way.** `FileSetSource::execute_rows` is the set's own counterpart of
`execute_rows_on` -- the set's rows, in the set's order -- so **a store's table can be read by the engine and
written back**:

```rust
use columnar_engine::prelude::*;

let set = FileSetSource::open(&table.paths())?;
let rows = set.execute_rows(&plan, &ExecOptions::default())?;
```

A plan with an **aggregation** is refused: the aggregate folds the rows into groups, and there is
nothing left to write. The rows are the engine's own vocabulary -- a column chunk with an encoding and a
validity mask -- and `columnar-store` turns them into Parquet.

## The guarantees

**A NULL never matches.** Not in the filter, not in the join, not in a `!=`. A NULL is the
*absence* of a validity bit rather than a sentinel inside a value array, so a row whose bit is
clear is not a candidate for any comparison at all. The rule is implemented identically in the
kernel's oracle and in both SIMD paths, and the predicate pushed into the scan and the filter that
follows it run the same kernel over the same values — so the two agree.

**A NaN never matches.** A comparison whose value or whose threshold is a NaN never matches, for
every operator, `!=` included. This is "unknown propagates", and it is stricter than SQL's
treatment of NaN.

**The plan output accounts for the whole query.** Every stage reports the rows it was handed, the
rows it produced, and the time it took. The **attribution** then divides the query's own elapsed
time into the time the stages account for and the time that fell outside them, so the output never
claims more than it can show and never hides what it cannot.

**The answer does not depend on the schedule or the thread count.** The read splits the file's
**row groups** across the threads — a row group is decoded independently of every other one — and
merges the parts back together in the file's order. The hash aggregate **sorts its groups by key**
before returning them, so the output order does not depend on the hash seed. The join keeps the
probe side's row order.

**Two runs produce the same output in the same order.** The same plan over the same data, twice.

**A table that is many files is one table.** `FileSetSource` reads a **set** of Parquet files as one
table: the metadata merged, the statistics the table's, the predicate reaching every file, and the rows
in the files' order. It costs **nothing measurable** over reading the same rows in one file, and the
naive way — open each file and run the query over each one — is wrong for a filter, an aggregate and a
join.

---

## The boundaries

**Read-only.** The engine reads Parquet and writes nothing. There is no write path, and no catalog
on disk: the catalog is in memory and the embedder owns it.

**Parquet only.** The engine opens no transport and speaks no binary protocol. Parquet appears at
the I/O boundary and nowhere above it — the engine's own representation is its own column chunks.

**One inner equi-join.** One build side, one probe side, one key column. The build side is the
right table; the probe side is the left one, and it stays lazy.

**No SQL.** The plan is a Rust struct. A parser is a month of work that produces no evidence about
the part of this project that is under test.

**No outer joins, no window functions, no `AVG`, and no more than two tables.** Each is a new
operator, and each wants its own measurement before it is believed. The operator set is filter,
project, hash aggregate, hash join and sort.

**The operators are single-threaded.** Only the read is parallel. The filter, the project, the
join, the sort and the aggregate run on one thread. The operators take a `&Dataset` and return
owned values, so the data path is already shaped for a future morsel loop — but no morsel driver,
no task pool and no per-thread scheduling exist yet.

**The memory budget is a pre-flight check, not a mechanism.** A plan whose estimated working set
exceeds the budget is refused before any work is done. It is not spilled to disk.

**Three physical types.** `int64`, `float64` and `utf8`. A NULL is not a value, and the engine
never casts silently: comparing an `int64` column against a `float64` threshold is a plan error
rather than something to paper over.

---

## The trust boundary

The engine reads **files an embedder's users provide**, so it says what it reads.

It reads **Parquet files**. It opens **no transport** and speaks **no binary protocol**.

**The dependency policy exempts one advisory, and the exemption is published.** `RUSTSEC-2024-0436` —
`paste` is no longer maintained — is ignored in
[`deny.toml`](https://github.com/QuickLeopard/columnar-engine/blob/main/deny.toml), with the
reason recorded beside it: it is a build-time macro of the Parquet stack, not shipped code.

**And the exemption of `thrift` is gone.** `RUSTSEC-2026-0435` — `thrift` 0.17.0, an unbounded
allocation in `TBinaryInputProtocol::read_bytes` — is no longer ignored, because the graph's `thrift`
is **0.23.0** and the advisory does not cover it. `cargo deny` says so itself: with the exemption in
place, it reports *no crate matched advisory criteria*. An exemption that names nothing is a silence.

**And the weakness that exemption stood next to was measured, and it is closed.** The hole was **not**
in the file's metadata — the footer is read through the reader's own *slice* protocol, which refuses a
declared length larger than the bytes that remain **before** it allocates. The hole was in the **page
headers**: the compact protocol allocates the declared length eagerly, bounded by a `u32`, so a
hostile Parquet file could ask for **four gigabytes** before a byte of payload was read — and the
probe that measured it asked for exactly what the file declared, out of a 25-byte header.

**The engine does not fix it with a wrapper of its own.** The fix is upstream, and it is in
`parquet`'s own reader from **58.4.1**: it does not read the page statistics by default, so the
declared length is never parsed at all. The reader now asks for **8 KiB** at every declared length —
the honest file's own figure. The measurement, the before and the after, are in
[`PERF.md` §3w](https://github.com/QuickLeopard/columnar-engine/blob/main/PERF.md#3w-the-readers-four-gigabytes)
and in [ADR-0049](https://github.com/QuickLeopard/columnar-engine/blob/main/docs/adr/0049-the-readers-four-gigabytes.md).

**A reader who cannot see the weakness cannot decide whether to read an untrusted file.** The
decision is the embedder's. The engine's job is to say what it does.

---

## Where to go next

The **embedding guide** —
[`docs/embedding.md`](https://github.com/QuickLeopard/columnar-engine/blob/main/docs/embedding.md) —
is the full walk-through: getting the data, building a plan, running it, reading the result, the
errors, the stability policy, and the order the three crates are published in.

The **rustdoc** carries the full surface list, and the crate-level doc carries a second worked
example.

The **measured numbers** are in
[`PERF.md`](https://github.com/QuickLeopard/columnar-engine/blob/main/PERF.md). It measures the
comparison kernel four ways and the engine against DuckDB, Polars and DataFusion, on the same file
and at the same thread count, and it states the conditions every figure was taken under. It owns
the figures; this README states none.
