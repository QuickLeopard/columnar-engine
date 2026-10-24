//! The Rust oracle.
//!
//! This is the correctness reference for the whole kernel crate. It is
//! written to be *obviously* right rather than fast: a straightforward loop,
//! one row at a time, with the predicate spelled out in full.
//!
//! Every other implementation -- the C++ scalar reference, the C++ NEON path,
//! the C++ AVX2 path and the Rust intrinsics path -- must agree with this
//! oracle bit for bit. The differential tests in `tests/` generate random
//! inputs and assert exactly that.
//!
//! The oracle is also what makes the SIMD paths reviewable: a reader who
//! doubts an intrinsic can compare it against these twelve lines.

use columnar_core::{ComparisonOp, NullBitmap};

/// Compares `values` against `threshold`, writing the match mask into `out`.
///
/// Returns the number of matching rows.
///
/// # Panics
///
/// Panics if `out` is shorter than `(values.len() + 63) / 64`.
pub fn compare_i64(
    values: &[i64],
    op: ComparisonOp,
    threshold: i64,
    validity: Option<&NullBitmap>,
    out: &mut [u64],
) -> usize {
    let mut count = 0usize;
    for (row, &value) in values.iter().enumerate() {
        let valid = validity.is_none_or(|v| v.is_valid(row));
        let matched = valid
            && match op {
                ComparisonOp::Lt => value < threshold,
                ComparisonOp::LtEq => value <= threshold,
                ComparisonOp::Gt => value > threshold,
                ComparisonOp::GtEq => value >= threshold,
                ComparisonOp::Eq => value == threshold,
                ComparisonOp::Ne => value != threshold,
            };
        if matched {
            out[row / 64] |= 1u64 << (row % 64);
            count += 1;
        }
    }
    count
}

/// Compares `values` against `threshold`, writing the match mask into `out`.
///
/// Returns the number of matching rows.
///
/// # NaN
///
/// A comparison involving a NaN value or a NaN threshold never matches, for
/// every operator **including** `Ne`. This is "unknown propagates": a NaN
/// cannot be meaningfully compared, so it cannot meaningfully satisfy a
/// filter either. The SIMD paths implement the same rule through ordered
/// comparison predicates, which is why `Ne` is expressed as
/// `value < threshold || value > threshold` rather than `value != threshold`.
///
/// # Panics
///
/// Panics if `out` is shorter than `(values.len() + 63) / 64`.
// The `float_cmp` and `nonminimal_bool` suggestions are both wrong here, and
// deliberately overridden:
//
// - `float_cmp` suggests comparing floats "within some margin of error". In
//   a query engine that would change what the query *means*: `WHERE x = 0.1`
//   is an equality test on the stored bits, not a tolerance test. An epsilon
//   comparison would silently return different rows than DuckDB.
// - `double_comparisons` suggests rewriting the ordered not-equal as `!=`.
//   That would make NaN match, which this engine's rule forbids.
#[allow(clippy::float_cmp, clippy::double_comparisons)]
pub fn compare_f64(
    values: &[f64],
    op: ComparisonOp,
    threshold: f64,
    validity: Option<&NullBitmap>,
    out: &mut [u64],
) -> usize {
    let mut count = 0usize;
    for (row, &value) in values.iter().enumerate() {
        let valid = validity.is_none_or(|v| v.is_valid(row));
        let matched = valid
            && match op {
                ComparisonOp::Lt => value < threshold,
                ComparisonOp::LtEq => value <= threshold,
                ComparisonOp::Gt => value > threshold,
                ComparisonOp::GtEq => value >= threshold,
                ComparisonOp::Eq => value == threshold,
                // Ordered not-equal: false if either operand is NaN.
                ComparisonOp::Ne => value < threshold || value > threshold,
            };
        if matched {
            out[row / 64] |= 1u64 << (row % 64);
            count += 1;
        }
    }
    count
}
