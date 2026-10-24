//! Differential tests: every implementation must agree with the oracle.
//!
//! This crate contains five implementations of one predicate:
//!
//! 1. the Rust oracle (`columnar_kernels::oracle`),
//! 2. the C++ scalar reference over the FFI seam,
//! 3. the C++ SIMD path over the FFI seam (`NEON` on `AArch64`, `AVX2` on x86-64),
//! 4. the Rust `std::arch` intrinsics path,
//! 5. and, indirectly, the C++ SIMD path's own internal fallback.
//!
//! This file asserts that all of them produce the same mask, bit for bit, on
//! random inputs. A SIMD kernel that is 10% faster but disagrees on one row
//! in a million is not a faster engine; it is a bug with a speed number
//! attached.
//!
//! The properties that are easiest to get wrong, and are therefore tested
//! explicitly rather than left to the random cases:
//!
//! - a NULL row never matches, **including for `!=`**;
//! - a NaN never matches, **including for `!=`**;
//! - the tail block (fewer than 64 remaining rows) is handled;
//! - an empty input is handled.

// Test code indexes small, locally-built vectors by construction. The
// workspace lint stays on for the library itself.
#![allow(clippy::indexing_slicing, clippy::cast_sign_loss, clippy::cast_precision_loss)]

use columnar_core::{ComparisonOp, NullBitmap};
use columnar_kernels::{compare_f64_into, compare_i64_into, oracle, Flavor};
use proptest::prelude::*;

/// Every flavor this machine can run, including the always-present scalar.
fn flavors() -> Vec<Flavor> {
    columnar_kernels::available()
}

fn op_from(index: u8) -> ComparisonOp {
    ComparisonOp::all()[usize::from(index) % ComparisonOp::all().len()]
}

/// A validity mask with a controllable null rate.
fn mask(len: usize, null_rate: f64, seed: u64) -> NullBitmap {
    if null_rate <= 0.0 {
        return NullBitmap::all_valid(len);
    }
    let mut nulls = Vec::new();
    let mut state = seed | 1;
    for row in 0..len {
        // xorshift64* -- deterministic, and independent of the `rand` ban.
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let draw = (state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64;
        if draw < null_rate {
            nulls.push(row);
        }
    }
    NullBitmap::with_nulls(len, &nulls)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, ..ProptestConfig::default() })]

    /// Every flavour must produce exactly the oracle's mask for int64 data.
    #[test]
    fn flavours_agree_with_the_oracle_for_i64(
        values in prop::collection::vec(any::<i64>(), 0..600),
        threshold in any::<i64>(),
        op_index in any::<u8>(),
    ) {
        let op = op_from(op_index);
        let mut expected = vec![0u64; values.len().div_ceil(64)];
        let expected_count = oracle::compare_i64(&values, op, threshold, None, &mut expected);

        for flavor in flavors() {
            let mut actual = vec![0u64; values.len().div_ceil(64)];
            let count = compare_i64_into(flavor, &values, op, threshold, None, &mut actual);
            prop_assert_eq!(&actual, &expected, "flavor {:?} disagreed on the mask", flavor);
            prop_assert_eq!(count, expected_count, "flavor {:?} disagreed on the count", flavor);
        }
    }

    /// Every flavour must produce exactly the oracle's mask for float64 data.
    #[test]
    fn flavours_agree_with_the_oracle_for_f64(
        values in prop::collection::vec(any::<f64>(), 0..600),
        threshold in any::<f64>(),
        op_index in any::<u8>(),
    ) {
        let op = op_from(op_index);
        let mut expected = vec![0u64; values.len().div_ceil(64)];
        let expected_count = oracle::compare_f64(&values, op, threshold, None, &mut expected);

        for flavor in flavors() {
            let mut actual = vec![0u64; values.len().div_ceil(64)];
            let count = compare_f64_into(flavor, &values, op, threshold, None, &mut actual);
            prop_assert_eq!(&actual, &expected, "flavor {:?} disagreed on the mask", flavor);
            prop_assert_eq!(count, expected_count, "flavor {:?} disagreed on the count", flavor);
        }
    }

    /// The same, with NULLs mixed in.
    #[test]
    fn flavours_agree_with_the_oracle_with_nulls(
        values in prop::collection::vec(any::<f64>(), 0..600),
        threshold in any::<f64>(),
        op_index in any::<u8>(),
        null_rate in 0.0f64..1.0f64,
        seed in any::<u64>(),
    ) {
        let op = op_from(op_index);
        let validity = mask(values.len(), null_rate, seed);

        let mut expected = vec![0u64; values.len().div_ceil(64)];
        let expected_count =
            oracle::compare_f64(&values, op, threshold, Some(&validity), &mut expected);

        for flavor in flavors() {
            let mut actual = vec![0u64; values.len().div_ceil(64)];
            let count =
                compare_f64_into(flavor, &values, op, threshold, Some(&validity), &mut actual);
            prop_assert_eq!(&actual, &expected, "flavor {:?} disagreed on the mask", flavor);
            prop_assert_eq!(count, expected_count, "flavor {:?} disagreed on the count", flavor);
        }
    }
}

