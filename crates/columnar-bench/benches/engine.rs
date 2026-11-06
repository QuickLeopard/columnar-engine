//! `CodSpeed` benchmarks for the engine, end to end.
//!
//! The benchmarks run the builtin queries -- the same plans the published
//! comparison runs -- over a small, deterministic TPC-H `lineitem` table and
//! its `orders` companion. The tables are generated once per process by
//! `columnar-testdata` into a temporary directory, so no dataset is committed
//! and none has to be prepared before the benchmark runs.
//!
//! Two paths are measured for every query:
//!
//! - `eager`: the table is already decoded into a [`Dataset`]; only the plan
//!   runs.
//! - `lazy`: the plan runs against the Parquet file, so the scan, the
//!   predicate pushdown and the decode of the columns it needs are measured
//!   too.
//!
//! The decode of the whole file and the generator are measured on their own.

use std::path::PathBuf;
use std::sync::OnceLock;

use columnar_engine::queries::{BuiltinQuery, QUERIES};
use columnar_engine::{execute, execute_on, open_source, Dataset, ExecOptions, ParquetSource};
use columnar_testdata::generator::DatasetGenerator;
use columnar_testdata::writer::{write_dataset, write_orders_dataset};
use columnar_testdata::DatasetSpec;
use divan::{black_box, Bencher};

/// Rows in the `lineitem` table. Small enough for the CPU simulation, big
/// enough to span several engine batches and several row groups.
const LINEITEM_ROWS: u64 = 50_000;

/// Rows in the `orders` table: a quarter of `lineitem`, as in TPC-H.
const ORDERS_ROWS: u64 = LINEITEM_ROWS / 4;

/// Rows per Parquet row group, so the file holds several of them.
const ROW_GROUP_ROWS: usize = 16_384;

/// Every builtin query, in the order the comparison reports them.
const QUERY_NAMES: [BuiltinQuery; 8] = [
    BuiltinQuery::Scan,
    BuiltinQuery::FilterNumeric,
    BuiltinQuery::FilterString,
    BuiltinQuery::FilterRange,
    BuiltinQuery::Q6,
    BuiltinQuery::Q6Revenue,
    BuiltinQuery::JoinOrders,
    BuiltinQuery::SortTopK,
];

fn main() {
    divan::main();
}

/// The generated tables, written once per process.
struct Tables {
    lineitem: PathBuf,
    orders: PathBuf,
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let dir =
            std::env::temp_dir().join(format!("columnar-bench-codspeed-{}", std::process::id()));
        let lineitem = dir.join("lineitem.parquet");
        let orders = dir.join("orders.parquet");
        let spec = DatasetSpec::default();
        write_dataset(&spec, LINEITEM_ROWS, &lineitem, ROW_GROUP_ROWS)
            .expect("the lineitem table is written");
        write_orders_dataset(&spec, ORDERS_ROWS, LINEITEM_ROWS, &orders, ROW_GROUP_ROWS)
            .expect("the orders table is written");
        Tables { lineitem, orders }
    })
}

fn decoded() -> &'static Dataset {
    static DATASET: OnceLock<Dataset> = OnceLock::new();
    DATASET.get_or_init(|| {
        Dataset::read_parquet(&tables().lineitem).expect("the lineitem table is read")
    })
}

fn source() -> &'static ParquetSource {
    static SOURCE: OnceLock<ParquetSource> = OnceLock::new();
    SOURCE.get_or_init(|| open_source(&tables().lineitem).expect("the lineitem table opens"))
}

/// The plan of a builtin query, wired to the generated `orders` file.
fn plan_for(query: BuiltinQuery) -> columnar_engine::QueryPlan {
    let definition =
        QUERIES.iter().find(|definition| definition.query == query).expect("it is a builtin query");
    (definition.plan)(&tables().orders)
}

/// The default options: one thread, the fastest kernel the machine has.
fn options() -> ExecOptions {
    ExecOptions { threads: Some(1), ..ExecOptions::default() }
}

#[divan::bench(args = QUERY_NAMES)]
fn eager(bencher: Bencher<'_, '_>, query: BuiltinQuery) {
    let plan = plan_for(query);
    let dataset = decoded();
    let options = options();
    bencher.bench(|| execute(black_box(&plan), dataset, &options).expect("the query runs"));
}

#[divan::bench(args = QUERY_NAMES)]
fn lazy(bencher: Bencher<'_, '_>, query: BuiltinQuery) {
    let plan = plan_for(query);
    let source = source();
    let options = options();
    bencher.bench(|| execute_on(black_box(&plan), source, &options).expect("the query runs"));
}

#[divan::bench]
fn read_parquet(bencher: Bencher<'_, '_>) {
    let path = &tables().lineitem;
    bencher.bench(|| Dataset::read_parquet(black_box(path)).expect("the lineitem table is read"));
}

#[divan::bench]
fn generate_lineitem_batch(bencher: Bencher<'_, '_>) {
    let spec = DatasetSpec::default();
    bencher.bench(|| {
        let mut generator = DatasetGenerator::new(&spec, 8_192).expect("the generator builds");
        generator.next_batch(black_box(8_192)).expect("the batch is generated")
    });
}
