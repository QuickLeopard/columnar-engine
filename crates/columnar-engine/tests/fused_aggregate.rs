//! The fused aggregate's tests.
//!
//! A **global** aggregate -- one with no GROUP BY -- folds the survivors one
//! after another. It never needs their values aligned with anything, so the
//! engine does not build a column of the survivors: the aggregate reads the
//! source column where it lies, over the filter's selection.
//!
//! That is a change of *how* the values are reached, not of *what* they are.
//! Every test here therefore asks the same question twice -- once through
//! the fused path and once through a path that gathers -- and the two
//! answers have to be **identical**, bit for bit.
//!
//! The data is chosen to make a mistake visible:
//!
//! - The float column's sum depends on the **order** the rows are added in.
//!   A fold that visited the survivors in the wrong order would produce a
//!   different number, and the bit comparison would see it.
//! - The columns hold NULLs, because a fold that counted a NULL row would
//!   produce a different number too.
//! - One column is dictionary-encoded, because a fold that read the
//!   dictionary wrongly would produce a different number.
//!
//! And one test asks the question the fusion is really about: **does the
//! projection still materialise anything?** The decode counter answers it.

// The indices below are literals into vectors whose length the test has just
// built, so the panic the lint guards against cannot happen.
#![allow(clippy::indexing_slicing)]
// The casts below turn a row number into an `i64` or an `i32`. The row
// numbers are below sixty-four, so they are exact in every one of those
// types; the lints cannot see that, because the row number is a `usize` and
// a `usize` could be sixty-four bits wide.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

use columnar_core::{
    ArithOp, ColumnChunk, ComparisonOp, Encoding, Expr, NullBitmap, Predicate, ScalarValue,
};
use columnar_engine::dataset::{decode_counts, disable_decode_counting, enable_decode_counting};
use columnar_engine::{
    execute, Aggregate, AggregateSpec, AggregateValue, Dataset, ExecOptions, QueryPlan,
};
use std::collections::HashMap;

/// The rows the datasets have.
const ROWS: usize = 64;

/// Runs a body with the decode counter on.
fn counting<R>(body: impl FnOnce() -> R) -> (R, HashMap<String, usize>) {
    enable_decode_counting();
    let result = body();
    let counts = decode_counts();
    disable_decode_counting();
    (result, counts)
}

/// A chunk with the given NULL rows.
fn chunk(name: &str, encoding: Encoding, nulls: &[usize]) -> ColumnChunk {
    let len = encoding.len();
    ColumnChunk::new(name, encoding, NullBitmap::with_nulls(len, nulls))
}

/// The value a NULL row holds in the source.
///
/// It is a value no correct sum would hold, so a fold that counted a NULL
/// row would produce a different number -- and be caught.
const NULL_VALUE: f64 = 1e9;

/// A float column whose sum depends on the order the rows are added in.
///
/// The values repeat `0.1, 0.2, 0.3`. In IEEE-754 those three do not add up
/// to the same number in every order: `0.1 + 0.2 + 0.3` is
/// `0.6000000000000001` while `0.3 + 0.2 + 0.1` is `0.6`, and the
/// difference grows as the pattern repeats. A fold that visited the
/// survivors in the wrong order would produce a different number, and the
/// bit comparison would see it.
fn order_sensitive_values() -> Vec<f64> {
    (0..ROWS)
        .map(|row| match row % 3 {
            0 => 0.1,
            1 => 0.2,
            _ => 0.3,
        })
        .collect()
}

/// The rows of the float column that are NULL.
fn float_nulls() -> Vec<usize> {
    (0..ROWS).filter(|row| row % 11 == 0).collect()
}

/// The dataset the tests fold.
///
/// It holds a float column, an integer column, a dictionary-encoded integer
/// column and a dictionary-encoded string column, and the float column has
/// NULLs in it.
fn dataset() -> Dataset {
    let mut floats = order_sensitive_values();
    for row in float_nulls() {
        floats[row] = NULL_VALUE;
    }

    let ints: Vec<i64> = (0..ROWS).map(|row| row as i64).collect();

    let dict: Vec<i64> = (0..11).map(|value| value * 3).collect();
    let indices: Vec<i32> = (0..ROWS).map(|row| (row % 11) as i32).collect();

    let strings: Vec<String> =
        ["alpha", "beta", "gamma", "delta"].iter().map(|s| (*s).to_owned()).collect();
    let string_indices: Vec<i32> = (0..ROWS).map(|row| (row % 4) as i32).collect();

    Dataset::from_chunks(vec![
        chunk("x", Encoding::Float64 { values: floats }, &float_nulls()),
        chunk("i", Encoding::Int64 { values: ints }, &[]),
        chunk("d", Encoding::Int64Dict { dict, indices }, &[]),
        chunk("s", Encoding::Utf8Dict { dict: strings, indices: string_indices }, &[]),
    ])
}

