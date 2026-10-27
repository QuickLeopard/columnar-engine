//! The sort, against a reference sort.
//!
//! The reference is the rule the engine follows, written out: the key
//! decides, the row's own position breaks the tie, a NULL key sorts last, and
//! a limit keeps the first rows of that order. The engine has to agree with
//! it, value for value.
//!
//! The order is observable because the values are chosen so that the sum
//! depends on it: `1e16 + 1.0` rounds to `1e16`, so the sum of those three
//! values is zero in one order and one in another. A sort that reordered the
//! rows would answer differently, bit for bit.

// The indices below are literals into vectors whose length the test has just
// asserted, so the panic the lint guards against cannot happen.
#![allow(clippy::indexing_slicing)]

use std::cmp::Ordering;
use std::path::Path;

use columnar_core::{Encoding, Expr, NullBitmap};
use columnar_engine::{
    execute, Aggregate, AggregateSpec, AggregateValue, Dataset, ExecOptions, MemoryBudget,
    QueryPlan, SortSpec,
};
use columnar_kernels::Flavor;

/// The scalar options, so a test never depends on what the machine can run.
fn scalar() -> ExecOptions {
    ExecOptions {
        memory_budget: MemoryBudget::default(),
        flavor: Some(Flavor::Scalar),
        ..ExecOptions::default()
    }
}

/// A column chunk with the given NULL rows.
fn chunk(name: &str, encoding: Encoding, nulls: &[usize]) -> columnar_core::ColumnChunk {
    let len = encoding.len();
    columnar_core::ColumnChunk::new(name, encoding, NullBitmap::with_nulls(len, nulls))
}

/// The rows of a column that hold no value.
fn null_rows<T>(values: &[Option<T>]) -> Vec<usize> {
    values.iter().enumerate().filter_map(|(row, value)| value.is_none().then_some(row)).collect()
}

/// A dataset of a sort key and a value.
fn dataset(keys: &[Option<f64>], values: &[Option<f64>]) -> Dataset {
    Dataset::from_chunks(vec![
        chunk(
            "k",
            Encoding::Float64 { values: keys.iter().map(|key| key.unwrap_or_default()).collect() },
            &null_rows(keys),
        ),
        chunk(
            "v",
            Encoding::Float64 {
                values: values.iter().map(|value| value.unwrap_or_default()).collect(),
            },
            &null_rows(values),
        ),
    ])
}

/// The sort's plan.
///
/// The aggregate is over the sorted rows: the plan carries a sort and an
/// aggregation, and the stage order is the plan's.
fn sort_plan(column: &str, descending: bool, limit: Option<usize>) -> QueryPlan {
    QueryPlan {
        filter: Vec::new(),
        projection: Vec::new(),
        aggregate: Some(AggregateSpec {
            group_by: Vec::new(),
            aggregates: vec![Aggregate::CountStar, Aggregate::Sum { expr: Expr::column("v") }],
        }),
        join: None,
        sort: Some(SortSpec { column: column.to_owned(), descending, limit }),
    }
}

/// The rows a sort keeps, by the reference rule.
///
/// The key decides, and the row's own position breaks the tie: the order is
/// total, so the answer does not depend on the sort's algorithm. A NULL key
/// sorts after every key that holds a value, in both directions.
fn reference_sort(keys: &[Option<f64>], descending: bool, limit: Option<usize>) -> Vec<usize> {
    let mut rows: Vec<usize> = (0..keys.len()).collect();
    rows.sort_by(|left, right| {
        let ordering = match (keys[*left], keys[*right]) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(left), Some(right)) => {
                let ordering = left.total_cmp(&right);
                if descending {
                    ordering.reverse()
                } else {
                    ordering
                }
            }
        };
        match ordering {
            Ordering::Equal => left.cmp(right),
            other => other,
        }
    });
    if let Some(limit) = limit {
        rows.truncate(limit);
    }
    rows
}

/// The sum of the kept rows, in the order they were sorted.
fn reference_sum(values: &[Option<f64>], rows: &[usize]) -> Option<f64> {
    let mut sum = 0.0;
    let mut seen = false;
    for &row in rows {
        if let Some(value) = values[row] {
            sum += value;
            seen = true;
        }
    }
    seen.then_some(sum)
}

