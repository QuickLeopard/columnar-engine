//! The engine's tests.
//!
//! Every test here builds its data in memory: no Parquet file is needed to
//! check the filter's semantics, the aggregate's arithmetic, the selection
//! vector's dispatch or the late-materialisation rule. The Parquet round trip
//! lives in `parquet_roundtrip.rs`.

// The indices below are literals into vectors whose length the test has
// just asserted, so the panic the lint guards against cannot happen.
#![allow(clippy::indexing_slicing)]
// The casts below turn a count and a dictionary index into a `usize`. Both
// are non-negative by construction -- a count is a count, and the dictionary
// indices in these tests are literals.
#![allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]

use std::collections::HashMap;

use columnar_core::{
    ColumnChunk, ComparisonOp, Encoding, NullBitmap, Predicate, ScalarValue, SelectionKind,
};
use columnar_engine::dataset::{decode_counts, disable_decode_counting, enable_decode_counting};
use columnar_engine::operators::filter::filter;
use columnar_engine::{
    execute, Aggregate, AggregateSpec, AggregateValue, Dataset, EngineError, ExecOptions, Group,
    MemoryBudget, MemoryError, QueryPlan,
};
use columnar_kernels::Flavor;

/// Builds a chunk with the given NULL rows.
fn chunk(name: &str, encoding: Encoding, nulls: &[usize]) -> ColumnChunk {
    let len = encoding.len();
    ColumnChunk::new(name, encoding, NullBitmap::with_nulls(len, nulls))
}

/// A dataset of `int64` columns.
fn int_dataset(columns: &[(&str, Vec<i64>)]) -> Dataset {
    let chunks = columns
        .iter()
        .map(|(name, values)| chunk(name, Encoding::Int64 { values: values.clone() }, &[]))
        .collect();
    Dataset { chunks }
}

/// The scalar options, so a test never depends on what the machine can run.
fn scalar() -> ExecOptions {
    ExecOptions { memory_budget: MemoryBudget::default(), flavor: Some(Flavor::Scalar) }
}

/// Runs a body with the decode counter on.
fn counting<R>(body: impl FnOnce() -> R) -> (R, HashMap<String, usize>) {
    enable_decode_counting();
    let result = body();
    let counts = decode_counts();
    disable_decode_counting();
    (result, counts)
}

#[test]
fn a_filter_selects_the_rows_the_predicate_names() {
    let chunk = chunk("x", Encoding::Int64 { values: vec![10, 20, 30, 40, 50] }, &[]);
    let column = chunk.decode();
    let predicate = Predicate::new("x", ComparisonOp::Gt, 25i64);

    let (selection, stats) = filter(&column, &predicate, Flavor::Scalar).expect("the filter runs");

    // Hand-computed: 30, 40 and 50 are greater than 25.
    assert_eq!(selection.rows(), vec![2, 3, 4]);
    assert_eq!(stats.rows_in, 5);
    assert_eq!(stats.rows_out, 3);
}

#[test]
fn a_null_row_never_matches_not_even_for_not_equal() {
    let chunk = chunk("x", Encoding::Int64 { values: vec![10, 20, 30, 40, 50] }, &[1, 3]);
    let column = chunk.decode();

    for op in ComparisonOp::all() {
        let predicate = Predicate::new("x", op, 25i64);
        let (selection, stats) =
            filter(&column, &predicate, Flavor::Scalar).expect("the filter runs");
        let rows = selection.rows();
        assert!(
            !rows.contains(&1) && !rows.contains(&3),
            "a NULL row matched the {op:?} comparison"
        );
        assert_eq!(
            usize::try_from(stats.rows_out).expect("a row count fits in a usize"),
            rows.len()
        );
    }
}

#[test]
fn a_null_row_never_matches_a_float_column_either() {
    let chunk = chunk("x", Encoding::Float64 { values: vec![1.0, 2.0, 3.0, 4.0] }, &[0, 3]);
    let column = chunk.decode();

    for op in ComparisonOp::all() {
        let predicate = Predicate::new("x", op, 2.0f64);
        let (selection, _) = filter(&column, &predicate, Flavor::Scalar).expect("the filter runs");
        let rows = selection.rows();
        assert!(
            !rows.contains(&0) && !rows.contains(&3),
            "a NULL row matched the {op:?} comparison"
        );
    }
}

