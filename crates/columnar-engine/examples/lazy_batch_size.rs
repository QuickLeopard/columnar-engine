//! The lazy path's batch size, measured for the shape it is used on.
//!
//! This is a diagnostic, not part of the engine. It answers one question:
//! **is the lazy path's batch size the right one for the lazy path's shape?**
//!
//! ```text
//! cargo run --release -p columnar-engine --example lazy_batch_size -- data/lineitem.parquet
//! ```
//!
//! The constant is a *tuning constant*, and the repository's rule for one is
//! that it must be measured for the shape of the work it is used on. The
//! eager path has its own (32,768), measured in spec 008 for sixteen
//! columns. The lazy path reads one to three, so its constant is measured
//! separately -- by this example, which changes nothing but the constant.
//!
//! Two things are measured:
//!
//! 1. a **sweep** over eight batch sizes, and
//! 2. an **interleaved A/B** of the two candidates.
//!
//! The second is there because of the first's weakness. A sweep runs its
//! sizes in order, so a machine that gets busier as the sweep goes on shows
//! up as a trend in the curve — and the trend is indistinguishable from a
//! real effect. The A/B measures the two candidates **against each other**,
//! with the order alternating on every round, so the drift hits both.

use std::path::Path;
use std::time::{Duration, Instant};

use columnar_core::{ComparisonOp, ScalarValue};
use columnar_engine::pushdown::{Pushdown, PushdownPlan};
use columnar_engine::ParquetSource;

/// The file's root index of `l_quantity`.
const QUANTITY: usize = 4;
/// The file's root index of `l_discount`.
const DISCOUNT: usize = 6;

/// The batch sizes to sweep.
const BATCH_SIZES: [usize; 8] = [1_024, 2_048, 4_096, 8_192, 16_384, 32_768, 65_536, 131_072];

/// The two candidates the A/B compares: the constant before this increment,
/// and the constant in the tree now.
const A_B_SIZES: [usize; 2] = [8_192, 65_536];

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

/// The engine's own lazy read, end to end, at one batch size.
fn read(path: &Path, batch_rows: usize) -> (Duration, usize) {
    let plan = plan();
    let source = ParquetSource::open(path).expect("the source opens");
    let started = Instant::now();
    let dataset = source
        .read_with_batch_rows(&[QUANTITY], &plan, columnar_kernels::best(), 1, batch_rows)
        .expect("the read runs");
    let elapsed = started.elapsed();
    let rows = dataset.len();
    std::hint::black_box(&dataset);
    (elapsed, rows)
}

/// An **interleaved** A/B of two batch sizes.
fn a_b(path: &Path) {
    let mut samples: Vec<Vec<Duration>> = A_B_SIZES.iter().map(|_| Vec::new()).collect();
    for round in 0..ITERATIONS {
        // The order alternates, so a round that always measures the first
        // size against a colder machine cannot bias the result.
        let order: [usize; 2] = if round % 2 == 0 { [0, 1] } else { [1, 0] };
        for index in order {
            let batch_rows = A_B_SIZES.get(index).copied().expect("the order names a size");
            let (elapsed, _) = read(path, batch_rows);
            if let Some(bucket) = samples.get_mut(index) {
                bucket.push(elapsed);
            }
        }
    }

    println!("--- the two candidates, interleaved, {ITERATIONS} rounds ---");
    let mut medians = Vec::with_capacity(A_B_SIZES.len());
    for (index, batch_rows) in A_B_SIZES.iter().enumerate() {
        let bucket = samples.get_mut(index).expect("there is a bucket per size");
        let scan_ms = ms(median(bucket));
        medians.push(scan_ms);
        let marker = match *batch_rows {
            8_192 => "  <- the constant before this increment",
            65_536 => "  <- the constant in the tree",
            _ => "",
        };
        println!("{batch_rows:>10}  {scan_ms:8.2} ms{marker}");
    }
    let old = medians.first().copied().unwrap_or_default();
    let new = medians.get(1).copied().unwrap_or_default();
    println!("the difference: {:8.2} ms  ({:+.1}%)", new - old, (new - old) / old * 100.0);
}

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "data/lineitem.parquet".to_owned());
    let path = Path::new(&path);

    println!("file: {}", path.display());
    println!("iterations: {ITERATIONS}\n");
    println!("batch rows    engine scan     rows");
    println!("-------------------------------------");

    for batch_rows in BATCH_SIZES {
        let mut samples = Vec::with_capacity(ITERATIONS);
        let mut rows = 0usize;
        for _ in 0..ITERATIONS {
            let (elapsed, survivors) = read(path, batch_rows);
            samples.push(elapsed);
            rows = survivors;
        }
        let scan_ms = ms(median(&mut samples));
        let marker = match batch_rows {
            8_192 => "  <- the constant before this increment",
            65_536 => "  <- the constant in the tree",
            _ => "",
        };
        println!("{batch_rows:>10}  {scan_ms:8.2} ms  {rows:>8}{marker}");
    }
    println!();
    a_b(path);
}