/// The engine's answer: how many rows the sort kept, and their sum.
fn engine_answer(plan: &QueryPlan, dataset: &Dataset) -> (i64, Option<f64>) {
    let result = execute(plan, dataset, &scalar()).expect("the plan runs");
    let group = result.groups.first().expect("a global aggregate has one group");
    let count = match group.values.first() {
        Some(AggregateValue::Int(count)) => *count,
        other => panic!("unexpected count {other:?}"),
    };
    let sum = match group.values.get(1) {
        Some(AggregateValue::Float(sum)) => Some(*sum),
        Some(AggregateValue::Null) => None,
        other => panic!("unexpected sum {other:?}"),
    };
    (count, sum)
}

/// A dataset of an **integer** sort key and an integer value.
fn integer_dataset(keys: &[i64], values: &[i64]) -> Dataset {
    Dataset::from_chunks(vec![
        chunk("k", Encoding::Int64 { values: keys.to_vec() }, &[]),
        chunk("v", Encoding::Int64 { values: values.to_vec() }, &[]),
    ])
}

/// The rows a sort keeps, by the reference rule, over integer keys.
///
/// The same rule as [`reference_sort`], with the keys compared as integers.
fn reference_sort_int(keys: &[i64], descending: bool, limit: Option<usize>) -> Vec<usize> {
    let mut rows: Vec<usize> = (0..keys.len()).collect();
    rows.sort_by(|left, right| {
        let ordering = keys[*left].cmp(&keys[*right]);
        let ordering = if descending { ordering.reverse() } else { ordering };
        match ordering {
            Ordering::Equal => left.cmp(right),
            other => other,
        }
    });
    if let Some(limit) = limit {
        rows.truncate(limit);
    }
    rows
}

/// The sum of the kept integer rows.
///
/// `None` when no rows are kept: a sum over nothing is a NULL, and the
/// engine says so.
fn reference_sum_int(values: &[i64], rows: &[usize]) -> Option<i64> {
    let mut sum = 0_i64;
    for &row in rows {
        sum += values[row];
    }
    (!rows.is_empty()).then_some(sum)
}

/// The engine's answer over an integer column: how many rows the sort kept,
/// and their sum.
fn engine_answer_int(plan: &QueryPlan, dataset: &Dataset) -> (i64, Option<i64>) {
    let result = execute(plan, dataset, &scalar()).expect("the plan runs");
    let group = result.groups.first().expect("a global aggregate has one group");
    let count = match group.values.first() {
        Some(AggregateValue::Int(count)) => *count,
        other => panic!("unexpected count {other:?}"),
    };
    let sum = match group.values.get(1) {
        Some(AggregateValue::Int(sum)) => Some(*sum),
        Some(AggregateValue::Null) => None,
        other => panic!("unexpected sum {other:?}"),
    };
    (count, sum)
}

/// The sort agrees with the reference over an **integer** column.
///
/// The published query sorts a float, and every other test here sorts a
/// float, so the engine's integer path -- the one that reads the column's
/// own integers and compares them as integers, with no enum and no
/// `Option` -- would otherwise never be run.
#[test]
fn the_sort_agrees_with_the_reference_over_integers() {
    let keys = vec![3_i64, 1, 4, 2, 1];
    let values = vec![10_i64, 20, 30, 40, 50];
    let dataset = integer_dataset(&keys, &values);

    for descending in [false, true] {
        for limit in [None, Some(0), Some(3), Some(9)] {
            let plan = sort_plan("k", descending, limit);
            let (count, sum) = engine_answer_int(&plan, &dataset);
            let rows = reference_sort_int(&keys, descending, limit);
            let expected = reference_sum_int(&values, &rows);

            assert_eq!(
                count,
                i64::try_from(rows.len()).expect("a row count fits in an i64"),
                "the sort kept the wrong number of rows (descending: {descending}, limit: {limit:?})"
            );
            assert_eq!(
                sum, expected,
                "the sort's rows differ from the reference's (descending: {descending}, limit: {limit:?})"
            );
        }
    }

    // The sums the reference produced, written out, so the test above cannot
    // pass by accident: the tie between the two `1`s is broken by the row's
    // own number, and the sum depends on it.
    assert_eq!(reference_sum_int(&values, &reference_sort_int(&keys, false, None)), Some(150));
    assert_eq!(reference_sum_int(&values, &reference_sort_int(&keys, true, None)), Some(150));
}