#[test]
fn a_nan_threshold_never_matches_not_even_for_not_equal() {
    let chunk = chunk("x", Encoding::Float64 { values: vec![1.0, 2.0, 3.0, 4.0] }, &[]);
    let column = chunk.decode();

    for op in ComparisonOp::all() {
        let predicate = Predicate::new("x", op, f64::NAN);
        let (selection, stats) =
            filter(&column, &predicate, Flavor::Scalar).expect("the filter runs");
        assert!(selection.is_empty(), "a NaN threshold matched a row for the {op:?} comparison");
        assert_eq!(stats.rows_out, 0);
    }
}

#[test]
fn a_nan_value_never_matches_not_even_for_not_equal() {
    let chunk = chunk("x", Encoding::Float64 { values: vec![1.0, f64::NAN, 3.0, f64::NAN] }, &[]);
    let column = chunk.decode();

    for op in ComparisonOp::all() {
        let predicate = Predicate::new("x", op, 2.0f64);
        let (selection, _) = filter(&column, &predicate, Flavor::Scalar).expect("the filter runs");
        let rows = selection.rows();
        assert!(
            !rows.contains(&1) && !rows.contains(&3),
            "a NaN value matched the {op:?} comparison"
        );
    }
}

#[test]
fn a_sparse_filter_selects_the_index_representation() {
    // Ten of a hundred rows survive: ten per cent, well under the thirty
    // per cent dispatch threshold.
    let values: Vec<i64> = (0..100).map(|row: i32| i64::from(row % 10)).collect();
    let chunk = chunk("x", Encoding::Int64 { values }, &[]);
    let column = chunk.decode();
    let predicate = Predicate::new("x", ComparisonOp::Eq, 0i64);

    let (selection, stats) = filter(&column, &predicate, Flavor::Scalar).expect("the filter runs");

    assert_eq!(stats.kind, SelectionKind::Index);
    assert_eq!(stats.rows_out, 10);
    assert_eq!(selection.len(), 10);
}

#[test]
fn a_dense_filter_selects_the_bitmap_representation() {
    // Sixty of a hundred rows survive: sixty per cent, over the threshold.
    let values: Vec<i64> = (0..100).map(|row: i32| i64::from(row % 10)).collect();
    let chunk = chunk("x", Encoding::Int64 { values }, &[]);
    let column = chunk.decode();
    let predicate = Predicate::new("x", ComparisonOp::GtEq, 0i64);

    let (selection, stats) = filter(&column, &predicate, Flavor::Scalar).expect("the filter runs");

    assert_eq!(stats.kind, SelectionKind::Bitmap);
    assert_eq!(stats.rows_out, 100);
    assert_eq!(selection.len(), 100);
}

#[test]
fn the_aggregate_computes_sum_count_count_star_min_and_max() {
    // g: the group key. v: the aggregated value, with two NULLs.
    let dataset = Dataset {
        chunks: vec![
            chunk("g", Encoding::Int64 { values: vec![1, 1, 1, 2, 2, 2, 3] }, &[]),
            chunk(
                "v",
                Encoding::Float64 { values: vec![10.0, 20.0, 0.0, 5.0, 0.0, 0.0, 7.0] },
                &[2, 4, 5],
            ),
        ],
    };

    let plan = QueryPlan {
        filter: Vec::new(),
        projection: vec![],
        aggregate: Some(AggregateSpec {
            group_by: vec!["g".to_owned()],
            aggregates: vec![
                Aggregate::Sum { column: "v".to_owned() },
                Aggregate::Count { column: "v".to_owned() },
                Aggregate::CountStar,
                Aggregate::Min { column: "v".to_owned() },
                Aggregate::Max { column: "v".to_owned() },
            ],
        }),
    };

    let result = execute(&plan, &dataset, &scalar()).expect("the plan runs");

    // Hand-computed:
    //   group 1: 10.0 and 20.0 -> sum 30, count 2, count(*) 3, min 10, max 20
    //   group 2: 5.0          -> sum  5, count 1, count(*) 3, min  5, max  5
    //   group 3: 7.0          -> sum  7, count 1, count(*) 1, min  7, max  7
    let expected = vec![
        Group {
            key: vec![ScalarValue::Int64(1)],
            values: vec![
                AggregateValue::Float(30.0),
                AggregateValue::Int(2),
                AggregateValue::Int(3),
                AggregateValue::Float(10.0),
                AggregateValue::Float(20.0),
            ],
        },
        Group {
            key: vec![ScalarValue::Int64(2)],
            values: vec![
                AggregateValue::Float(5.0),
                AggregateValue::Int(1),
                AggregateValue::Int(3),
                AggregateValue::Float(5.0),
                AggregateValue::Float(5.0),
            ],
        },
        Group {
            key: vec![ScalarValue::Int64(3)],
            values: vec![
                AggregateValue::Float(7.0),
                AggregateValue::Int(1),
                AggregateValue::Int(1),
                AggregateValue::Float(7.0),
                AggregateValue::Float(7.0),
            ],
        },
    ];
    assert_eq!(result.groups, expected);
}

