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

// The tests index into the result's groups directly. A group that is not
// there is a test bug, and the panic says which assertion it broke.
#![allow(clippy::indexing_slicing)]

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

/// The real Q6 -- the revenue query -- must give the same answer on both
/// paths, and the same answer `DuckDB` gives.
///
/// The number below is `DuckDB`'s, computed on this same file:
///
/// ```
/// $ duckdb -c "SELECT sum(l_extendedprice * (1 - l_discount))
///               FROM 'data/lineitem.parquet'
///               WHERE l_shipdate >= '1994-01-01' AND l_shipdate < '1995-01-01'
///                 AND l_discount >= 0.05 AND l_discount <= 0.07
///                 AND l_quantity < 24;"
/// 5526985127.480705
/// ```
///
/// The engine's answer is `5526985127.48064`. The two differ in the last
/// few digits, because a floating-point sum is not associative and the two
/// systems sum in different orders -- and the comparison the harness makes
/// is a relative one, for exactly that reason.
#[test]
fn the_real_q6_answers_what_duckdb_answers() {
    let path = dataset();
    let definition = definition(BuiltinQuery::Q6Revenue);

    let source = ParquetSource::open(path).expect("the source can be opened");
    let lazy = execute_on(&(definition.plan)(), &source, &ExecOptions::default())
        .expect("the lazy path answers the query");

    let dataset = Dataset::read_parquet(path).expect("the dataset can be read eagerly");
    let eager = execute(&(definition.plan)(), &dataset, &ExecOptions::default())
        .expect("the eager path answers the query");

    assert_eq!(lazy.groups, eager.groups, "the two paths disagree on the real Q6");

    let value = match eager.groups[0].values.first() {
        Some(columnar_engine::AggregateValue::Float(value)) => *value,
        other => panic!("the sum is {other:?}, not a float"),
    };
    let duckdb = 5_526_985_127.480_705_f64;
    let relative = (value - duckdb).abs() / duckdb.abs();
    assert!(
        relative < 1e-9,
        "the engine computed {value} and DuckDB computed {duckdb}: a relative difference of {relative}"
    );
}

/// The dataset really was generated, and it holds the whole file.
#[test]
fn the_dataset_was_generated() {
    let path = dataset();
    assert!(path.exists(), "the dataset was not generated");
    let bytes = std::fs::metadata(path).expect("the dataset's size is readable").len();
    assert!(bytes > 200_000_000, "the dataset is smaller than the published file");
}

/// The file has several row groups, so the parallel path can engage at all.
///
/// The parallel read splits the file's **row groups** across the threads. A
/// file with one row group therefore has one worker however many threads are
/// asked for, and every parallel test below would be testing the
/// single-threaded path under the parallel path's name.
///
/// This is checked rather than assumed, because the check is the only thing
/// that makes the tests below mean anything.
#[test]
fn the_dataset_has_several_row_groups() {
    let path = dataset();
    let source = ParquetSource::open(path).expect("the source can be opened");
    let row_groups = source.row_groups();
    assert!(
        row_groups > 1,
        "the file has {row_groups} row group(s), so the parallel path cannot engage"
    );
}

/// Every built-in query must give the same answer at every thread count.
///
/// This is the requirement that makes parallel decode **safe** rather than
/// merely fast. `SUM` is not associative, so a parallel read that ordered its
/// rows by *completion* rather than by *position* would make the engine's
/// answer depend on the scheduler: the same query would return different
/// bits on different runs, and a user could not tell a correct answer from
/// an incorrect one.
///
/// So the answers have to be identical -- the single-threaded one, the
/// parallel ones, and the eager path's. Over every built-in query, on the
/// real file.
#[test]
fn every_builtin_query_agrees_at_every_thread_count() {
    let path = dataset();

    for definition in QUERIES {
        // The eager path: the reference answer, computed from the whole file
        // in one go.
        let dataset = Dataset::read_parquet(path).expect("the dataset can be read eagerly");
        let eager = execute(&(definition.plan)(), &dataset, &ExecOptions::default())
            .expect("the eager path answers the query");

        let source = ParquetSource::open(path).expect("the source can be opened");
        let single = execute_on(&(definition.plan)(), &source, &ExecOptions::default())
            .expect("the lazy path answers the query");

        assert_eq!(
            single.groups,
            eager.groups,
            "`{}`: the lazy and the eager path disagreed",
            definition.query.name()
        );

        // More threads than the file has row groups, so the cap is exercised
        // as well as the split.
        for threads in [2, 3, 4, 6, 8, 14] {
            let options = ExecOptions { threads: Some(threads), ..ExecOptions::default() };
            let parallel = execute_on(&(definition.plan)(), &source, &options)
                .expect("the parallel query runs");

            assert_eq!(
                parallel.groups,
                single.groups,
                "`{}`: the answer differs at {threads} threads",
                definition.query.name()
            );
            assert_eq!(
                parallel.plan_output.threads,
                threads.min(source.row_groups()),
                "`{}`: the plan output does not report the thread count",
                definition.query.name()
            );
        }
    }
}

/// The sum is the same integer, bit for bit, at every thread count.
///
/// The sum is an integer sum, so "the same" is not a matter of tolerance: it
/// is the same `i64`. A parallel read that reordered the rows would not
/// change an integer sum -- but it would change a *floating-point* one, and
/// this is the cheapest place to hold the order honest.
#[test]
fn the_sum_is_bit_for_bit_the_same_at_every_thread_count() {
    let path = dataset();
    let definition = definition(BuiltinQuery::FilterRange);

    let source = ParquetSource::open(path).expect("the source can be opened");
    let single =
        execute_on(&(definition.plan)(), &source, &ExecOptions::default()).expect("the query runs");

    for threads in [2, 4, 8, 14] {
        let options = ExecOptions { threads: Some(threads), ..ExecOptions::default() };
        let parallel =
            execute_on(&(definition.plan)(), &source, &options).expect("the parallel query runs");
        assert_eq!(parallel.groups, single.groups, "the sum differs at {threads} threads");
    }
}