/// The sort agrees with the reference, ascending and descending.
///
/// The values are order-sensitive, and deliberately so: their sum is 1.0
/// ascending, 0.0 descending and 2.0 in the order the rows came in. A sort
/// that did not sort, or that sorted the wrong way, would answer with a
/// different number -- and not with a number that is *nearly* the right one.
#[test]
fn the_sort_agrees_with_the_reference() {
    let keys = vec![Some(3.0), Some(1.0), Some(4.0), Some(2.0)];
    let values = vec![Some(1e16), Some(-1e16), Some(1.0), Some(1.0)];
    let dataset = dataset(&keys, &values);

    for descending in [false, true] {
        for limit in [None, Some(3)] {
            let plan = sort_plan("k", descending, limit);
            let (count, sum) = engine_answer(&plan, &dataset);
            let rows = reference_sort(&keys, descending, limit);
            let expected = reference_sum(&values, &rows);

            assert_eq!(
                count,
                i64::try_from(rows.len()).expect("a row count fits in an i64"),
                "the sort kept the wrong number of rows (descending: {descending}, limit: {limit:?})"
            );
            assert_eq!(
                sum, expected,
                "the sort's rows differ from the reference's (descending: {descending}, limit: {limit:?})"
            );
        }
    }

    // The sums the reference produced, written out: the order is visible in
    // them, so the test above cannot pass by accident.
    assert_eq!(reference_sum(&values, &reference_sort(&keys, false, None)), Some(1.0));
    assert_eq!(reference_sum(&values, &reference_sort(&keys, true, None)), Some(0.0));
    assert_eq!(reference_sum(&values, &[0, 1, 2, 3]), Some(2.0));
}

/// Ties are broken by the row's own position.
///
/// The keys are all equal, so the order is decided entirely by the tie
/// break -- and the values are order-sensitive, so the tie break is visible
/// in the sum.
#[test]
fn ties_are_broken_by_the_row_index() {
    let keys = vec![Some(1.0), Some(1.0), Some(1.0), Some(1.0)];
    let values = vec![Some(1e16), Some(-1e16), Some(1.0), Some(1.0)];
    let dataset = dataset(&keys, &values);

    for descending in [false, true] {
        let plan = sort_plan("k", descending, None);
        let (count, sum) = engine_answer(&plan, &dataset);
        let rows = reference_sort(&keys, descending, None);
        let expected = reference_sum(&values, &rows);

        assert_eq!(count, 4, "the sort lost a row");
        assert_eq!(
            sum, expected,
            "the ties were broken in another order (descending: {descending})"
        );
    }
}

/// A NULL key sorts last, in both directions.
#[test]
fn a_null_key_sorts_last() {
    let keys = vec![Some(2.0), None, Some(1.0), None];
    let values = vec![Some(1e16), Some(-1e16), Some(1.0), Some(1.0)];
    let dataset = dataset(&keys, &values);

    for descending in [false, true] {
        let plan = sort_plan("k", descending, None);
        let (count, sum) = engine_answer(&plan, &dataset);
        let rows = reference_sort(&keys, descending, None);
        let expected = reference_sum(&values, &rows);

        assert_eq!(count, 4, "the sort lost a row");
        assert_eq!(sum, expected, "the NULL keys were not sorted last");
    }
}

/// An empty input sorts to an empty output.
#[test]
fn an_empty_input_sorts_to_nothing() {
    let dataset = dataset(&[], &[]);

    for descending in [false, true] {
        for limit in [None, Some(10)] {
            let plan = sort_plan("k", descending, limit);
            let (count, sum) = engine_answer(&plan, &dataset);

            assert_eq!(count, 0, "an empty input produced rows");
            assert_eq!(sum, None, "an empty input produced a sum");
        }
    }
}

/// A limit larger than the input keeps everything.
#[test]
fn a_limit_larger_than_the_input_keeps_everything() {
    let keys = vec![Some(3.0), Some(1.0), Some(2.0)];
    let values = vec![Some(30.0), Some(10.0), Some(20.0)];
    let dataset = dataset(&keys, &values);

    for descending in [false, true] {
        let plan = sort_plan("k", descending, Some(100));
        let (count, sum) = engine_answer(&plan, &dataset);
        let rows = reference_sort(&keys, descending, Some(100));
        let expected = reference_sum(&values, &rows);

        assert_eq!(count, 3, "the limit dropped rows it could not afford to drop");
        assert_eq!(sum, expected, "the rows differ from the reference's");
    }
}