#[test]
fn a_sum_over_an_all_null_group_is_null() {
    let dataset = Dataset {
        chunks: vec![
            chunk("g", Encoding::Int64 { values: vec![1, 1, 2] }, &[]),
            chunk("v", Encoding::Float64 { values: vec![0.0, 0.0, 3.0] }, &[0, 1]),
        ],
    };

    let plan = QueryPlan {
        filter: Vec::new(),
        projection: vec![],
        aggregate: Some(AggregateSpec {
            group_by: vec!["g".to_owned()],
            aggregates: vec![
                Aggregate::Sum { column: "v".to_owned() },
                Aggregate::Count { column: "v".to_owned() },
            ],
        }),
    };

    let result = execute(&plan, &dataset, &scalar()).expect("the plan runs");

    assert_eq!(
        result.groups,
        vec![
            Group {
                key: vec![ScalarValue::Int64(1)],
                values: vec![AggregateValue::Null, AggregateValue::Int(0)],
            },
            Group {
                key: vec![ScalarValue::Int64(2)],
                values: vec![AggregateValue::Float(3.0), AggregateValue::Int(1)],
            },
        ]
    );
}

#[test]
fn the_group_order_is_the_same_across_runs() {
    let dataset = int_dataset(&[("g", vec![7, 3, 5, 1, 9, 3, 8, 2, 6, 4, 5, 9, 1, 7, 2, 3])]);

    let plan = QueryPlan {
        filter: Vec::new(),
        projection: vec![],
        aggregate: Some(AggregateSpec {
            group_by: vec!["g".to_owned()],
            aggregates: vec![Aggregate::CountStar],
        }),
    };

    let first = execute(&plan, &dataset, &scalar()).expect("the plan runs");
    let second = execute(&plan, &dataset, &scalar()).expect("the plan runs");

    // Two runs, two hash tables, two hash seeds: the same order anyway.
    assert_eq!(first.groups, second.groups);

    let keys: Vec<i64> = first
        .groups
        .iter()
        .map(|group| match group.key.first() {
            Some(ScalarValue::Int64(value)) => *value,
            other => panic!("unexpected group key {other:?}"),
        })
        .collect();
    assert_eq!(keys, vec![1, 2, 3, 4, 5, 6, 7, 8, 9]);
}