/// Lengths around the 64-row block boundary, where a tail bug hides.
#[test]
fn block_boundaries_are_handled() {
    for len in [0usize, 1, 2, 63, 64, 65, 127, 128, 129, 191, 192, 193] {
        let values: Vec<f64> = (0..len).map(|i| (i % 7) as f64 - 3.0).collect();
        let validity = NullBitmap::all_valid(len);
        for op in ComparisonOp::all() {
            let mut expected = vec![0u64; len.div_ceil(64)];
            let expected_count =
                oracle::compare_f64(&values, op, 0.0, Some(&validity), &mut expected);
            for flavor in flavors() {
                let mut actual = vec![0u64; len.div_ceil(64)];
                let count =
                    compare_f64_into(flavor, &values, op, 0.0, Some(&validity), &mut actual);
                assert_eq!(actual, expected, "len {len}, op {op:?}, flavor {flavor:?}");
                assert_eq!(count, expected_count, "len {len}, op {op:?}, flavor {flavor:?}");
            }
        }
    }
}

/// A NULL row must never match -- including under `!=`.
#[test]
fn a_null_row_never_matches_even_under_not_equal() {
    let len = 128;
    let values: Vec<f64> = vec![5.0; len];
    for null_row in [0usize, 1, 63, 64, 127] {
        let validity = NullBitmap::with_nulls(len, &[null_row]);
        for op in ComparisonOp::all() {
            // The expected count depends on the operator -- `5.0 < 5.0` is
            // false, `5.0 <= 5.0` is true -- so it comes from the oracle
            // rather than from an assumption.
            let mut expected = vec![0u64; len.div_ceil(64)];
            let expected_count =
                oracle::compare_f64(&values, op, 5.0, Some(&validity), &mut expected);

            for flavor in flavors() {
                let mut out = vec![0u64; len.div_ceil(64)];
                let count = compare_f64_into(flavor, &values, op, 5.0, Some(&validity), &mut out);
                let matched = out[null_row / 64] & (1u64 << (null_row % 64));
                assert_eq!(
                    matched, 0,
                    "row {null_row} matched under {op:?} on {flavor:?} but it is NULL"
                );
                assert_eq!(count, expected_count, "row {null_row} under {op:?} on {flavor:?}");
            }
        }
    }
}

/// A NaN must never match -- including under `!=`.
///
/// This is the rule that is easiest to get wrong, because `NaN != 5.0` is
/// `true` in IEEE 754 and in Rust. The engine's rule is stricter: unknown
/// propagates, so a NaN cannot satisfy a filter.
#[test]
fn a_nan_never_matches_even_under_not_equal() {
    let len = 128;
    let mut values = vec![5.0f64; len];
    values[77] = f64::NAN;

    for op in ComparisonOp::all() {
        for flavor in flavors() {
            let mut out = vec![0u64; len.div_ceil(64)];
            compare_f64_into(flavor, &values, op, 5.0, None, &mut out);
            let matched = out[1] & (1u64 << (77 - 64));
            assert_eq!(matched, 0, "the NaN row matched under {op:?} on {flavor:?}");
        }
    }
}

/// A NaN *threshold* must never match anything.
#[test]
fn a_nan_threshold_matches_nothing() {
    let len = 128;
    let values = vec![5.0f64; len];
    for op in ComparisonOp::all() {
        for flavor in flavors() {
            let mut out = vec![0u64; len.div_ceil(64)];
            let count = compare_f64_into(flavor, &values, op, f64::NAN, None, &mut out);
            assert_eq!(count, 0, "a NaN threshold matched under {op:?} on {flavor:?}");
        }
    }
}

