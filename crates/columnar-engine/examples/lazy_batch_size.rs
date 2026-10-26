//! The lazy path's reader batch size, measured for the shape it is used on.
//!
//! This is a diagnostic, not part of the engine. It answers one question:
//! **is the lazy path's reader batch size the right one for the lazy path's
//! shape?**
//!
//! ```text
//! cargo run --release -p columnar-engine --example lazy_batch_size -- data/lineitem.parquet \
//!     --out bench-results/batch-size-run1.json
//! ```
//!
//! The limit is a *tuning constant*, and the repository's rule for one is
//! that it must be measured for the shape of the work it is used on. The
//! eager path has its own (32,768), measured in spec 008 for sixteen
//! columns. The lazy path reads one to three, so its constant is measured
//! separately -- by this example, which changes nothing but the constant.
//!
//! Two things are measured:
//!
//! 1. a **sweep** over the limits, and
//! 2. an **interleaved A/B** of the two candidates.
//!
//! The second is there because of the first's weakness. A sweep runs its
//! sizes in order, so a machine that gets busier as the sweep goes on shows
//! up as a trend in the curve -- and the trend is indistinguishable from a
//! real effect. The A/B measures the two candidates **against each other**,
//! with the order alternating on every round, so the drift hits both.
//!
//! **A limit is not a batch size.** The reader's batch size is the largest
//! divisor of the file's row groups' row count that fits under the limit, so
//! that no row group ends with a partial batch (ADR-0023). The sweep therefore
//! reports the batch each limit derives, and the two candidates are the batch
//! the tree used before spec 016 -- a fixed 65,536 rows, which leaves a
//! partial batch at the end of every row group -- and the batch the tree
//! derives now.

use std::path::Path;
use std::time::{Duration, Instant};

use columnar_core::{ComparisonOp, ScalarValue};
use columnar_engine::pushdown::{Pushdown, PushdownPlan};
use columnar_engine::{ParquetSource, MAX_READER_BATCH_ROWS};

/// The file's root index of `l_quantity`.
const QUANTITY: usize = 4;
/// The file's root index of `l_discount`.
const DISCOUNT: usize = 6;
/// The file's root index of `l_shipdate`.
const SHIPDATE: usize = 10;

/// The limits to sweep.
const LIMITS: [usize; 9] = [1_024, 2_048, 4_096, 8_192, 16_384, 32_768, 65_536, 131_072, 262_144];

/// The batch size the tree used before spec 016: a fixed 65,536 rows.
const OLD_BATCH_ROWS: usize = 65_536;

/// How many times each measurement is repeated.
const ITERATIONS: usize = 9;

/// Milliseconds, for printing.
fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// The median of a set of durations.
fn median(samples: &mut [Duration]) -> Duration {
    samples.sort_unstable();
    // The sample set is never empty: the caller filled it with
    // `ITERATIONS` samples before it got here.
    #[allow(clippy::indexing_slicing)]
    let middle = samples[samples.len() / 2];
    middle
}

/// The predicates, as the engine pushes them.
fn plan() -> PushdownPlan {
    let mut plan = PushdownPlan::new();
    let mut discount =
        Pushdown::new(DISCOUNT, "l_discount", ComparisonOp::GtEq, ScalarValue::Float64(0.04));
    discount.and(ComparisonOp::LtEq, ScalarValue::Float64(0.07));
    plan.push(discount);
    plan
}

/// The real Q6's predicate set, as the engine pushes it.
///
/// This is the shape the **published** measurements run: two predicate
/// columns, one of them a dictionary-encoded string, and nothing
/// materialised.
fn q6_plan() -> PushdownPlan {
    let mut plan = PushdownPlan::new();
    let mut discount =
        Pushdown::new(DISCOUNT, "l_discount", ComparisonOp::GtEq, ScalarValue::Float64(0.04));
    discount.and(ComparisonOp::LtEq, ScalarValue::Float64(0.07));
    plan.push(discount);

    let mut shipdate = Pushdown::new(
        SHIPDATE,
        "l_shipdate",
        ComparisonOp::GtEq,
        ScalarValue::Utf8("1994-01-01".to_owned()),
    );
    shipdate.and(ComparisonOp::Lt, ScalarValue::Utf8("1995-01-01".to_owned()));
    plan.push(shipdate);
    plan
}

/// The engine's own lazy read, end to end, at one batch size.
///
/// `materialised` names the columns the query keeps; the predicates are the
/// plan's.
fn read(
    path: &Path,
    batch_rows: usize,
    plan: &PushdownPlan,
    materialised: &[usize],
) -> (Duration, usize) {
    let source = ParquetSource::open(path).expect("the source opens");
    let started = Instant::now();
    let dataset = source
        .read_with_batch_rows(materialised, plan, columnar_kernels::best(), 1, batch_rows)
        .expect("the read runs");
    let elapsed = started.elapsed();
    let rows = dataset.len();
    std::hint::black_box(&dataset);
    (elapsed, rows)
}