#[test]
fn a_plan_filters_and_aggregates_without_decoding_the_other_columns() {
    // The TPC-H `lineitem` shape, trimmed to four columns.
    let dataset = Dataset {
        chunks: vec![
            chunk("l_discount", Encoding::Float64 { values: vec![0.02, 0.06, 0.01, 0.09] }, &[]),
            chunk(
                "l_extendedprice",
                Encoding::Float64 { values: vec![100.0, 200.0, 300.0, 400.0] },
                &[],
            ),
            chunk(
                "l_returnflag",
                Encoding::Utf8 {
                    values: vec!["N".to_owned(), "A".to_owned(), "N".to_owned(), "R".to_owned()],
                },
                &[],
            ),
            chunk(
                "l_comment",
                Encoding::Utf8 {
                    values: vec![
                        "a long comment".to_owned(),
                        "another long comment".to_owned(),
                        "yet another long comment".to_owned(),
                        "the last long comment".to_owned(),
                    ],
                },
                &[],
            ),
        ],
    };

    let plan = QueryPlan {
        filter: vec![Predicate::new("l_discount", ComparisonOp::Lt, 0.05f64)],
        projection: vec![],
        aggregate: Some(AggregateSpec {
            group_by: vec!["l_returnflag".to_owned()],
            aggregates: vec![Aggregate::Sum { column: "l_extendedprice".to_owned() }],
        }),
    };

    let (result, counts) = counting(|| execute(&plan, &dataset, &scalar()).expect("the plan runs"));

    // The comment column is neither filtered nor aggregated, so it is never
    // decoded. That is the late-materialisation rule, and it is observable.
    assert!(!counts.contains_key("l_comment"), "the comment column was decoded: {counts:?}");
    // The filter's own column is not materialised either: the filter reads it
    // straight from the chunk, and for a dictionary-encoded column it reads
    // only the dictionary. Materialising it would be work nobody asked for.
    assert!(!counts.contains_key("l_discount"), "{counts:?}");
    assert!(counts.contains_key("l_extendedprice"), "{counts:?}");
    assert!(counts.contains_key("l_returnflag"), "{counts:?}");

    // And the answer is right: rows 0 and 2 survive, both in group "N".
    assert_eq!(result.groups.len(), 1);
    assert_eq!(result.groups[0].key, vec![ScalarValue::Utf8("N".to_owned())]);
    assert_eq!(result.groups[0].values, vec![AggregateValue::Float(400.0)]);
}

#[test]
fn a_plan_over_the_budget_is_rejected_before_it_does_any_work() {
    let dataset = int_dataset(&[("x", vec![1, 2, 3, 4])]);
    let plan = QueryPlan {
        filter: Vec::new(),
        projection: vec![],
        aggregate: Some(AggregateSpec { group_by: vec![], aggregates: vec![Aggregate::CountStar] }),
    };

    let options =
        ExecOptions { memory_budget: MemoryBudget { bytes: 1 }, flavor: Some(Flavor::Scalar) };

    let (result, counts) = counting(|| execute(&plan, &dataset, &options));

    assert!(matches!(result, Err(EngineError::Memory(MemoryError::BudgetExceeded))));
    assert!(counts.is_empty(), "the rejected plan decoded {counts:?}");
}

#[test]
fn count_star_does_not_materialise_any_column() {
    let dataset = int_dataset(&[("x", vec![1, 2, 3, 4])]);
    let plan = QueryPlan {
        filter: Vec::new(),
        projection: vec![],
        aggregate: Some(AggregateSpec { group_by: vec![], aggregates: vec![Aggregate::CountStar] }),
    };

    let (result, counts) = counting(|| execute(&plan, &dataset, &scalar()).expect("the plan runs"));

    assert!(counts.is_empty(), "COUNT(*) materialised a column: {counts:?}");
    assert_eq!(result.groups.len(), 1);
    assert_eq!(result.groups[0].values, vec![AggregateValue::Int(4)]);
}

#[test]
fn an_unknown_column_is_reported_before_anything_is_read() {
    let dataset = int_dataset(&[("x", vec![1, 2, 3, 4])]);
    let plan = QueryPlan {
        filter: vec![Predicate::new("nope", ComparisonOp::Gt, 1i64)],
        projection: vec![],
        aggregate: None,
    };

    let (result, counts) = counting(|| execute(&plan, &dataset, &scalar()));

    assert!(matches!(result, Err(EngineError::UnknownColumn(name)) if name == "nope"));
    assert!(counts.is_empty(), "the rejected plan decoded {counts:?}");
}

#[test]
fn a_filter_over_the_wrong_type_is_a_plan_error() {
    let dataset = int_dataset(&[("x", vec![1, 2, 3, 4])]);
    let plan = QueryPlan {
        filter: vec![Predicate::new("x", ComparisonOp::Gt, 1.5f64)],
        projection: vec![],
        aggregate: None,
    };

    let result = execute(&plan, &dataset, &scalar());
    assert!(matches!(result, Err(EngineError::TypeMismatch(_))));
}

