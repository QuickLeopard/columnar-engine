//! The embedder test.
//!
//! **This file imports only the facade.** Every name it uses comes from
//! `columnar_engine::prelude::*` — the surface — and from the standard
//! library. It is the surface's test: it is written the way an embedder
//! writes, and if it needs something the surface does not carry, the surface
//! is wrong.
//!
//! The one thing it does that an embedder would not do is write its own
//! Parquet file, because a test that needs a dataset on disk is a test that
//! does not run anywhere.

use columnar_engine::prelude::*;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;

/// A file with two rows that survive and one that does not.
const QUANTITY: [i64; 4] = [1, 5, 9, 13];
const COMMENT: [&str; 4] = ["a", "b", "c", "d"];

/// Writes a small Parquet file and returns its path.
///
/// The file is removed when the guard is dropped, so a test run leaves
/// nothing behind.
struct TempFile {
    path: PathBuf,
}

impl TempFile {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("columnar-engine-embed-{name}.parquet"));
        let schema = Arc::new(Schema::new(vec![
            Field::new("l_quantity", DataType::Int64, false),
            Field::new("l_comment", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(QUANTITY.to_vec())),
                Arc::new(StringArray::from(COMMENT.to_vec())),
            ],
        )
        .expect("the batch is well formed");
        let file = std::fs::File::create(&path).expect("the file is created");
        let mut writer = ArrowWriter::try_new(file, schema, None).expect("the writer is built");
        writer.write(&batch).expect("the batch is written");
        writer.close().expect("the file is finished");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The first aggregate value of the first group.
///
/// The indexing is a panic the test wants, so it is written as one.
fn first_value(result: &AggregateResult) -> AggregateValue {
    result
        .groups
        .first()
        .expect("the query returned a group")
        .values
        .first()
        .expect("the group has a value")
        .clone()
}

/// The plan the tests use: two rows, counted.
fn count_over_two() -> QueryPlan {
    QueryPlan::new(
        vec![col("l_quantity").gt(2)],
        Vec::new(),
        Some(AggregateSpec { group_by: Vec::new(), aggregates: vec![Aggregate::CountStar] }),
    )
}

/// The lazy path: open a file, plan over it, run it.
#[test]
fn the_lazy_path_answers_the_question() {
    let file = TempFile::new("lazy");
    let source = open_source(file.path()).expect("the file opens");
    let result =
        execute_on(&count_over_two(), &source, &ExecOptions::default()).expect("the query runs");
    assert_eq!(
        first_value(&result),
        AggregateValue::Int(3),
        "three of the four rows are greater than two"
    );
}

/// The eager path: the same plan over a decoded dataset.
#[test]
fn the_eager_path_answers_the_same_question() {
    let file = TempFile::new("eager");
    let dataset = Dataset::read_parquet(file.path()).expect("the file reads");
    let result =
        execute(&count_over_two(), &dataset, &ExecOptions::default()).expect("the query runs");
    assert_eq!(
        first_value(&result),
        AggregateValue::Int(3),
        "the eager path and the lazy path agree"
    );
}

/// The catalog: a name becomes a path, and the plan carries the path.
#[test]
fn the_catalog_resolves_a_name_when_the_plan_is_built() {
    let file = TempFile::new("catalog");
    let catalog =
        Catalog::new().with_table("lineitem", file.path()).with_table("part", "data/part.parquet");

    let plan = catalog
        .plan()
        .filter(col("l_quantity").gt(2))
        .join(TableAddress::Named("part"), "l_partkey", "p_partkey")
        .aggregate(AggregateSpec { group_by: Vec::new(), aggregates: vec![Aggregate::CountStar] })
        .build()
        .expect("the catalog holds both names");

    let join = plan.join.expect("the plan has a join");
    assert_eq!(
        join.build_path,
        Path::new("data/part.parquet"),
        "the name is resolved to a path when the plan is built"
    );
}

/// An unknown name is an error at build time, and the error names the table.
#[test]
fn an_unknown_table_is_an_error_before_anything_is_read() {
    let catalog = Catalog::new().with_table("lineitem", "data/lineitem.parquet");

    let error = catalog
        .plan()
        .join(TableAddress::Named("orders"), "l_orderkey", "o_orderkey")
        .build()
        .expect_err("the catalog does not hold `orders`");

    let EngineError::UnknownTable { name, known } = &error else {
        panic!("the error is about the table that is not in the catalog");
    };
    assert_eq!(name, "orders", "the error names the table");
    assert_eq!(
        known.iter().map(String::as_str).collect::<Vec<_>>(),
        vec!["lineitem"],
        "the error lists the tables it does hold"
    );

    // And the message a user reads says what to do about it.
    let message = error.to_string();
    assert!(message.contains("orders"), "the message names the table: {message}");
    assert!(message.contains("lineitem"), "the message lists the alternatives: {message}");
    assert!(
        message.contains("Catalog::with_table"),
        "the message says what to do about it: {message}"
    );
}

/// A table that cannot be opened is an error that names the table.
#[test]
fn a_table_that_cannot_be_opened_names_the_table() {
    let catalog = Catalog::new().with_table("missing", "data/does-not-exist.parquet");
    let error = open_table(&catalog, "missing").expect_err("the file is not there");

    let EngineError::TableUnreadable { table, .. } = error else {
        panic!("the error is about the table that could not be opened");
    };
    assert_eq!(table, "missing", "the error names the table");
}

/// The expression builder writes the plan the way a query reads.
#[test]
fn the_expression_builder_writes_the_plan() {
    let predicate = col("l_quantity").gt(2);
    assert_eq!(predicate.column, "l_quantity");
    assert_eq!(predicate.op, ComparisonOp::Gt);
    assert_eq!(predicate.value, ScalarValue::Int64(2));

    // And the arithmetic reads in the order the query writes it.
    let expression = lit(1.0) - col("l_discount");
    assert_eq!(
        expression.to_sql(),
        "(1 - l_discount)",
        "the order of a subtraction is not a detail"
    );
}

/// The plan output accounts for the whole query.
#[test]
fn the_plan_output_accounts_for_the_query() {
    let file = TempFile::new("output");
    let source = open_source(file.path()).expect("the file opens");
    let result =
        execute_on(&count_over_two(), &source, &ExecOptions::default()).expect("the query runs");

    let output = &result.plan_output;
    assert!(!output.stages.is_empty(), "the query ran stages");
    assert!(output.estimated_bytes > 0, "the query estimated its working set");
    assert_eq!(
        output.budget_bytes,
        ExecOptions::default().memory_budget.bytes,
        "the estimate was checked against the budget"
    );
    assert!(
        output.columns_read.iter().any(|name| name == "l_quantity"),
        "the column the query read is named: {:?}",
        output.columns_read
    );
}

/// A plan that does not fit the budget is refused, and it is refused before
/// any work is done.
#[test]
fn a_plan_that_does_not_fit_the_budget_is_refused() {
    let file = TempFile::new("budget");
    let source = open_source(file.path()).expect("the file opens");
    let options = ExecOptions { memory_budget: MemoryBudget { bytes: 0 }, ..Default::default() };

    let error = execute_on(&count_over_two(), &source, &options).expect_err("the budget is zero");
    assert!(
        matches!(error, EngineError::Memory(MemoryError::BudgetExceeded)),
        "the error is the budget: {error}"
    );
}
