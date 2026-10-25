//! The kernel A/B: the project's headline measurement.
//!
//! The measurement is deliberately simple, because a complicated benchmark is
//! a benchmark whose result cannot be explained:
//!
//! 1. take a column of `f64` values from the real dataset,
//! 2. run the comparison kernel over it, once per flavor,
//! 3. report rows per second and nanoseconds per row.
//!
//! ## Two baselines, not one
//!
//! The `scalar` flavor is the C++ branchless scalar reference, compiled with
//! `-fno-vectorize` so that it really is scalar. It computes the same
//! predicate in the same shape as the SIMD path, so
//! `ratio_vs_scalar` measures vectorisation and not a difference in
//! algorithm. A speedup measured against a naive branching baseline would be
//! a claim about branch prediction, not about SIMD.
//!
//! The `cpp-auto` flavor is the *same source* compiled with ordinary flags.
//! The compiler vectorises it, and it turns out to be a strong baseline --
//! on this machine it beats the hand-written NEON kernel. `ratio_vs_auto`
//! answers the question that actually matters: does the hand-written kernel
//! beat what the compiler does with the same loop? A number below `1.0`
//! says it does not.
//!
//! Publishing only the first ratio would have produced a headline speedup
//! that credited SIMD for work the compiler was already doing. See ADR-0006.
//!
//! ## The harness checks itself
//!
//! Every flavor must produce the same number of matching rows. The A/B
//! asserts this, so a flavor that silently computes a different predicate
//! fails the benchmark rather than publishing a faster wrong number.
//!
//! ## The FFI seam is measured, not assumed
//!
//! The `rust-simd` flavor runs the same predicate through `std::arch`
//! intrinsics with no FFI at all. Comparing it against `cpp-simd` is what
//! turns "the FFI seam might be the bottleneck" into a number. Review
//! finding 2.12 asked for exactly this measurement.

// The row counts are converted to `f64` for the nanoseconds-per-row
// arithmetic. A row count beyond 2^53 would lose precision in the ratio,
// which is not a number this benchmark can reach.
#![allow(clippy::cast_precision_loss)]

use std::collections::BTreeMap;
use std::time::Instant;

use columnar_core::ComparisonOp;
use columnar_kernels::Flavor;

/// The result of measuring one flavor.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct KernelFlavorResult {
    /// The flavor's name.
    pub flavor: String,
    /// Rows processed per second.
    pub rows_per_sec: f64,
    /// Nanoseconds per row.
    pub ns_per_row: f64,
    /// The number of rows that matched. Every flavor must agree on this.
    pub matches: u64,
    /// The ratio against the **genuinely scalar** baseline
    /// (`-fno-vectorize`): how much is vectorisation worth?
    pub ratio_vs_scalar: f64,
    /// The ratio against the **compiler-vectorised** baseline: does the
    /// hand-written kernel beat what the compiler does with the same loop?
    ///
    /// This is the ratio that decides whether the hand-written SIMD kernel
    /// is worth its complexity. A number below `1.0` says it is not.
    pub ratio_vs_auto: f64,
}

/// The result of the whole A/B.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct KernelAb {
    /// The number of rows in one pass over the data.
    pub rows: usize,
    /// The number of timed passes.
    pub iterations: usize,
    /// The column that was filtered.
    ///
    /// A kernel number without the column it was measured on is not a
    /// complete number: the values' distribution decides how well the
    /// branches predict, and the column's encoding decides how much of the
    /// data the kernel actually has to touch.
    pub column: String,
    /// The operator that was applied.
    pub operator: String,
    /// The threshold that was compared against.
    pub threshold: f64,
    /// One result per flavor.
    pub flavors: Vec<KernelFlavorResult>,
}