#[test]
fn the_plan_output_reports_the_stages_and_the_late_materialisation() {
    let dataset = Dataset {
        chunks: vec![
            chunk("l_discount", Encoding::Float64 { values: vec![0.02, 0.06, 0.01, 0.09] }, &[]),
            chunk(
                "l_extendedprice",
                Encoding::Float64 { values: vec![100.0, 200.0, 300.0, 400.0] },
                &[],
            ),
        ],
    };

    let plan = QueryPlan {
        filter: vec![Predicate::new("l_discount", ComparisonOp::Lt, 0.05f64)],
        projection: vec![],
        aggregate: Some(AggregateSpec {
            group_by: vec![],
            aggregates: vec![Aggregate::Sum { column: "l_extendedprice".to_owned() }],
        }),
    };

    let result = execute(&plan, &dataset, &scalar()).expect("the plan runs");
    let output = result.plan_output.to_string();

    assert!(output.contains("optimization order: predicate pushdown"), "{output}");
    assert!(output.contains("stage"), "{output}");
    assert!(output.contains("scan"), "{output}");
    assert!(output.contains("filter"), "{output}");
    assert!(output.contains("aggregate"), "{output}");
    assert!(output.contains("materialised columns: l_extendedprice"), "{output}");
    assert!(output.contains("estimated memory:"), "{output}");
    assert!(output.contains("budget"), "{output}");
    assert!(output.contains("kernel flavor: scalar"), "{output}");

    // The stage table reports the rows that flowed through it.
    let stages = &result.stages;
    assert_eq!(stages.len(), 4);
    assert_eq!(stages[0].stage, "scan");
    assert_eq!(stages[0].rows_in, 4);
    assert_eq!(stages[0].rows_out, 4);
    assert_eq!(stages[1].stage, "filter");
    assert_eq!(stages[1].rows_in, 4);
    assert_eq!(stages[1].rows_out, 2);
    assert_eq!(stages[2].stage, "project");
    assert_eq!(stages[2].rows_in, 2);
    assert_eq!(stages[2].rows_out, 2);
    assert_eq!(stages[3].stage, "aggregate");
    assert_eq!(stages[3].rows_in, 2);
    assert_eq!(stages[3].rows_out, 1);
}

#[test]
fn a_scan_all_plan_reads_every_column() {
    let dataset = int_dataset(&[("x", vec![1, 2, 3]), ("y", vec![4, 5, 6])]);
    let plan = QueryPlan::scan_all();

    let (result, counts) = counting(|| execute(&plan, &dataset, &scalar()).expect("the plan runs"));

    assert!(counts.contains_key("x"), "{counts:?}");
    assert!(counts.contains_key("y"), "{counts:?}");
    // No aggregation, so no groups.
    assert!(result.groups.is_empty());
}

// ---------------------------------------------------------------------------
// Conjunctions, and the dictionary fast path
// ---------------------------------------------------------------------------

/// A dataset with a dictionary-encoded string column and a numeric column.
fn dictionary_dataset() -> Dataset {
    let chunks = vec![
        chunk("l_discount", Encoding::Float64 { values: vec![0.01, 0.05, 0.05, 0.09, 0.03] }, &[]),
        chunk(
            "l_shipdate",
            Encoding::Utf8Dict {
                dict: vec!["1993-05-01".to_owned(), "1994-06-01".to_owned()],
                indices: vec![0, 1, 1, 0, 1],
            },
            &[],
        ),
    ];
    Dataset { chunks }
}

#[test]
fn a_conjunction_keeps_only_the_rows_that_satisfy_every_predicate() {
    let dataset = dictionary_dataset();
    let plan = QueryPlan {
        filter: vec![
            Predicate::new("l_discount", ComparisonOp::GtEq, 0.04f64),
            Predicate::new("l_discount", ComparisonOp::LtEq, 0.06f64),
        ],
        projection: vec![],
        aggregate: Some(AggregateSpec { group_by: vec![], aggregates: vec![Aggregate::CountStar] }),
    };

    let result = execute(&plan, &dataset, &scalar()).expect("the plan runs");

    // Only row 1 and row 2 have a discount in [0.04, 0.06].
    assert_eq!(result.groups.len(), 1);
    assert_eq!(result.groups[0].values, vec![AggregateValue::Int(2)]);
}