/// The plan's filter: the rows from the third onwards.
///
/// The filter is on a column the aggregate does not read, so the aggregate's
/// column is folded in place over the survivors.
fn filter() -> Vec<Predicate> {
    vec![Predicate::new("i", ComparisonOp::GtEq, ScalarValue::Int64(3))]
}

/// The sum of the survivors of `x`, computed independently of the engine.
///
/// The oracle walks the rows in ascending order and skips the NULL ones,
/// which is what the engine's fold has to do.
fn oracle_sum() -> f64 {
    let dataset = dataset();
    let chunk = &dataset.chunks[0];
    let Encoding::Float64 { values } = &chunk.encoding else {
        panic!("the first column is a float column")
    };
    let mut total = 0.0f64;
    for (row, value) in values.iter().enumerate().take(ROWS).skip(3) {
        if chunk.validity.is_valid(row) {
            total += value;
        }
    }
    total
}

/// The sum of the survivors of `x`, added in the **reverse** order.
///
/// The data is only useful if the order matters, and the test says so: if
/// this equalled [`oracle_sum`], the data would no longer catch a fold that
/// visited the survivors in the wrong order.
fn reversed_oracle_sum() -> f64 {
    let dataset = dataset();
    let chunk = &dataset.chunks[0];
    let Encoding::Float64 { values } = &chunk.encoding else {
        panic!("the first column is a float column")
    };
    let mut total = 0.0f64;
    for (row, value) in values.iter().enumerate().take(ROWS).skip(3).rev() {
        if chunk.validity.is_valid(row) {
            total += value;
        }
    }
    total
}

/// The plan that sums `x`.
fn summing(aggregate: Aggregate) -> QueryPlan {
    QueryPlan::new(
        filter(),
        Vec::new(),
        Some(AggregateSpec { group_by: vec![], aggregates: vec![aggregate] }),
    )
}

/// The sum of the survivors of `x`, as the engine computes it.
fn engine_sum(aggregate: Aggregate) -> AggregateValue {
    let result =
        execute(&summing(aggregate), &dataset(), &ExecOptions::default()).expect("the plan runs");
    assert_eq!(result.groups.len(), 1);
    result.groups[0].values[0].clone()
}

#[test]
fn the_fused_sum_is_the_gathered_sum_bit_for_bit() {
    // The data has to depend on the order the rows are added in, or this
    // test would not catch a fold that visited them in the wrong order.
    assert_ne!(
        oracle_sum().to_bits(),
        reversed_oracle_sum().to_bits(),
        "the data no longer depends on the order the rows are added in"
    );

    // The fused path: a sum over a plain column.
    let fused = engine_sum(Aggregate::Sum { expr: Expr::column("x") });

    // The gathered path: the same sum, over an expression. An expression's
    // columns are gathered, because the expression is evaluated over the
    // survivors -- so this is the old path, unchanged.
    let gathered = engine_sum(Aggregate::Sum {
        expr: Expr::binary(ArithOp::Mul, Expr::column("x"), Expr::constant(1.0f64)),
    });

    assert_eq!(fused, gathered, "the fused sum and the gathered sum differ");
    // And the answer is the one the oracle computes, so neither path is
    // wrong in the same way.
    let AggregateValue::Float(total) = fused else { panic!("a float sum is a float") };
    assert_eq!(total.to_bits(), oracle_sum().to_bits(), "the sum is not the oracle's");
}

#[test]
fn the_fused_sum_skips_the_null_rows() {
    // The float column has a NULL every eleventh row, and the filter's
    // survivors include some of them.
    let dataset = dataset();
    let chunk = &dataset.chunks[0];
    let nulls_in_the_survivors = (3..ROWS).filter(|&row| !chunk.validity.is_valid(row)).count();
    assert!(nulls_in_the_survivors > 0, "the test needs NULL rows among the survivors");

    let fused = engine_sum(Aggregate::Sum { expr: Expr::column("x") });
    let gathered = engine_sum(Aggregate::Sum {
        expr: Expr::binary(ArithOp::Mul, Expr::column("x"), Expr::constant(1.0f64)),
    });
    assert_eq!(fused, gathered);
}

