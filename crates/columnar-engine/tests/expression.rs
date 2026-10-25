//! The arithmetic expression: its rules, and its agreement with the eager path.
//!
//! The rules under test are the ones the spec fixes:
//!
//! - an unknown column is a **plan** error, reported before anything is read;
//! - a `utf8` column is not a number, and neither is a `utf8` constant;
//! - division needs a float operand, because the engines disagree about
//!   integer division;
//! - `int64` only if every operand is `int64`;
//! - NULL in, NULL out;
//! - `NaN` propagates through arithmetic, even though it never matches a
//!   comparison;
//! - integer arithmetic saturates.

// The tests index into the result's groups directly. A group that is not
// there is a test bug, and the panic says which assertion it broke.
#![allow(clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;

use arrow::array::{ArrayRef, Float64Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use columnar_core::{
    ArithOp, BatchColumn, BatchData, ComparisonOp, Expr, NullBitmap, Predicate, ScalarValue,
};
use columnar_engine::{
    execute, execute_on, Aggregate, AggregateSpec, Dataset, EngineError, ExecOptions,
    ParquetSource, QueryPlan, ResolvedAggregateFn,
};

/// A float column over the given values.
fn floats(values: &[Option<f64>]) -> ArrayRef {
    Arc::new(Float64Array::from(values.to_vec()))
}

/// A temporary file that removes itself when it is dropped.
struct TempFile {
    path: PathBuf,
}

impl TempFile {
    /// A path under the system's temporary directory, unique to this test.
    fn new(name: &str) -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let path =
            std::env::temp_dir().join(format!("columnar-expression-{name}-{unique}.parquet"));
        Self { path }
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Builds a dataset with two float columns and a nullable one.
fn dataset() -> Dataset {
    let price = columnar_core::ColumnChunk {
        name: "l_extendedprice".to_owned(),
        encoding: columnar_core::Encoding::Float64 { values: vec![100.0, 200.0, 300.0, 400.0] },
        validity: NullBitmap::all_valid(4),
        len: 4,
    };
    let discount = columnar_core::ColumnChunk {
        name: "l_discount".to_owned(),
        encoding: columnar_core::Encoding::Float64 { values: vec![0.0, 0.1, 0.2, 0.3] },
        // The third row has no discount, so the expression over it is NULL.
        validity: NullBitmap::with_nulls(4, &[2]),
        len: 4,
    };
    let quantity = columnar_core::ColumnChunk {
        name: "l_quantity".to_owned(),
        encoding: columnar_core::Encoding::Float64 { values: vec![1.0, 2.0, 3.0, 4.0] },
        validity: NullBitmap::all_valid(4),
        len: 4,
    };
    Dataset::from_chunks(vec![price, discount, quantity])
}

/// A plan that sums `l_extendedprice * (1 - l_discount)`.
fn revenue_plan() -> QueryPlan {
    QueryPlan {
        filter: Vec::new(),
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::Sum {
                expr: Expr::binary(
                    ArithOp::Mul,
                    Expr::column("l_extendedprice"),
                    Expr::binary(ArithOp::Sub, Expr::constant(1.0), Expr::column("l_discount")),
                ),
            }],
        }),
    }
}

/// The sum the plan above computes, by hand.
///
/// 100 × 1.0 = 100, 200 × 0.9 = 180, the third row is NULL and is skipped,
/// and 400 × 0.7 = 280. The total is 560.
fn hand_computed() -> f64 {
    100.0 + 180.0 + 280.0
}

/// The expression aggregate sums what it says it sums.
#[test]
fn an_expression_aggregate_sums_the_expression() {
    let dataset = dataset();
    let result =
        execute(&revenue_plan(), &dataset, &ExecOptions::default()).expect("the plan runs");

    let values = &result.groups[0].values;
    match values.first() {
        Some(columnar_engine::AggregateValue::Float(value)) => {
            assert!(
                (value - hand_computed()).abs() < 1e-9,
                "the sum is {value}, not {}",
                hand_computed()
            );
        }
        other => panic!("the sum is {other:?}, not a float"),
    }
}

/// A NULL row is skipped, exactly as SQL skips it.
#[test]
fn a_null_row_is_skipped_by_the_sum() {
    let dataset = dataset();
    let result =
        execute(&revenue_plan(), &dataset, &ExecOptions::default()).expect("the plan runs");

    // Three of the four rows have a value, and one of them is the NULL row.
    let values = &result.groups[0].values;
    match values.first() {
        Some(columnar_engine::AggregateValue::Float(value)) => {
            assert!(
                (value - hand_computed()).abs() < 1e-9,
                "the sum is {value}: the NULL row was not skipped"
            );
        }
        other => panic!("the sum is {other:?}, not a float"),
    }
}