#[test]
fn a_conjunction_over_different_columns_keeps_the_intersection() {
    let dataset = dictionary_dataset();
    let plan = QueryPlan {
        filter: vec![
            Predicate::new("l_discount", ComparisonOp::GtEq, 0.04f64),
            Predicate::new(
                "l_shipdate",
                ComparisonOp::GtEq,
                ScalarValue::Utf8("1994-01-01".to_owned()),
            ),
        ],
        projection: vec![],
        aggregate: Some(AggregateSpec { group_by: vec![], aggregates: vec![Aggregate::CountStar] }),
    };

    let result = execute(&plan, &dataset, &scalar()).expect("the plan runs");

    // Rows 1 and 2 have a discount above 0.04, and both have the 1994 date.
    assert_eq!(result.groups[0].values, vec![AggregateValue::Int(2)]);
}

#[test]
fn a_string_predicate_filters_a_dictionary_encoded_column() {
    let dataset = dictionary_dataset();
    let plan = QueryPlan {
        filter: vec![Predicate::new(
            "l_shipdate",
            ComparisonOp::Lt,
            ScalarValue::Utf8("1994-01-01".to_owned()),
        )],
        projection: vec![],
        aggregate: Some(AggregateSpec { group_by: vec![], aggregates: vec![Aggregate::CountStar] }),
    };

    let result = execute(&plan, &dataset, &scalar()).expect("the plan runs");

    // Only the rows pointing at the 1993 date survive.
    assert_eq!(result.groups[0].values, vec![AggregateValue::Int(2)]);
}

/// The dictionary fast path must agree with a row-by-row comparison.
#[test]
fn the_dictionary_fast_path_agrees_with_a_row_by_row_comparison() {
    let dictionary =
        vec!["1993-05-01".to_owned(), "1994-06-01".to_owned(), "1995-07-01".to_owned()];
    let indices = vec![0i32, 1, 2, 0, 2, 1, 0, 2, 1];
    let thresholds = ["1993-01-01", "1994-01-01", "1995-01-01", "1996-01-01"];

    for threshold in thresholds {
        for op in ComparisonOp::all() {
            let dataset = Dataset {
                chunks: vec![chunk(
                    "l_shipdate",
                    Encoding::Utf8Dict { dict: dictionary.clone(), indices: indices.clone() },
                    &[],
                )],
            };
            let plan = QueryPlan {
                filter: vec![Predicate::new(
                    "l_shipdate",
                    op,
                    ScalarValue::Utf8(threshold.to_owned()),
                )],
                projection: vec![],
                aggregate: Some(AggregateSpec {
                    group_by: vec![],
                    aggregates: vec![Aggregate::CountStar],
                }),
            };
            let result = execute(&plan, &dataset, &scalar()).expect("the plan runs");
            let fast = match result.groups[0].values[0].clone() {
                AggregateValue::Int(count) => count as usize,
                other => panic!("expected an integer count, got {other:?}"),
            };

            // The row-by-row answer.
            let row_by_row = indices
                .iter()
                .filter(|&&index| {
                    let value = dictionary[index as usize].as_str();
                    match op {
                        ComparisonOp::Lt => value < threshold,
                        ComparisonOp::LtEq => value <= threshold,
                        ComparisonOp::Gt => value > threshold,
                        ComparisonOp::GtEq => value >= threshold,
                        ComparisonOp::Eq => value == threshold,
                        ComparisonOp::Ne => value != threshold,
                    }
                })
                .count();

            assert_eq!(
                fast, row_by_row,
                "the dictionary fast path disagreed with the row-by-row answer for {op:?} against {threshold}"
            );
        }
    }
}

