//! Reports which SIMD path is live, and proves it by counting rows.
//!
//! This exists for `scripts/verify-avx2.sh`. The script needs to show two
//! things about an ARM64 Mac under Rosetta 2:
//!
//! 1. without the override, the probe correctly reports that there is no
//!    AVX2 path here -- the host is an ARM64 chip, and CPUID says so;
//! 2. with `COLUMNAR_FORCE_SIMD=avx2`, the AVX2 path is live **and its
//!    instructions actually ran**, which the row counter proves.
//!
//! A report of "the path is available" would not be enough. The counter
//! moves only if the AVX2 code really executed, and the mask it produced is
//! checked against the oracle, so the two together say what is actually
//! being claimed: the path ran, and it ran correctly.
//!
//! Run with `--nocapture` to see the report.

use columnar_core::ComparisonOp;
use columnar_kernels::force::forced;
use columnar_kernels::{avx2_rows_processed, compare_f64_into, oracle, Flavor};

#[test]
fn report_the_live_simd_path() {
    let values: Vec<f64> = (0..1024).map(|i| f64::from(i % 13)).collect();
    let threshold = 6.0;

    let mut expected = vec![0u64; values.len().div_ceil(64)];
    let expected_count =
        oracle::compare_f64(&values, ComparisonOp::GtEq, threshold, None, &mut expected);

    let before = avx2_rows_processed();
    let mut actual = vec![0u64; values.len().div_ceil(64)];
    let count = compare_f64_into(
        Flavor::CppSimd,
        &values,
        ComparisonOp::GtEq,
        threshold,
        None,
        &mut actual,
    );
    let avx2_rows = avx2_rows_processed() - before;

    println!("forced        : {}", forced().name());
    println!("simd_kind     : {}", columnar_kernels::simd_kind().name());
    println!("avx2 rows     : {avx2_rows}");
    println!("matches oracle: {}", actual == expected && count == expected_count);

    assert_eq!(actual, expected, "the live SIMD path disagreed with the oracle");
    assert_eq!(count, expected_count);
}
