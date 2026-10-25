//! The lazy path and the eager path must agree on the **real** dataset.
//!
//! The engine's own `lazy_scan.rs` holds the two paths to the same answer
//! over hand-built files that hit the edges: NULLs, NaNs, dictionary
//! columns, row counts around the block boundary. This file does the other
//! half of the job: it runs **the same plans the benchmark runs** -- the
//! TPC-H Q6 predicate set and the rest of the built-in queries -- over the
//! **real generated file**, and checks that the lazy path gives the same
//! answer as the eager one.
//!
//! It also checks the claim the increment rests on: that the lazy path
//! reads only the columns the plan needs. On the real file that is two of
//! sixteen columns for Q6, and `l_comment` -- half of the file's raw bytes --
//! is not among them.
//!
//! The dataset is generated rather than committed: a 231 MiB binary in the
//! repository would be a blob nobody could review, and a generator nobody
//! could run would be worse.

use std::path::{Path, PathBuf};

use columnar_cli::queries::{BuiltinQuery, QueryDefinition, QUERIES};
use columnar_engine::{execute, execute_on, Dataset, ExecOptions, ParquetSource};
use columnar_testdata::{generate, DatasetSpec};

/// The dataset, generated once for the whole test binary.
///
/// Generating six million rows takes a few seconds, and there are several
/// queries to check, so the file is made once and shared.
fn dataset() -> &'static Path {
    static PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        let path = std::env::temp_dir().join("columnar-lazy-e2e-lineitem.parquet");
        let spec = DatasetSpec { scale_factor: 1, seed: 24001, null_rate: 0.0 };
        generate(&spec, &path).expect("the dataset can be generated");
        path
    })
}

/// The definition of a built-in query.
fn definition(query: BuiltinQuery) -> &'static QueryDefinition {
    QUERIES.iter().find(|definition| definition.query == query).expect("the query is built in")
}

/// The two paths must give the same answer, over the real file.
fn check(definition: &QueryDefinition) -> columnar_engine::AggregateResult {
    let path = dataset();

    let dataset = Dataset::read_parquet(path).expect("the dataset can be read eagerly");
    let eager = execute(&(definition.plan)(), &dataset, &ExecOptions::default())
        .expect("the eager path answers the query");

    let source = ParquetSource::open(path).expect("the source can be opened");
    let lazy = execute_on(&(definition.plan)(), &source, &ExecOptions::default())
        .expect("the lazy path answers the query");

    assert_eq!(
        eager.groups,
        lazy.groups,
        "`{}`: the eager and the lazy path disagreed",
        definition.query.name()
    );
    lazy
}

/// Every built-in query must give the same answer on both paths.
#[test]
fn every_builtin_query_agrees_on_both_paths() {
    for definition in QUERIES {
        check(definition);
    }
}

/// The lazy path reads only the columns the plan needs.
///
/// This is the claim the increment rests on, so it is checked against the
/// plan output over the real file rather than inferred from a timing.
#[test]
fn the_lazy_path_reads_two_of_the_sixteen_columns_for_q6() {
    let path = dataset();
    let definition = definition(BuiltinQuery::Q6);

    let source = ParquetSource::open(path).expect("the source can be opened");
    assert_eq!(source.columns().len(), 16, "the file does not have sixteen columns");

    let result =
        execute_on(&(definition.plan)(), &source, &ExecOptions::default()).expect("the query runs");

    let mut read = result.plan_output.columns_read.clone();
    read.sort();
    assert_eq!(
        read,
        vec!["l_discount".to_owned(), "l_shipdate".to_owned()],
        "the lazy path did not read exactly Q6's two columns"
    );

    // All four of Q6's predicates are pushed into the reader -- as two
    // conjunctions, one per column, because the reader decodes a column once
    // per projection mask that mentions it.
    assert_eq!(
        result.plan_output.pushed_down,
        vec![
            "l_discount >= 0.04 and l_discount <= 0.07".to_owned(),
            "l_shipdate >= '1994-01-01' and l_shipdate < '1995-01-01'".to_owned(),
        ],
        "not every Q6 predicate was pushed down: {:?}",
        result.plan_output.pushed_down
    );

    // And the eager path, for contrast, materialises the whole file.
    let dataset = Dataset::read_parquet(path).expect("the dataset can be read eagerly");
    let eager =
        execute(&(definition.plan)(), &dataset, &ExecOptions::default()).expect("the query runs");
    assert!(
        eager.plan_output.columns_read.len() >= 16,
        "the eager path did not read every column: {:?}",
        eager.plan_output.columns_read
    );
}

/// The scan stage of the lazy path reports the file's rows in and the
/// surviving rows out.
///
/// This is what makes the cold measurement legible: the rows the reader
/// discarded during the scan are visible in the plan output.
#[test]
fn the_scan_stage_reports_the_rows_the_reader_discarded() {
    let path = dataset();
    let definition = definition(BuiltinQuery::Q6);

    let source = ParquetSource::open(path).expect("the source can be opened");
    let result =
        execute_on(&(definition.plan)(), &source, &ExecOptions::default()).expect("the query runs");

    let scan = result
        .plan_output
        .stages
        .iter()
        .find(|stage| stage.stage == "scan")
        .expect("the plan has a scan stage");
    assert_eq!(scan.rows_in, 6_000_000, "the scan did not start at the file's row count");
    assert!(scan.rows_out < scan.rows_in, "the scan discarded nothing");
}

/// The dataset really was generated, and it holds the whole file.
#[test]
fn the_dataset_was_generated() {
    let path = dataset();
    assert!(path.exists(), "the dataset was not generated");
    let bytes = std::fs::metadata(path).expect("the dataset's size is readable").len();
    assert!(bytes > 200_000_000, "the dataset is smaller than the published file");
}