/// An empty input produces an empty mask and a zero count.
#[test]
fn an_empty_input_is_handled() {
    for flavor in flavors() {
        let mut out: Vec<u64> = Vec::new();
        let count = compare_i64_into(flavor, &[], ComparisonOp::Eq, 0, None, &mut out);
        assert_eq!(count, 0);
        assert!(out.is_empty());
    }
}

/// The match mask must never set a bit beyond the end of the input.
#[test]
fn no_bit_is_set_beyond_the_end_of_the_input() {
    for len in [1usize, 5, 63, 64, 65, 130] {
        let values = vec![1i64; len];
        for flavor in flavors() {
            let mut out = vec![0u64; len.div_ceil(64)];
            compare_i64_into(flavor, &values, ComparisonOp::Eq, 1, None, &mut out);
            for row in len..out.len() * 64 {
                assert_eq!(
                    out[row / 64] & (1u64 << (row % 64)),
                    0,
                    "row {row} was set beyond the end of a {len}-row input on {flavor:?}"
                );
            }
        }
    }
}

/// A forced SIMD path must actually have executed.
///
/// This is the test that makes the `COLUMNAR_FORCE_SIMD` override worth
/// having. The differential suite above compares every available flavour
/// against the oracle -- but if the override silently failed to take effect,
/// the "AVX2" flavour would actually be the scalar fallback, and the suite
/// would be comparing the scalar reference against itself and proving
/// nothing at all.
///
/// So the check is not "the dispatch says AVX2". It is: the row counters
/// moved, the mask still matches the oracle, and therefore the instructions
/// ran and produced the right answer.
///
/// On an ARM64 Mac under Rosetta 2 this is the only way the AVX2 path can
/// be exercised at all, because CPUID reports no AVX2 on the host.
#[test]
fn a_forced_simd_path_actually_executed() {
    use columnar_kernels::force::{forced, ForcedSimd};

    let requested = forced();
    if requested == ForcedSimd::Auto {
        // Nothing forced; the probe governs. The counters are then expected
        // to be zero on a machine with no SIMD path, and non-zero on one
        // with -- but that is not this test's business.
        return;
    }

    let values: Vec<f64> = (0..256).map(|i| f64::from(i % 11)).collect();
    let mut expected = vec![0u64; values.len().div_ceil(64)];
    let expected_count = oracle::compare_f64(&values, ComparisonOp::GtEq, 5.0, None, &mut expected);

    #[cfg(target_arch = "x86_64")]
    if requested == ForcedSimd::Avx2 {
        let cpp_before = columnar_kernels::avx2_rows_processed();
        let rust_before = columnar_kernels::rust_simd::rows_processed();

        let mut actual = vec![0u64; values.len().div_ceil(64)];
        let count =
            compare_f64_into(Flavor::CppSimd, &values, ComparisonOp::GtEq, 5.0, None, &mut actual);
        assert_eq!(actual, expected, "the forced C++ AVX2 path disagreed with the oracle");
        assert_eq!(count, expected_count);
        assert!(
            columnar_kernels::avx2_rows_processed() > cpp_before,
            "the C++ AVX2 path reported itself live but processed no rows"
        );

        let mut actual = vec![0u64; values.len().div_ceil(64)];
        let count =
            compare_f64_into(Flavor::RustSimd, &values, ComparisonOp::GtEq, 5.0, None, &mut actual);
        assert_eq!(actual, expected, "the forced Rust AVX2 path disagreed with the oracle");
        assert_eq!(count, expected_count);
        assert!(
            columnar_kernels::rust_simd::rows_processed() > rust_before,
            "the Rust AVX2 path reported itself live but processed no rows"
        );
    }

    #[cfg(target_arch = "aarch64")]
    if requested == ForcedSimd::Neon {
        let before = columnar_kernels::rust_simd::rows_processed();
        let mut actual = vec![0u64; values.len().div_ceil(64)];
        let count =
            compare_f64_into(Flavor::RustSimd, &values, ComparisonOp::GtEq, 5.0, None, &mut actual);
        assert_eq!(actual, expected, "the forced NEON path disagreed with the oracle");
        assert_eq!(count, expected_count);
        assert!(columnar_kernels::rust_simd::rows_processed() > before);
    }

    if requested == ForcedSimd::None {
        assert!(
            !flavors().contains(&Flavor::RustSimd),
            "COLUMNAR_FORCE_SIMD=none but the Rust intrinsics path was still offered"
        );
    }
}
