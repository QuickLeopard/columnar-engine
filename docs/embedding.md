# Embedding the columnar engine

**This is the guide an embedder reads.** It walks the whole way round: getting the data, building a
plan, running it, reading the result, the errors, what is frozen, and the order the three crates
are published in.

The engine is a library, not a service. It reads a Parquet file, executes a typed plan over it, and
hands back the groups and the plan output. There is no server, no wire protocol and no client.

The snippets below are written as an embedder writes them, and they assume a function that returns
`Result<(), Box<dyn std::error::Error>>`. Every name comes from `columnar_engine::prelude::*`.

---

## What it is, and what it is not

**What it is.** A typed query plan over Parquet, as a Rust library. The pipeline is fixed and
pinned: `scan → filter → project → [join] → [sort] → aggregate`, and the optimisation order is
pinned as well: `predicate pushdown → projection pushdown → late materialization → join → sort →
aggregate`. Both orders are printed in the plan output, so a reader never has to guess what the
engine did.

**What it is not.** Each of these is out of scope, and out of scope means *said out loud*, not
deferred silently:

- **Not a SQL parser.** The plan is a Rust struct. A parser is a month of work that produces no
  evidence about the part of this project that is under test.
- **Not a write path.** The engine is read-only over Parquet.
- **Not a server.** No wire protocol, no client, no daemon.
- **Not a storage engine.** There are no transactions, no MVCC, and no catalog on disk: the catalog
  is in memory and the embedder owns it.
- **Not a general query engine.** No outer joins, no window functions, no `AVG`, and no more than
  two tables. The operator set is filter, project, hash aggregate, hash join and sort.
- **Not parallel in its operators.** The **read** is parallel; the operators are not.

---

## Getting the data

### A file on disk

`open_source` takes a path and gives back a `ParquetSource`. It opens the file, reads its schema,
and reports a failure as `EngineError::TableUnreadable` rather than as a bare I/O error, because
the thing that failed is a **table**, and the user needs to know which.

```rust
use columnar_engine::prelude::*;
use std::path::Path;

let source = open_source(Path::new("data/lineitem.parquet"))?;
```

The source reads at most `MAX_READER_BATCH_ROWS` rows at a time — 131,072 — and the reader's batch
size is a public constant, so an embedder can say what it is working with.

### A named table

A `Catalog` maps names to Parquet paths. It is the embedder's: the engine never reads one from
disk, never mutates one, and never outlives the borrow it was given.

`open_table` resolves a name through the catalog and opens the file it resolves to. An unknown name
is an error that names the table and lists the names the catalog does hold.

```rust
use columnar_engine::prelude::*;

let catalog = Catalog::new()
    .with_table("lineitem", "data/lineitem.parquet")
    .with_table("orders", "data/orders.parquet");

let source = open_table(&catalog, "lineitem")?;
```

### A table that is many files

`FileSetSource` reads a **set** of Parquet files as **one table**. It is what an append-only store's
table needs: a commit appends a file and never modifies an old one, so the table is the set of them.

```rust
use columnar_engine::prelude::*;
use std::path::PathBuf;

let paths: Vec<PathBuf> = manifest.files.iter().map(PathBuf::from).collect();
let set = FileSetSource::open(&paths)?;
let result = set.execute(&plan, &ExecOptions::default())?;
```

**The files have to share their schema** -- their columns, their order, their types, and how their values
are stored -- and a set whose files disagree is not a table: the error says which file disagreed and how.
**The order of the paths is the table's order**: the rows come back file by file, and inside a file row
group by row group.

**And the set answers the same question the one file answers.** The same plan over the set and over the
same rows in one file gives the same answer, and reading the set costs **nothing measurable** over reading
the one file. The naive way -- open each file and run the query over each one -- is **wrong three times
over**: the rows of a filtered table are split across the files, the groups of an aggregate are split
across them, and the sides of a join are split across them.

**And the join's build side takes a set.** `JoinSpec.build_paths` is a list of paths: one path is one
file, exactly as it was, and several are read as one table — so a store's table can be on the **build**
side of a join as well as on the **probe** side.

### The eager path

`Dataset::read_parquet` decodes the whole file into column chunks, in memory. The eager path runs
the same plan over that dataset with `execute`. It is the right path when the data is read once and
queried several times.