/// An expression naming a column the data does not have is a plan error.
#[test]
fn an_unknown_column_in_an_expression_is_a_plan_error() {
    let dataset = dataset();
    let plan = QueryPlan {
        filter: Vec::new(),
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::Sum {
                expr: Expr::binary(
                    ArithOp::Mul,
                    Expr::column("l_extendedprice"),
                    Expr::column("l_no_such_column"),
                ),
            }],
        }),
    };

    match execute(&plan, &dataset, &ExecOptions::default()) {
        Err(EngineError::UnknownColumn(name)) => assert_eq!(name, "l_no_such_column"),
        other => panic!("the plan should have been rejected: {other:?}"),
    }
}

/// A string column is not a number.
#[test]
fn a_utf8_column_in_an_expression_is_a_type_error() {
    let dataset = Dataset::from_chunks(vec![columnar_core::ColumnChunk {
        name: "l_shipdate".to_owned(),
        encoding: columnar_core::Encoding::Utf8 { values: vec!["1994-01-01".to_owned()] },
        validity: NullBitmap::all_valid(1),
        len: 1,
    }]);

    let plan = QueryPlan {
        filter: Vec::new(),
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::Sum { expr: Expr::column("l_shipdate") }],
        }),
    };

    match execute(&plan, &dataset, &ExecOptions::default()) {
        Err(EngineError::TypeMismatch(_)) => {}
        other => panic!("the plan should have been rejected: {other:?}"),
    }
}

/// Integer division is refused, because the engines disagree about it.
#[test]
fn integer_division_is_refused() {
    let dataset = dataset();
    let plan = QueryPlan {
        filter: Vec::new(),
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::Sum {
                expr: Expr::binary(ArithOp::Div, Expr::constant(5i64), Expr::constant(2i64)),
            }],
        }),
    };

    match execute(&plan, &dataset, &ExecOptions::default()) {
        Err(EngineError::TypeMismatch(message)) => {
            assert!(message.contains("division"), "the message does not explain itself: {message}");
        }
        other => panic!("the plan should have been rejected: {other:?}"),
    }
}

/// The expression's columns are materialised, and the plan output names them.
///
/// A column the expression reads is a column the plan reads. If it were not
/// materialised, the read would fetch the wrong columns and the answer would
/// be right by accident.
#[test]
fn the_expressions_columns_are_materialised() {
    let dataset = dataset();
    let result =
        execute(&revenue_plan(), &dataset, &ExecOptions::default()).expect("the plan runs");
    let output = result.plan_output.to_string();

    assert!(output.contains("materialised columns: l_extendedprice, l_discount"), "{output}");
    assert!(output.contains("evaluate"), "the evaluate stage is missing:\n{output}");
}

/// The expression is evaluated over the rows that survived the filter.
///
/// SQL applies `WHERE` before the aggregate, and so does this: the evaluate
/// stage sees the survivors, not the whole file.
#[test]
fn the_expression_is_evaluated_over_the_survivors() {
    let dataset = dataset();
    let plan = QueryPlan {
        filter: vec![Predicate::new("l_quantity", ComparisonOp::Lt, 3.0f64)],
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::Sum {
                expr: Expr::binary(
                    ArithOp::Mul,
                    Expr::column("l_extendedprice"),
                    Expr::binary(ArithOp::Sub, Expr::constant(1.0), Expr::column("l_discount")),
                ),
            }],
        }),
    };

    let result = execute(&plan, &dataset, &ExecOptions::default()).expect("the plan runs");

    // The first two rows survive: 100 × 1.0 + 200 × 0.9 = 280.
    match result.groups[0].values.first() {
        Some(columnar_engine::AggregateValue::Float(value)) => {
            assert!((value - 280.0).abs() < 1e-9, "the sum is {value}, not 280");
        }
        other => panic!("the sum is {other:?}, not a float"),
    }

    let evaluate = result
        .stages
        .iter()
        .find(|stage| stage.stage == "evaluate")
        .expect("the evaluate stage ran");
    assert_eq!(evaluate.rows_in, 2, "the expression was evaluated over the whole file");
}