#[test]
fn a_fused_aggregate_materialises_nothing() {
    let (result, counts) = counting(|| {
        execute(
            &summing(Aggregate::Sum { expr: Expr::column("x") }),
            &dataset(),
            &ExecOptions::default(),
        )
        .expect("the plan runs")
    });

    // The column was folded where it lies, so it was never materialised:
    // the projection copied nothing.
    assert!(!counts.contains_key("x"), "the projection materialised the column: {counts:?}");

    // But the query did read it, so the plan output names it. A column the
    // query read is a column the query read.
    assert!(result.plan_output.materialised_columns.contains(&"x".to_owned()));
}

#[test]
fn an_expressions_columns_are_still_gathered() {
    let (result, counts) = counting(|| {
        execute(
            &summing(Aggregate::Sum {
                expr: Expr::binary(ArithOp::Mul, Expr::column("x"), Expr::constant(1.0f64)),
            }),
            &dataset(),
            &ExecOptions::default(),
        )
        .expect("the plan runs")
    });

    // An expression is evaluated over the survivors, so its columns have to
    // be aligned with them -- which means gathered. The fusion does not
    // reach them.
    assert!(counts.contains_key("x"), "the expression's column was not gathered: {counts:?}");
    let AggregateValue::Float(total) = result.groups[0].values[0].clone() else {
        panic!("a float sum is a float")
    };
    assert_eq!(total.to_bits(), oracle_sum().to_bits());
}

#[test]
fn a_grouped_aggregate_still_gathers() {
    // The grouped path needs its keys aligned with its values, and the hash
    // table wants both, so every column is gathered there.
    let plan = QueryPlan::new(
        filter(),
        Vec::new(),
        Some(AggregateSpec {
            group_by: vec!["d".to_owned()],
            aggregates: vec![Aggregate::Sum { expr: Expr::column("x") }],
        }),
    );

    let (result, counts) =
        counting(|| execute(&plan, &dataset(), &ExecOptions::default()).expect("the plan runs"));

    assert!(counts.contains_key("x"), "the grouped path did not gather the column: {counts:?}");
    assert!(counts.contains_key("d"), "the grouped path did not gather the key: {counts:?}");
    assert!(!result.groups.is_empty());
}

#[test]
fn a_column_an_expression_reads_is_still_gathered() {
    // The plan reads `x` twice: plainly, by the first sum, and through the
    // expression, by the second. The expression is evaluated over the
    // survivors, so its column has to be aligned with them -- which means
    // gathered. The plain sum reads the gathered copy too: one column, one
    // copy, and the fusion does not reach it.
    let plan = QueryPlan::new(
        filter(),
        Vec::new(),
        Some(AggregateSpec {
            group_by: vec![],
            aggregates: vec![
                Aggregate::Sum { expr: Expr::column("x") },
                Aggregate::Sum {
                    expr: Expr::binary(ArithOp::Mul, Expr::column("x"), Expr::constant(1.0f64)),
                },
            ],
        }),
    );

    let (result, counts) =
        counting(|| execute(&plan, &dataset(), &ExecOptions::default()).expect("the plan runs"));

    assert!(counts.contains_key("x"), "the expression's column was not gathered: {counts:?}");

    // And the two sums agree: `x * 1.0` is `x`, bit for bit.
    let AggregateValue::Float(plain) = result.groups[0].values[0].clone() else {
        panic!("a float sum is a float")
    };
    let AggregateValue::Float(computed) = result.groups[0].values[1].clone() else {
        panic!("a float sum is a float")
    };
    assert_eq!(plain.to_bits(), computed.to_bits());
    assert_eq!(plain.to_bits(), oracle_sum().to_bits());
}

#[test]
fn an_integer_sum_saturates_in_place_too() {
    // The sum saturates at the `i64` bounds rather than wrapping, so an
    // overflowing sum is loud rather than wrong. A wrapped sum would be
    // `i64::MIN` here, which is a very different answer.
    let values = vec![i64::MAX, 1, i64::MAX];
    let dataset = Dataset::from_chunks(vec![chunk("v", Encoding::Int64 { values }, &[])]);
    let plan = QueryPlan::new(
        Vec::new(),
        Vec::new(),
        Some(AggregateSpec {
            group_by: vec![],
            aggregates: vec![Aggregate::Sum { expr: Expr::column("v") }],
        }),
    );

    let result = execute(&plan, &dataset, &ExecOptions::default()).expect("the plan runs");
    assert_eq!(result.groups[0].values, vec![AggregateValue::Int(i64::MAX)]);
}