impl KernelAb {
    /// Runs the A/B over `values`.
    ///
    /// `values` is the column being filtered. The same slice is passed to
    /// every flavor, and the measurement loop reuses one output buffer, so
    /// the comparison is between the kernels and not between the allocators.
    ///
    /// # Panics
    ///
    /// Panics if the flavors disagree on the number of matching rows, or if
    /// the measured baseline is not slower than zero.
    ///
    /// It also panics if the SIMD probe is overridden -- see
    /// [`crate::machine::FORCED_SIMD_REFUSAL`]. A timing taken with an
    /// override describes an emulator rather than hardware, so this refuses
    /// to produce one.
    #[must_use]
    pub fn measure(
        column: &str,
        values: &[f64],
        op: ComparisonOp,
        threshold: f64,
        iterations: usize,
    ) -> Self {
        assert!(
            !crate::machine::simd_probe_is_overridden(),
            "{}",
            crate::machine::FORCED_SIMD_REFUSAL
        );
        let iterations = iterations.max(1);
        let mut buffer = vec![0u64; values.len().div_ceil(64)];

        let mut results = Vec::new();
        let mut scalar_ns_per_row = 0.0_f64;
        let mut auto_ns_per_row = 0.0_f64;

        for flavor in Flavor::all() {
            let (ns_per_row, matches) =
                Self::time_one(values, op, threshold, iterations, flavor, &mut buffer);

            if flavor == Flavor::Scalar {
                scalar_ns_per_row = ns_per_row;
            }
            if flavor == Flavor::CppAuto {
                auto_ns_per_row = ns_per_row;
            }

            results.push(KernelFlavorResult {
                flavor: flavor.name().to_owned(),
                rows_per_sec: if ns_per_row > 0.0 {
                    1_000_000_000.0 / ns_per_row
                } else {
                    f64::INFINITY
                },
                ns_per_row,
                matches,
                ratio_vs_scalar: 0.0, // filled in below
                ratio_vs_auto: 0.0,   // filled in below
            });
        }

        // The harness checks itself: every flavor must agree on the number of
        // matching rows. A flavor that computes a different predicate would
        // otherwise publish a faster, wrong number.
        if let Some(first) = results.first() {
            for result in &results {
                assert_eq!(
                    result.matches, first.matches,
                    "flavor `{}` matched {} rows but `{}` matched {} -- the flavors do not agree",
                    result.flavor, result.matches, first.flavor, first.matches
                );
            }
        }

        for result in &mut results {
            result.ratio_vs_scalar =
                if scalar_ns_per_row > 0.0 { scalar_ns_per_row / result.ns_per_row } else { 1.0 };
            result.ratio_vs_auto =
                if auto_ns_per_row > 0.0 { auto_ns_per_row / result.ns_per_row } else { 1.0 };
        }

        Self {
            rows: values.len(),
            iterations,
            column: column.to_owned(),
            operator: op.symbol().to_owned(),
            threshold,
            flavors: results,
        }
    }

    /// One independent sample per timed block, per flavor.
    ///
    /// The A/B reports one number per flavor; the performance gate needs the
    /// *samples*, because a statistical test compares distributions and a
    /// median of each side would throw away exactly the information the test
    /// is about. A "block" is the same shape as one A/B measurement -- a
    /// warm-up pass and then `iterations` timed passes -- so a block is a
    /// stable measurement and the blocks are what vary.
    ///
    /// The samples are rows per second, which is the direction a regression
    /// lies in: lower is slower.
    ///
    /// # Panics
    ///
    /// Panics if the SIMD probe is overridden -- see
    /// [`crate::machine::FORCED_SIMD_REFUSAL`] -- or if a block's number of
    /// matching rows disagrees with another block's.
    #[must_use]
    pub fn measure_samples(
        values: &[f64],
        op: ComparisonOp,
        threshold: f64,
        iterations: usize,
        blocks: usize,
    ) -> BTreeMap<String, Vec<f64>> {
        assert!(
            !crate::machine::simd_probe_is_overridden(),
            "{}",
            crate::machine::FORCED_SIMD_REFUSAL
        );
        let iterations = iterations.max(1);
        let blocks = blocks.max(1);
        let mut buffer = vec![0u64; values.len().div_ceil(64)];

        let mut samples: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        for flavor in Flavor::all() {
            let mut per_block = Vec::with_capacity(blocks);
            let mut expected: Option<u64> = None;
            for _ in 0..blocks {
                let (ns_per_row, matches) =
                    Self::time_one(values, op, threshold, iterations, flavor, &mut buffer);

                // The harness checks itself: every block must agree on the
                // number of matching rows. A flavor that computed a
                // different predicate would otherwise publish a faster,
                // wrong number.
                match expected {
                    None => expected = Some(matches),
                    Some(previous) => assert_eq!(
                        matches,
                        previous,
                        "flavor `{}` matched {matches} rows in one block and {previous} in another",
                        flavor.name()
                    ),
                }

                per_block.push(if ns_per_row > 0.0 {
                    1_000_000_000.0 / ns_per_row
                } else {
                    f64::INFINITY
                });
            }
            samples.insert(flavor.name().to_owned(), per_block);
        }
        samples
    }

    /// Times one flavor and returns (ns per row, matches).
    fn time_one(
        values: &[f64],
        op: ComparisonOp,
        threshold: f64,
        iterations: usize,
        flavor: Flavor,
        buffer: &mut [u64],
    ) -> (f64, u64) {
        // Warm-up pass: the first call into the C++ translation unit pays for
        // the dynamic loader's relocation work and the branch predictor's
        // cold state. A benchmark that includes that measures the loader.
        buffer.fill(0);
        columnar_kernels::compare_f64_into(flavor, values, op, threshold, None, buffer);

        let mut matches = 0_u64;
        let start = Instant::now();
        for _ in 0..iterations {
            buffer.fill(0);
            matches =
                columnar_kernels::compare_f64_into(flavor, values, op, threshold, None, buffer)
                    as u64;
        }
        let elapsed = start.elapsed();

        let total_rows = (values.len() * iterations) as f64;
        let ns_per_row = elapsed.as_nanos() as f64 / total_rows.max(1.0);
        (ns_per_row, matches)
    }
}