/// A `NaN` propagates through arithmetic, even though it never matches a
/// comparison.
///
/// The two rules are different on purpose: a comparison is a question and a
/// `NaN` has no answer, while arithmetic is a computation and a `NaN` is a
/// value.
#[test]
fn a_nan_propagates_through_arithmetic() {
    let dataset = Dataset::from_chunks(vec![columnar_core::ColumnChunk {
        name: "x".to_owned(),
        encoding: columnar_core::Encoding::Float64 { values: vec![f64::NAN, 2.0] },
        validity: NullBitmap::all_valid(2),
        len: 2,
    }]);
    let plan = QueryPlan {
        filter: Vec::new(),
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::Sum {
                expr: Expr::binary(ArithOp::Mul, Expr::column("x"), Expr::constant(2.0)),
            }],
        }),
    };

    let result = execute(&plan, &dataset, &ExecOptions::default()).expect("the plan runs");
    match result.groups[0].values.first() {
        Some(columnar_engine::AggregateValue::Float(value)) => {
            assert!(value.is_nan(), "the sum is {value}, not a NaN");
        }
        other => panic!("the sum is {other:?}, not a float"),
    }
}

/// The expression's type is `int64` only when every operand is.
#[test]
fn the_expressions_type_is_int64_only_when_every_operand_is() {
    let dataset = Dataset::from_chunks(vec![
        columnar_core::ColumnChunk {
            name: "i".to_owned(),
            encoding: columnar_core::Encoding::Int64 { values: vec![1, 2] },
            validity: NullBitmap::all_valid(2),
            len: 2,
        },
        columnar_core::ColumnChunk {
            name: "f".to_owned(),
            encoding: columnar_core::Encoding::Float64 { values: vec![1.0, 2.0] },
            validity: NullBitmap::all_valid(2),
            len: 2,
        },
    ]);

    // `i + i` is an integer sum, and it is exact.
    let integer = QueryPlan {
        filter: Vec::new(),
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::Sum {
                expr: Expr::binary(ArithOp::Add, Expr::column("i"), Expr::column("i")),
            }],
        }),
    };
    let result = execute(&integer, &dataset, &ExecOptions::default()).expect("the plan runs");
    match result.groups[0].values.first() {
        Some(columnar_engine::AggregateValue::Int(value)) => assert_eq!(*value, 6),
        other => panic!("the sum is {other:?}, not an integer"),
    }

    // `i + f` is a float sum.
    let float = QueryPlan {
        filter: Vec::new(),
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::Sum {
                expr: Expr::binary(ArithOp::Add, Expr::column("i"), Expr::column("f")),
            }],
        }),
    };
    let result = execute(&float, &dataset, &ExecOptions::default()).expect("the plan runs");
    match result.groups[0].values.first() {
        Some(columnar_engine::AggregateValue::Float(value)) => assert!((value - 6.0).abs() < 1e-9),
        other => panic!("the sum is {other:?}, not a float"),
    }
}

/// Integer arithmetic saturates rather than wrapping.
///
/// A wrapping sum would be a different answer on a different machine, and a
/// different answer on the same machine after a rebuild.
#[test]
fn integer_arithmetic_saturates() {
    let dataset = Dataset::from_chunks(vec![columnar_core::ColumnChunk {
        name: "x".to_owned(),
        encoding: columnar_core::Encoding::Int64 { values: vec![i64::MAX] },
        validity: NullBitmap::all_valid(1),
        len: 1,
    }]);
    let plan = QueryPlan {
        filter: Vec::new(),
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::Sum {
                expr: Expr::binary(ArithOp::Add, Expr::column("x"), Expr::constant(1i64)),
            }],
        }),
    };

    let result = execute(&plan, &dataset, &ExecOptions::default()).expect("the plan runs");
    match result.groups[0].values.first() {
        Some(columnar_engine::AggregateValue::Int(value)) => {
            assert_eq!(*value, i64::MAX, "the sum wrapped instead of saturating");
        }
        other => panic!("the sum is {other:?}, not an integer"),
    }
}