#[test]
fn a_dictionary_column_is_folded_through_its_dictionary() {
    // The dictionary has eleven values and the rows point into it in a
    // repeating pattern. A fold that read the wrong dictionary entry would
    // produce a different sum.
    let fused = engine_sum(Aggregate::Sum { expr: Expr::column("d") });
    let gathered = engine_sum(Aggregate::Sum {
        expr: Expr::binary(ArithOp::Mul, Expr::column("d"), Expr::constant(1i64)),
    });
    assert_eq!(fused, gathered, "the fused dictionary sum and the gathered one differ");

    // The oracle: the dictionary's values, added in the survivors' order.
    let dataset = dataset();
    let chunk = &dataset.chunks[2];
    let Encoding::Int64Dict { dict, indices } = &chunk.encoding else {
        panic!("the third column is a dictionary column")
    };
    let mut total = 0i64;
    for &index in indices.iter().take(ROWS).skip(3) {
        let index = usize::try_from(index).expect("a dictionary index is not negative");
        total += dict[index];
    }
    assert_eq!(fused, AggregateValue::Int(total));
}

#[test]
fn a_count_over_a_column_skips_the_null_rows() {
    // `COUNT(x)` counts the survivors that hold a value. The NULL rows are
    // not counted, exactly as they are not counted by the gathered path.
    let dataset = dataset();
    let chunk = &dataset.chunks[0];
    let expected = (3..ROWS).filter(|&row| chunk.validity.is_valid(row)).count();

    let result = execute(
        &summing(Aggregate::Count { column: "x".to_owned() }),
        &dataset,
        &ExecOptions::default(),
    )
    .expect("the plan runs");
    assert_eq!(result.groups[0].values, vec![AggregateValue::Int(expected as i64)]);
}

#[test]
fn a_min_and_a_max_are_folded_in_place_too() {
    let dataset = dataset();
    let chunk = &dataset.chunks[0];
    let survivors: Vec<usize> = (3..ROWS).filter(|&row| chunk.validity.is_valid(row)).collect();
    let values = order_sensitive_values();
    let expected_min = survivors.iter().map(|&row| values[row]).fold(f64::INFINITY, f64::min);
    let expected_max = survivors.iter().map(|&row| values[row]).fold(f64::NEG_INFINITY, f64::max);

    let result = execute(
        &summing(Aggregate::Min { column: "x".to_owned() }),
        &dataset,
        &ExecOptions::default(),
    )
    .expect("the plan runs");
    assert_eq!(result.groups[0].values, vec![AggregateValue::Float(expected_min)]);

    let result = execute(
        &summing(Aggregate::Max { column: "x".to_owned() }),
        &dataset,
        &ExecOptions::default(),
    )
    .expect("the plan runs");
    assert_eq!(result.groups[0].values, vec![AggregateValue::Float(expected_max)]);
}

#[test]
fn the_filtered_column_can_be_the_aggregated_column() {
    // `SELECT sum(x) FROM t WHERE x > 0` reads the same column twice: the
    // filter reads it to decide, and the aggregate folds it. The fusion
    // holds, and the two reads do not interfere.
    let dataset = dataset();
    let plan = QueryPlan::new(
        vec![Predicate::new("x", ComparisonOp::Gt, ScalarValue::Float64(0.15))],
        Vec::new(),
        Some(AggregateSpec {
            group_by: vec![],
            aggregates: vec![Aggregate::Sum { expr: Expr::column("x") }],
        }),
    );

    let result = execute(&plan, &dataset, &ExecOptions::default()).expect("the plan runs");

    let chunk = &dataset.chunks[0];
    let Encoding::Float64 { values } = &chunk.encoding else {
        panic!("the first column is a float column")
    };
    let mut total = 0.0f64;
    for (row, &value) in values.iter().enumerate() {
        // A NULL row never matches, even when its value would: the NULL
        // rows hold a large number here, on purpose.
        if chunk.validity.is_valid(row) && value > 0.15 {
            total += value;
        }
    }
    let AggregateValue::Float(sum) = result.groups[0].values[0].clone() else {
        panic!("a float sum is a float")
    };
    assert_eq!(sum.to_bits(), total.to_bits());
}