/// An **interleaved** A/B of two batch sizes.
fn a_b(
    path: &Path,
    old: usize,
    new: usize,
    plan: &PushdownPlan,
    materialised: &[usize],
) -> Vec<f64> {
    let sizes = [old, new];
    let mut samples: Vec<Vec<Duration>> = sizes.iter().map(|_| Vec::new()).collect();
    for round in 0..ITERATIONS {
        // The order alternates, so a round that always measures the first
        // size against a colder machine cannot bias the result.
        let order: [usize; 2] = if round % 2 == 0 { [0, 1] } else { [1, 0] };
        for index in order {
            let batch_rows = sizes.get(index).copied().expect("the order names a size");
            let (elapsed, _) = read(path, batch_rows, plan, materialised);
            if let Some(bucket) = samples.get_mut(index) {
                bucket.push(elapsed);
            }
        }
    }

    println!("--- the two candidates, interleaved, {ITERATIONS} rounds ---");
    let mut medians = Vec::with_capacity(sizes.len());
    for (index, batch_rows) in sizes.iter().enumerate() {
        let bucket = samples.get_mut(index).expect("there is a bucket per size");
        let scan_ms = ms(median(bucket));
        medians.push(scan_ms);
        let marker = match *batch_rows {
            65_536 => "  <- the fixed batch size before spec 016",
            _ if *batch_rows == new => "  <- the batch the tree derives now",
            _ => "",
        };
        println!("{batch_rows:>10}  {scan_ms:8.2} ms{marker}");
    }
    let old_ms = medians.first().copied().unwrap_or_default();
    let new_ms = medians.get(1).copied().unwrap_or_default();
    println!(
        "the difference: {:8.2} ms  ({:+.1}%)",
        new_ms - old_ms,
        (new_ms - old_ms) / old_ms * 100.0
    );
    medians
}

/// The commit the working tree is at, for the report.
fn commit() -> String {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success());
    output.map_or_else(
        || "unknown".to_owned(),
        |output| String::from_utf8_lossy(&output.stdout).trim().to_owned(),
    )
}

/// The host's load average, for the report.
fn load_average() -> f64 {
    std::process::Command::new("sysctl")
        .args(["-n", "vm.loadavg"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| {
            String::from_utf8_lossy(&output.stdout)
                .split_whitespace()
                .next()
                .and_then(|value| value.parse::<f64>().ok())
        })
        .unwrap_or_default()
}

fn main() {
    let mut file = "data/lineitem.parquet".to_owned();
    let mut out: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = args.next(),
            _ => file = arg,
        }
    }
    let path = Path::new(&file);

    let source = ParquetSource::open(path).expect("the source opens");
    let current = source.batch_rows_under(MAX_READER_BATCH_ROWS);

    println!("file: {}", path.display());
    println!("iterations: {ITERATIONS}\n");

    // The first shape is the lazy path's smallest: one materialised column and
    // one predicate. The second is the real Q6's: two predicate columns, one
    // of them a dictionary-encoded string, and nothing materialised.
    let shapes: [(&str, PushdownPlan, &[usize]); 2] = [
        ("one column, one predicate", plan(), &[QUANTITY]),
        ("the real Q6's predicate set", q6_plan(), &[]),
    ];

    let mut reported: Vec<serde_json::Value> = Vec::with_capacity(shapes.len());

    for (name, plan, materialised) in shapes {
        println!("=== {name} ===");
        println!("    limit      batch     engine scan     rows");
        println!("------------------------------------------------");

        let mut limits: Vec<serde_json::Value> = Vec::with_capacity(LIMITS.len());
        for limit in LIMITS {
            let batch_rows = source.batch_rows_under(limit);
            let mut samples = Vec::with_capacity(ITERATIONS);
            let mut rows = 0usize;
            for _ in 0..ITERATIONS {
                let (elapsed, survivors) = read(path, batch_rows, &plan, materialised);
                samples.push(elapsed);
                rows = survivors;
            }
            let scan_ms = ms(median(&mut samples));
            let marker =
                if limit == MAX_READER_BATCH_ROWS { "  <- the limit in the tree" } else { "" };
            println!("{limit:>10} {batch_rows:>10}  {scan_ms:8.2} ms  {rows:>8}{marker}");
            limits.push(serde_json::json!({
                "limit": limit,
                "batch": batch_rows,
                "scan_ms": scan_ms,
                "rows": rows,
            }));
        }
        println!();
        let candidates = a_b(path, OLD_BATCH_ROWS, current, &plan, materialised);
        println!();
        reported.push(serde_json::json!({
            "shape": name,
            "limits": limits,
            "candidates": [
                { "batch": OLD_BATCH_ROWS, "scan_ms": candidates.first().copied().unwrap_or_default() },
                { "batch": current, "scan_ms": candidates.get(1).copied().unwrap_or_default() },
            ],
        }));
    }

    if let Some(out) = out {
        let report = serde_json::json!({
            "probe": "batch-size",
            "file": path.display().to_string(),
            "commit": commit(),
            "iterations": ITERATIONS,
            "host_load_average": load_average(),
            "old_batch_rows": OLD_BATCH_ROWS,
            "limit_in_the_tree": MAX_READER_BATCH_ROWS,
            "batch_in_the_tree": current,
            "shapes": reported,
        });
        std::fs::write(&out, serde_json::to_string_pretty(&report).expect("the report is written"))
            .expect("the report is written");
        println!("report written to {out}");
    }
}