/// The limit keeps the **best** rows, not the first ones.
///
/// The limit is what makes the sort a bounded heap rather than a sort of
/// everything, so it has to keep the same rows a full sort would have put
/// first.
#[test]
fn the_limit_keeps_the_best_rows() {
    let keys = vec![Some(1.0), Some(9.0), Some(5.0), Some(7.0), Some(3.0)];
    let values = vec![Some(10.0), Some(90.0), Some(50.0), Some(70.0), Some(30.0)];
    let dataset = dataset(&keys, &values);

    // Descending, the two best rows are the ones with the largest keys.
    let plan = sort_plan("k", true, Some(2));
    let (count, sum) = engine_answer(&plan, &dataset);
    assert_eq!(count, 2, "the limit kept the wrong number of rows");
    assert_eq!(sum, Some(160.0), "the limit kept the wrong rows: {sum:?}");

    // Ascending, they are the two smallest.
    let plan = sort_plan("k", false, Some(2));
    let (count, sum) = engine_answer(&plan, &dataset);
    assert_eq!(count, 2, "the limit kept the wrong number of rows");
    assert_eq!(sum, Some(40.0), "the limit kept the wrong rows: {sum:?}");
}

/// A limit of zero keeps nothing.
#[test]
fn a_limit_of_zero_keeps_nothing() {
    let keys = vec![Some(1.0), Some(2.0)];
    let values = vec![Some(10.0), Some(20.0)];
    let dataset = dataset(&keys, &values);

    let plan = sort_plan("k", false, Some(0));
    let (count, sum) = engine_answer(&plan, &dataset);

    assert_eq!(count, 0, "a limit of zero kept rows");
    assert_eq!(sum, None, "a limit of zero produced a sum");
}

/// The plan output names the sort's key and its limit.
#[test]
fn the_plan_output_names_the_sort() {
    let keys = vec![Some(1.0), Some(2.0)];
    let values = vec![Some(10.0), Some(20.0)];
    let dataset = dataset(&keys, &values);

    let plan = sort_plan("k", true, Some(10));
    let result = execute(&plan, &dataset, &scalar()).expect("the plan runs");
    let report = result.plan_output.sort.as_ref().expect("the plan output reports the sort");

    assert_eq!(report.column, "k");
    assert!(report.descending, "the plan output does not say the order was descending");
    assert_eq!(report.limit, Some(10), "the plan output does not name the limit");
    assert_eq!(report.rows, 2, "the plan output does not count the rows the sort kept");

    // The rendered plan names the sort's key, its direction and its limit.
    let rendered = result.plan_output.to_string();
    assert!(rendered.contains("sort: k DESC LIMIT 10"), "the plan output is {rendered}");
}

/// The published sort query runs, and answers what the SQL says.
///
/// The query is the engine's own: `sort-topk`, the ten most expensive line
/// items, summed. The sort keeps ten rows out of the whole file, and the
/// aggregate is over those ten -- not over all of them.
#[test]
fn the_published_sort_query_runs() {
    // Twelve line items, with prices that are not in order.
    let prices: Vec<Option<f64>> = vec![
        Some(10.0),
        Some(50.0),
        Some(20.0),
        Some(70.0),
        Some(30.0),
        Some(60.0),
        Some(40.0),
        Some(80.0),
        Some(5.0),
        Some(90.0),
        Some(15.0),
        Some(25.0),
    ];
    let definition = columnar_engine::queries::QUERIES
        .iter()
        .find(|definition| definition.query == columnar_engine::BuiltinQuery::SortTopK)
        .expect("the sort query is a builtin query");
    let plan = (definition.plan)(Path::new("unused"));

    // The plan reads `l_extendedprice`, so the dataset holds it under that
    // name.
    let dataset = Dataset::from_chunks(vec![chunk(
        "l_extendedprice",
        Encoding::Float64 {
            values: prices.iter().map(|price| price.unwrap_or_default()).collect(),
        },
        &null_rows(&prices),
    )]);

    let result = execute(&plan, &dataset, &scalar()).expect("the plan runs");
    let sum = match result.groups.first().map(|group| group.values.first()) {
        Some(Some(AggregateValue::Float(sum))) => *sum,
        other => panic!("unexpected aggregate {other:?}"),
    };

    // The ten largest of the twelve: 90, 80, 70, 60, 50, 40, 30, 25, 20 and
    // 15.
    let expected: f64 = 90.0 + 80.0 + 70.0 + 60.0 + 50.0 + 40.0 + 30.0 + 25.0 + 20.0 + 15.0;
    assert_eq!(sum.to_bits(), expected.to_bits(), "the sort kept the wrong rows: {sum}");
}