```rust
use columnar_engine::prelude::*;
use std::path::Path;

let dataset = Dataset::read_parquet(Path::new("data/lineitem.parquet"))?;
```

`Dataset` also answers `len()`, `column_names()` and `chunk(name)`.

---

## Writing the data: the store

The engine **reads** Parquet files and it never writes one. **`columnar-store` is the crate that writes
them**, and it keeps the catalog that says which files make up a table.

- A **table** is a **set of files**. A commit appends a file and never modifies an old one.
- The **manifest** is the table's truth: its files, their rows and their bytes, and the table's totals.
- The **commit** is five steps — the data file made durable, the manifest written, made durable and
  `rename`d over the old one, and the directory made durable — and **there is no write-ahead log**.

```rust
use columnar_store::Store;

let store = Store::open("/tmp/warehouse")?;
let manifest = store.append("lineitem", &batches)?;
```

**And the table hands the engine its files.** `Table::paths` is the glue: the list of paths, in the
table's order, is what the engine's source over a set of files takes.

```rust
use columnar_engine::prelude::*;
use columnar_store::Store;

let store = Store::open("/tmp/warehouse")?;
let table = store.table("lineitem")?.expect("the table is there");

let set = FileSetSource::open(&table.paths())?;
let result = set.execute(&plan, &ExecOptions::default())?;
```

**And the rows arrive on the store's grid.** Every batch but the last has to be **65,536** rows, and the
store checks it: the Parquet writer cuts every batch it is handed into mini-batches of 1,024 values, and
the dictionary's fallback to plain is decided at one of those boundaries, so **the same rows in a different
batch size are a different file**. A batch off the grid is refused with an error rather than written.

**And the store sits below the engine.** `columnar-store` does not depend on `columnar-engine`, so the
engine's read path cannot move because of anything the store does. **The glue between them is a list of
paths** -- and the engine's side of that glue is built: `FileSetSource` reads the set as one table, and it
costs nothing measurable over reading the same rows in one file.

**And the engine's rows go the other way.** The engine's rows are the engine's own vocabulary, and
the store takes them: `execute_rows_on` gives the plan's rows, and `append_rows` writes them, re-batches
them on the store's grid, and commits them. **The glue is two lines**, and the grid stays the store's.

```rust
use columnar_engine::prelude::*;
use columnar_store::Store;

let source = open_source("data/lineitem.parquet")?;
let rows = execute_rows_on(&plan, &source, &ExecOptions::default())?;

let store = Store::open("/tmp/warehouse")?;
store.append_rows("lineitem", &rows.chunks, writer_properties(&[]))?;
```

**A plan with an aggregation is refused**, because the aggregate folds the rows into groups and there is
nothing left to write. **And the schema is the store's**: a column that holds no NULL is written
REQUIRED, and a column that holds one is written OPTIONAL -- the engine's rows say which rows are NULL
and not whether a column may be.

**And a table comes back the same way it went in.** The set has its own way to hand its rows out --
`FileSetSource::execute_rows` -- so **the round trip is three lines**: the table's paths go to the set, the
set's rows come out, and the store writes them.

```rust
use columnar_engine::prelude::*;
use columnar_store::Store;

let store = Store::open("/tmp/warehouse")?;
let table = store.table("lineitem")?.expect("the table is there");

let set = FileSetSource::open(&table.paths())?;
let rows = set.execute_rows(&plan, &ExecOptions::default())?;

store.append_rows("lineitem-copy", &rows.chunks, writer_properties(&[]))?;
```