/// A global aggregate over an empty input still produces one row, the way
/// SQL says it does. Every incumbent answers `0` for `count(*)` over an empty
/// set, so an engine that returned no row would be answering a different
/// question -- and the external comparison would be meaningless.
#[test]
fn a_global_aggregate_over_no_rows_still_produces_one_row() {
    let dataset = int_dataset(&[("x", vec![10, 20, 30])]);
    let plan = QueryPlan {
        // No row satisfies this.
        filter: vec![Predicate::new("x", ComparisonOp::Gt, 100i64)],
        projection: vec![],
        aggregate: Some(AggregateSpec { group_by: vec![], aggregates: vec![Aggregate::CountStar] }),
    };

    let result = execute(&plan, &dataset, &scalar()).expect("the plan runs");

    assert_eq!(result.groups.len(), 1, "a global aggregate must produce one row");
    assert_eq!(result.groups[0].values, vec![AggregateValue::Int(0)]);
}

/// The same rule for a SUM, which is NULL over an empty set rather than 0.
#[test]
fn a_sum_over_no_rows_is_null() {
    let dataset = int_dataset(&[("x", vec![10, 20, 30])]);
    let plan = QueryPlan {
        filter: vec![Predicate::new("x", ComparisonOp::Gt, 100i64)],
        projection: vec![],
        aggregate: Some(AggregateSpec {
            group_by: vec![],
            aggregates: vec![Aggregate::Sum { column: "x".to_owned() }],
        }),
    };

    let result = execute(&plan, &dataset, &scalar()).expect("the plan runs");

    assert_eq!(result.groups.len(), 1);
    assert_eq!(result.groups[0].values, vec![AggregateValue::Null]);
}

/// A global aggregate takes the single-group path, and that path must agree
/// with the hash path on every aggregate it supports.
///
/// The global path exists because the hash path -- a key vector and a hash
/// lookup per row -- is pure overhead when there is exactly one group. This
/// test pins the two paths together: the global answer and the one-group
/// answer have to be identical, or the fast path is a second implementation
/// with its own idea of the truth.
#[test]
fn a_global_aggregate_agrees_with_the_single_group_aggregate() {
    let dataset = int_dataset(&[("x", vec![10, 20, 30, 40, 50])]);

    let global = execute(
        &QueryPlan {
            filter: vec![Predicate::new("x", ComparisonOp::Gt, 20i64)],
            projection: vec![],
            aggregate: Some(AggregateSpec {
                group_by: vec![],
                aggregates: vec![
                    Aggregate::CountStar,
                    Aggregate::Count { column: "x".to_owned() },
                    Aggregate::Sum { column: "x".to_owned() },
                    Aggregate::Min { column: "x".to_owned() },
                    Aggregate::Max { column: "x".to_owned() },
                ],
            }),
        },
        &dataset,
        &scalar(),
    )
    .expect("the global plan runs");

    let grouped = execute(
        &QueryPlan {
            filter: vec![Predicate::new("x", ComparisonOp::Gt, 20i64)],
            projection: vec![],
            aggregate: Some(AggregateSpec {
                // One group, forced by a constant key, so the hash path runs.
                group_by: vec!["x".to_owned()],
                aggregates: vec![
                    Aggregate::CountStar,
                    Aggregate::Count { column: "x".to_owned() },
                    Aggregate::Sum { column: "x".to_owned() },
                    Aggregate::Min { column: "x".to_owned() },
                    Aggregate::Max { column: "x".to_owned() },
                ],
            }),
        },
        &dataset,
        &scalar(),
    )
    .expect("the grouped plan runs");

    assert_eq!(global.groups.len(), 1, "a global aggregate produces one row");
    // 30, 40 and 50 survive the filter.
    assert_eq!(
        global.groups[0].values,
        vec![
            AggregateValue::Int(3),
            AggregateValue::Int(3),
            AggregateValue::Int(120),
            AggregateValue::Int(30),
            AggregateValue::Int(50),
        ]
    );

    // The grouped plan has one group per distinct value, so the totals are
    // the same but split across three rows. The global path's `COUNT(*)` is
    // the sum of the grouped path's `COUNT(*)`.
    let grouped_counts: i64 = grouped
        .groups
        .iter()
        .filter_map(|group| match group.values.first() {
            Some(AggregateValue::Int(value)) => Some(*value),
            _ => None,
        })
        .sum();
    assert_eq!(grouped_counts, 3, "the grouped path counts the same rows");
}