/// The expression's columns are read from the file, not from the plan's
/// projection.
///
/// The lazy path resolves the plan against the *file's* schema and remaps the
/// indices afterwards, so an expression's indices have to move with the
/// columns they name.
#[test]
fn the_lazy_path_resolves_an_expression_against_the_file() {
    let file = TempFile::new("expression-lazy");
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("other", DataType::Float64, true),
            Field::new("l_extendedprice", DataType::Float64, true),
            Field::new("l_discount", DataType::Float64, true),
        ])),
        vec![
            floats(&[Some(0.0), Some(0.0), Some(0.0), Some(0.0)]),
            floats(&[Some(100.0), Some(200.0), Some(300.0), Some(400.0)]),
            // The third row has no discount, exactly as in the eagerly
            // decoded dataset, so the two paths are answering the same
            // question about the same data.
            floats(&[Some(0.0), Some(0.1), None, Some(0.3)]),
        ],
    )
    .expect("the batch matches the schema");

    {
        let file_handle = std::fs::File::create(&file.path).expect("the file can be created");
        let properties = parquet::file::properties::WriterProperties::builder()
            .set_dictionary_enabled(true)
            .build();
        let mut writer =
            parquet::arrow::ArrowWriter::try_new(file_handle, batch.schema(), Some(properties))
                .expect("the writer accepts the batch");
        writer.write(&batch).expect("the batch can be written");
        writer.close().expect("the file can be closed");
    }

    let source = ParquetSource::open(&file.path).expect("the source can be opened");
    let lazy =
        execute_on(&revenue_plan(), &source, &ExecOptions::default()).expect("the lazy plan runs");

    match lazy.groups[0].values.first() {
        Some(columnar_engine::AggregateValue::Float(value)) => {
            assert!(
                (value - hand_computed()).abs() < 1e-9,
                "the lazy sum is {value}, not {}",
                hand_computed()
            );
        }
        other => panic!("the lazy sum is {other:?}, not a float"),
    }
}

/// The computed column carries the expression's rows.
///
/// The column is what the aggregate sums, so it has to have one row per
/// surviving row and one validity bit per row.
#[test]
fn the_computed_column_has_one_row_per_surviving_row() {
    let dataset = dataset();
    let plan = revenue_plan();
    let resolved = plan.resolve(&dataset).expect("the plan resolves");
    let expression =
        match resolved.aggregate.as_ref().expect("there is an aggregate").aggregates.first() {
            Some(ResolvedAggregateFn::Sum { expr }) => expr,
            other => panic!("the aggregate is {other:?}"),
        };

    let columns: Vec<BatchColumn> = vec![
        BatchColumn::with_validity(
            "l_extendedprice".to_owned(),
            NullBitmap::all_valid(4),
            BatchData::Float64(vec![100.0, 200.0, 300.0, 400.0]),
        ),
        BatchColumn::with_validity(
            "l_discount".to_owned(),
            NullBitmap::with_nulls(4, &[2]),
            BatchData::Float64(vec![0.0, 0.1, 0.2, 0.3]),
        ),
    ];
    let by_index = |index: usize| -> &BatchColumn { &columns[index] };

    let evaluated = columnar_engine::operators::expr::evaluate(expression, &by_index, 4)
        .expect("the expression evaluates");

    assert_eq!(evaluated.validity.len(), 4);
    match &evaluated.data {
        BatchData::Float64(values) => {
            assert!((values[0] - 100.0).abs() < 1e-9);
            assert!((values[1] - 180.0).abs() < 1e-9);
            assert!(!evaluated.validity.is_valid(2), "the NULL row is not NULL");
            assert!((values[3] - 280.0).abs() < 1e-9);
        }
        other => panic!("the computed column holds {other:?}"),
    }
}

/// The engine's own `SUM` of a plain column is unchanged.
///
/// The expression is a superset of the old behaviour, and the old behaviour
/// has to be exactly the old behaviour.
#[test]
fn a_sum_of_a_plain_column_is_unchanged() {
    let dataset = dataset();
    let plan = QueryPlan {
        filter: Vec::new(),
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::Sum { expr: Expr::column("l_quantity") }],
        }),
    };
    let result = execute(&plan, &dataset, &ExecOptions::default()).expect("the plan runs");

    match result.groups[0].values.first() {
        Some(columnar_engine::AggregateValue::Float(value)) => {
            assert!((value - 10.0).abs() < 1e-9, "the sum is {value}, not 10");
        }
        other => panic!("the sum is {other:?}, not a float"),
    }

    // And it does **not** gain an evaluate stage: there is nothing to evaluate.
    assert!(
        !result.stages.iter().any(|stage| stage.stage == "evaluate"),
        "a plain column's sum should not have an evaluate stage"
    );
}

/// The expression's columns are read from the file.
#[test]
fn the_scalar_value_of_a_constant_is_reported() {
    assert_eq!(ScalarValue::Float64(1.5).to_string(), "1.5");
}