**The rows are the file's rows**: the set's rows and one file's rows are the same rows, in the same order,
and the two write the same file. **What the round trip does not carry is the nullability** above -- the
rows say which rows are NULL and not whether a column may be. **The dictionary does come back**: the
engine hands out the rows that were read **as they lie** when the plan removes nothing, and the store's
conversion takes both that shape and the gathered one. **The figures are in
[PERF.md §3aa](https://github.com/QuickLeopard/columnar-engine/blob/main/PERF.md#3aa-the-sets-rows).**

**And the crate publishes with the others.** It was held back while its API was still moving, and the
increment that settled it — the engine's source over a set of files — is done. **The publish itself is a
separate act** and it needs a crates.io account and a token: the order and the commands are below.

---

## Building a plan

### The plan struct

`QueryPlan` is a plain struct: a filter, a projection, an optional aggregation, an optional join
and an optional sort. `QueryPlan::new` takes the first three; the other two are set by name.

```rust
use columnar_engine::prelude::*;

let plan = QueryPlan::new(
    vec![col("l_discount").gt(0.04)],
    Vec::new(),
    Some(AggregateSpec {
        group_by: Vec::new(),
        aggregates: vec![Aggregate::CountStar],
    }),
);
```

The projection lists the columns to materialise, and an empty projection means every column. A
plan **with** an aggregation ignores the projection: the aggregate result is the output, and late
materialisation keeps the decoded set to the filter and aggregate columns.

### The builder

The builder is the same plan, with the naming in it. It is bound to a catalog — the one from the
section above — and it takes the same pieces in the order a query reads.

```rust
use columnar_engine::prelude::*;

let catalog = Catalog::new()
    .with_table("orders", "data/orders.parquet");

let plan = catalog
    .plan()
    .filter(col("l_discount").gt(0.04))
    .projection(["l_orderkey", "l_extendedprice"])
    .aggregate(AggregateSpec {
        group_by: Vec::new(),
        aggregates: vec![Aggregate::CountStar],
    })
    .join(TableAddress::Named("orders"), "l_orderkey", "o_orderkey")
    .sort(SortSpec {
        column: "l_extendedprice".to_owned(),
        descending: true,
        limit: Some(10),
    })
    .build()?;
```

`join_filtered` is the same join with predicates on the build side; they are applied **before**
the table is hashed, so they save the work of hashing the rows that fail them.

### The names are resolved when the plan is built

The builder holds a **reference** to the table it was given — a name, or a path — and resolves it
**when the plan is built**, before any data is read. The plan the engine receives is an ordinary
`QueryPlan`, with a path where the name was.

An unknown name is therefore an error at build time, and the error names the table:

```rust
use columnar_engine::prelude::*;

let catalog = Catalog::new().with_table("lineitem", "data/lineitem.parquet");

let error = catalog
    .plan()
    .join(TableAddress::Named("orders"), "l_orderkey", "o_orderkey")
    .build()
    .expect_err("the catalog does not hold `orders`");

match error {
    EngineError::UnknownTable { name, known } => {
        assert_eq!(name, "orders");
        assert_eq!(known, vec!["lineitem".to_owned()]);
    }
    other => panic!("the error is about the table: {other}"),
}
```

---

## The expression builder

`col("x")` is a column, and the methods on it build the two things a plan is made of: a
**predicate** and an **aggregate**.

```rust
use columnar_engine::prelude::*;

let predicate = col("l_quantity").gt(3);
assert_eq!(predicate.column, "l_quantity");
assert_eq!(predicate.op, ComparisonOp::Gt);
assert_eq!(predicate.value, ScalarValue::Int64(3));
```

The six operators are `gt`, `gte`, `lt`, `lte`, `eq` and `ne`. The order of the operands is not a
detail: `lit(1.0) - col("l_discount")` is `1 - l_discount`, and `col("l_discount") - lit(1.0)` is
the other one.

The four aggregates are `sum`, `count`, `min` and `max`. `count` counts a column's **non-NULL**
values; `Aggregate::CountStar` counts the rows.

```rust
use columnar_engine::prelude::*;

let sum = col("l_extendedprice").sum();
let count = col("l_quantity").count();
let min = col("l_quantity").min();
let max = col("l_quantity").max();
let rows = Aggregate::CountStar;
```

An aggregate can sum an **expression**, which is what TPC-H's Q6 needs. `Expr::binary` is the
one way to combine two expressions:

```rust
use columnar_engine::prelude::*;

// `1 - l_discount`, in the order the query writes it.
let discount = lit(1.0) - col("l_discount");
assert_eq!(discount.to_sql(), "(1 - l_discount)");

// And the sum Q6 needs: `l_extendedprice * (1 - l_discount)`.
let revenue = Expr::binary(ArithOp::Mul, col("l_extendedprice").expr(), discount);
assert_eq!(revenue.to_sql(), "(l_extendedprice * (1 - l_discount))");

let aggregate = Aggregate::Sum { expr: revenue };
```

---

## Running it

### The two entry points

`execute_on` is the **lazy** path and `execute` is the **eager** path. Both take a plan, the data,
and the options, and both return an `AggregateResult`.

```rust
use columnar_engine::prelude::*;

let plan = QueryPlan::new(
    vec![col("l_quantity").gt(2)],
    Vec::new(),
    Some(AggregateSpec {
        group_by: Vec::new(),
        aggregates: vec![Aggregate::CountStar],
    }),
);
let source = open_source("data/lineitem.parquet")?;
let dataset = Dataset::read_parquet("data/lineitem.parquet")?;

// The lazy path: a plan over a Parquet file.
let result = execute_on(&plan, &source, &ExecOptions::default())?;

// The eager path: the same plan over a decoded dataset.
let result = execute(&plan, &dataset, &ExecOptions::default())?;
```

The lazy path reads only the columns the plan needs, resolves the plan against the file's schema
before a row is read, and pushes the plan's predicates into the scan. The pushdown reproduces the
engine's filter semantics exactly — a NULL never matches, a NaN never matches, for every operator
including `!=` — so the pushed-down filter and the filter that follows it agree.

### The knobs

`ExecOptions` carries three knobs, and all three are public fields.

```rust
use columnar_engine::prelude::*;

let options = ExecOptions {
    // The per-query memory budget. The default is 256 MiB.
    memory_budget: MemoryBudget { bytes: 256 * 1024 * 1024 },
    // Which comparison kernel to run. `None` means "the fastest available".
    flavor: Some(Flavor::CppSimd),
    // How many threads to read the file with. `None` means one.
    threads: Some(4),
};
```

**`threads` is the read's thread count, and nothing else's.** The read splits the file's **row
groups** across the threads — a row group is decoded independently of every other one — and merges
the parts back together in the file's order. The operators are single-threaded in this release. The
answer does not depend on the thread count, because the rows come back in the file's order whatever
it is set to.

### What the plan output contains

`result.plan_output` is the engine's account of the query. Every field on it is public.

```rust
use columnar_engine::prelude::*;

let source = open_source("data/lineitem.parquet")?;
let plan = QueryPlan::new(
    vec![col("l_quantity").gt(2)],
    Vec::new(),
    Some(AggregateSpec {
        group_by: Vec::new(),
        aggregates: vec![Aggregate::CountStar],
    }),
);
let result = execute_on(&plan, &source, &ExecOptions::default())?;
let output = &result.plan_output;

// The stages, in execution order: what each was handed, what it produced,
// and how long it took.
for stage in &output.stages {
    println!("{}: {} in, {} out", stage.stage, stage.rows_in, stage.rows_out);
}

// The columns the query read, and the columns it materialised.
println!("read: {:?}", output.columns_read);
println!("materialised: {:?}", output.materialised_columns);

// The predicates that were pushed into the scan, and what they cost.
println!("pushed down: {:?}", output.pushed_down);
println!("predicate ns: {}", output.pushdown_metrics.predicate_ns);

// The join's report and the sort's report, when the plan had them.
if let Some(join) = &output.join {
    println!("join: {} x {} on {}", join.probe_key, join.build_key, join.build_file);
}
if let Some(sort) = &output.sort {
    println!("sort: {} ({} rows)", sort.column, sort.rows);
}

// The attribution: the query's own time, and how the stages divide it.
println!(
    "accounted {:?} of {:?}",
    output.attribution.accounted, output.attribution.elapsed
);
```

The **attribution** is the field that makes the output an account rather than a list of numbers: it
holds the query's own elapsed time, the time the stages account for, and the time that fell
outside them. A plan output that claims more than it can show is a plan output that is lying, and
this one cannot.

The output also carries `optimization_order` (the pinned order, as a string), `estimated_bytes` and
`budget_bytes` (the estimate and the budget it was checked against), `kernel_flavor`, `threads`,
`filter_workers` and `projection_workers`.

---

## The memory budget

**The budget is a pre-flight check, not a mechanism.** The engine estimates the plan's working set
from the file's schema and its row count, and compares the estimate with the budget **before** the
read. A plan that does not fit is refused having read nothing at all.

```rust
use columnar_engine::prelude::*;

let source = open_source("data/lineitem.parquet")?;
let plan = QueryPlan::new(
    vec![col("l_quantity").gt(2)],
    Vec::new(),
    Some(AggregateSpec {
        group_by: Vec::new(),
        aggregates: vec![Aggregate::CountStar],
    }),
);
let options = ExecOptions { memory_budget: MemoryBudget { bytes: 0 }, ..Default::default() };
let error = execute_on(&plan, &source, &options).expect_err("the budget is zero");

assert!(matches!(error, EngineError::Memory(MemoryError::BudgetExceeded)));
```

**It does not spill.** There is no spill file, no partition and no fallback: a plan that does not
fit is an error, and the remedy is a bigger budget or a smaller query.

The estimate accounts for the parallel read's peak, which holds two copies of the read set rather
than one, and for the join's build side and the sort's working set. A plan that does not fit is
refused before any of it is built.

---

## Reading the result

`AggregateResult` holds three things: the **groups**, the per-stage **statistics**, and the
rendered **plan output**.

```rust
use columnar_engine::prelude::*;

let source = open_source("data/lineitem.parquet")?;
let plan = QueryPlan::new(
    vec![col("l_quantity").gt(2)],
    Vec::new(),
    Some(AggregateSpec {
        group_by: Vec::new(),
        aggregates: vec![Aggregate::CountStar],
    }),
);
let result = execute_on(&plan, &source, &ExecOptions::default())?;

for group in &result.groups {
    // The group key values, in `group_by` order.
    for key in &group.key {
        println!("key: {key}");
    }

    // The aggregate values, in the order the aggregates were declared.
    for value in &group.values {
        match value {
            AggregateValue::Int(count) => println!("count: {count}"),
            AggregateValue::Float(total) => println!("sum: {total}"),
            AggregateValue::Null => println!("no value"),
        }
    }
}
```

**The NULL rule.** An aggregate skips the NULLs among the values it is given, and a group that had
nothing to add has **no number**: a `SUM` over an all-NULL group is `AggregateValue::Null`, not
zero. `AggregateValue` has three variants — `Float`, `Int` and `Null` — and the difference between
them is a difference of *meaning*, not of representation.

`groups` is empty when the plan had no aggregation. When it did, the groups come back **sorted by
key**, so the order does not depend on the hash seed and two runs produce the same output in the
same order.

---

## The errors

Everything that can go wrong is `EngineError`. It has eight variants today, and two of them are
the ones the naming layer adds: `UnknownTable` for a name the catalog does not hold, and
`TableUnreadable` for a table that could not be opened.

| Variant | What it means |
|---|---|
| `Memory(MemoryError::BudgetExceeded)` | the plan's estimated working set does not fit the budget |
| `UnknownColumn(String)` | the plan names a column the data does not have |
| `TypeMismatch(String)` | the plan uses a column with a type the operator cannot handle |
| `InvalidPlan(String)` | the plan is inconsistent: it was resolved against one schema and run over another |
| `Io(std::io::Error)` | the file could not be read |
| `Parquet(String)` | the file is not a Parquet file this reader understands |
| `UnknownTable { name, known }` | the plan named a table the catalog does not hold |
| `TableUnreadable { table, reason }` | a table could not be opened |

The engine never casts silently: comparing an `int64` column against a `float64` threshold is a
`TypeMismatch`, not something to paper over.

**`EngineError` is `#[non_exhaustive]`, so a match outside the crate needs a wildcard arm.** The
enum gains a variant in every increment that finds a new way to fail, and an exhaustive match on it
would be a breaking change each time.

```rust
use columnar_engine::prelude::*;

fn describe(error: &EngineError) -> String {
    match error {
        EngineError::Memory(MemoryError::BudgetExceeded) => {
            "the plan did not fit the budget".to_owned()
        }
        EngineError::UnknownColumn(name) => format!("no column named `{name}`"),
        EngineError::TypeMismatch(message) => format!("type mismatch: {message}"),
        EngineError::InvalidPlan(message) => format!("invalid plan: {message}"),
        EngineError::Io(io) => format!("io error: {io}"),
        EngineError::Parquet(message) => format!("parquet error: {message}"),
        EngineError::UnknownTable { name, known } => {
            format!("unknown table `{name}`; the catalog holds {known:?}")
        }
        EngineError::TableUnreadable { table, reason } => {
            format!("could not open `{table}`: {reason}")
        }
        // The wildcard arm is required, because `EngineError` is
        // `#[non_exhaustive]`.
        _ => "something else".to_owned(),
    }
}
```

`MemoryError` and `DatasetError` are the two smaller error types, and `DatasetError` converts into
`EngineError` for you.

---

## The stability policy

**This is the section to come back to.** It says what is promised and what is not.

**Frozen.** These types are a promise, and a breaking change to any of them is a **MAJOR** version:

`QueryPlan`, `Predicate`, `Aggregate`, `AggregateSpec`, `JoinSpec`, `SortSpec`,
`AggregateResult`, `AggregateValue`, `Group`, `EngineError`, `MemoryError`, `DatasetError`.

**Experimental.** The plan output's **rendering** and the **report's text**. The numbers the
reports carry are measurements and they move; the words around them and the shape of the rendering
may change without a major version. Read them, do not parse them.

**A breaking change to the frozen surface is a MAJOR version.** That is the whole policy.

**And the types that will grow are closed for construction.** `EngineError`, `Expr` and
`ScalarValue` are `#[non_exhaustive]` enums, and `Predicate` is a `#[non_exhaustive]` struct. So:

- **Adding a variant to an enum, or a field to the struct, is not a breaking change.** A type this
  general grows, and the non-exhaustive marking is what makes that growth cheap.
- **Removing a variant, or changing a field's type or its meaning, is a breaking change**, and it is
  a major version.
- **A match outside the crate needs a wildcard arm.** That is the price of the growth, and it is
  paid once.

The `Predicate` fields stay public and readable. What is closed is the **construction**:
`Predicate::new` is the one way in, and a struct literal in an embedder's code would be a breaking
change the moment the struct grew.

---

## Publishing the four crates

The four crates publish in the order their dependencies decide — **`columnar-core`, then
`columnar-kernels`, then `columnar-store`, then `columnar-engine`**.

`columnar-core` depends on nothing in the workspace. `columnar-kernels` depends on
`columnar-core`. `columnar-engine` depends on both. `columnar-store` depends on `columnar-core` — the
vocabulary, and nothing else in this workspace — and it writes files rather than reading them through
the engine's vocabulary. Each internal dependency carries a `version` beside its `path`, so
the published crate resolves on crates.io rather than in the workspace — and that is what makes the
order mandatory: `columnar-kernels` cannot be published before `columnar-core` is on crates.io, and
`columnar-engine` cannot be published before both are.

**Only the `[dependencies]` decide the order.** A dev-dependency is dropped from the published
manifest, so it is not part of the published graph: the store's *tests* depend on the engine, and the
store is still published before it.

**The store and the kernels are ready at once**, because both depend on the core and on nothing else:
**where the graph does not decide, the list does** — the list in `scripts/check-package.sh` is written in
the publish order, and the script computes the rest. The order is therefore `columnar-core`,
`columnar-kernels`, `columnar-store`, `columnar-engine`.

Each crate's `--dry-run` is run first, and the four are published in that order:

```bash
# 1. columnar-core -- it depends on nothing in the workspace.
cargo publish --dry-run -p columnar-core
cargo publish -p columnar-core

# 2. columnar-kernels -- it depends on columnar-core, which is now published.
cargo publish --dry-run -p columnar-kernels
cargo publish -p columnar-kernels

# 3. columnar-store -- it depends on columnar-core, which is now published.
cargo publish --dry-run -p columnar-store
cargo publish -p columnar-store

# 4. columnar-engine -- it depends on both, which are now published.
cargo publish --dry-run -p columnar-engine
cargo publish -p columnar-engine
```

---

## The trust boundary

The engine reads **files an embedder's users provide**, so it says what it reads.

It reads **Parquet files**. It opens **no transport** and speaks **no binary protocol**.

**The dependency policy exempts one advisory, and the exemption is published.**
`RUSTSEC-2024-0436` — `paste` is no longer maintained — is ignored in
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
decision is the embedder's. The engine's job is to say what it does, and to say it here.

---

## Where to go next

- The **crate README** for `columnar-engine` is the one-page version of this guide:
  [`crates/columnar-engine/README.md`](https://github.com/QuickLeopard/columnar-engine/blob/main/crates/columnar-engine/README.md).
- The **rustdoc** carries the full surface list, and the crate-level doc carries a worked example.
- The **measured numbers** are in
  [`PERF.md`](https://github.com/QuickLeopard/columnar-engine/blob/main/PERF.md). It measures the
  kernel four ways and the engine against DuckDB, Polars and DataFusion, and it states the
  conditions every figure was taken under.
