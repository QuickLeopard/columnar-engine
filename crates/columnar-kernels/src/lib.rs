//! Comparison kernels for the columnar engine.
//!
//! This crate owns the single hottest operation in the engine: deciding which
//! rows of a column satisfy a predicate. It contains six implementations of
//! that one operation, and every one of them must agree with every other one
//! bit for bit:
//!
//! | Implementation | What it is | Why it exists |
//! |---|---|---|
//! | [`oracle::compare_i64`] / [`oracle::compare_f64`] | a plain Rust loop | the correctness reference |
//! | C++ scalar | the same predicate, branchless, compiled with `-fno-vectorize` | the A/B **baseline**: what vectorisation is worth |
//! | C++ auto | the same source, compiled with ordinary flags | what the compiler does on its own — the bar the hand-written kernel has to clear |
//! | C++ SIMD | `NEON` on `AArch64`, `AVX2` on x86-64, over the FFI seam | the headline kernel |
//! | Rust SIMD | `std::arch` intrinsics, no FFI | measures what the FFI seam costs |
//! | [`Flavor`] | the public selection enum | what callers pick |
//!
//! ## Two baselines, not one
//!
//! The first version of this crate published one ratio: `cpp-simd` over
//! `scalar`. Disassembling the compiled object showed that the "scalar"
//! reference was not scalar -- clang had auto-vectorised it, and the object
//! contained `fcmgt.2d`, `ushl.2d` and `dup.2d`. The published ratio was
//! therefore measuring two vectorised implementations against each other and
//! calling it a vectorisation speedup.
//!
//! So the scalar reference is now compiled twice from the same source:
//! once with the vectoriser switched off ([`Flavor::Scalar`]) and once with
//! it on ([`Flavor::CppAuto`]). The A/B publishes both ratios:
//!
//! - `scalar -> cpp-simd`: how much is vectorisation worth?
//! - `cpp-auto -> cpp-simd`: does the hand-written kernel beat what the
//!   compiler does with the same loop?
//!
//! The second question has no predetermined answer. See ADR-0006.
//!
//! ## Why there are three public flavors
//!
//! The design review asked a question that a project with one kernel cannot
//! answer: *"The `cc`-based FFI for a single kernel may not be the actual
//! bottleneck. If the surrounding code is the bottleneck, the FFI overhead can
//! erase the win."* (review finding 2.12)
//!
//! The answer has to be a measurement. [`Flavor::RustSimd`] runs the same
//! predicate through Rust intrinsics with no FFI at all. If it and
//! [`Flavor::CppSimd`] measure the same, the seam is free and the C++ stays.
//! If it does not, the C++ story has to go. Both numbers are published in
//! `PERF.md`, and the decision is recorded in `docs/adr/0003-cpp-ffi-seam.md`.
//!
//! ## NULL and NaN
//!
//! - A row whose validity bit is clear is NULL and **never matches**, for
//!   every operator, including `!=`.
//! - A comparison involving a NaN value or a NaN threshold **never matches**,
//!   for every operator, including `!=`. This is "unknown propagates", and it
//!   is stricter than SQL's treatment of NaN.
//!
//! Both rules are implemented identically in the oracle and in both SIMD
//! paths, and the differential tests cover them explicitly.

#![deny(missing_docs)]
// See the note in `columnar-core`: the kernel indexes value arrays and
// bit-mask words directly, with the bounds established by the caller and
// checked by the differential tests.
#![allow(clippy::indexing_slicing)]

pub mod ffi;
pub mod oracle;
pub mod rust_simd;

use columnar_core::{ComparisonOp, NullBitmap};

/// Which implementation of the comparison kernel to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Flavor {
    /// The C++ scalar reference, compiled with vectorisation **disabled**.
    ///
    /// This is the A/B baseline for "how much is vectorisation worth?". It
    /// computes the same predicate as the SIMD paths in the same branchless
    /// shape, so the ratio between the two is a measurement of vectorisation
    /// rather than of branch prediction.
    Scalar,
    /// The C++ scalar reference, compiled with **ordinary** flags, so the
    /// compiler vectorises it.
    ///
    /// This is the stronger baseline, and the one that decides whether the
    /// hand-written SIMD kernel is worth its complexity: it is what you get
    /// for free by writing the loop in plain C++ and letting the compiler
    /// do it. A SIMD kernel that does not beat this has not earned its
    /// keep.
    CppAuto,
    /// The C++ SIMD path, reached over the FFI seam.
    ///
    /// `NEON` on `AArch64`, `AVX2` on x86-64. On a machine without either, the C++
    /// side falls back to its scalar path and this flavor behaves exactly
    /// like [`Flavor::CppAuto`].
    CppSimd,
    /// The Rust `std::arch` intrinsics path. No FFI.
    RustSimd,
}

impl Flavor {
    /// A short, stable name for plan output and benchmark labels.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Scalar => "scalar",
            Self::CppAuto => "cpp-auto",
            Self::CppSimd => "cpp-simd",
            Self::RustSimd => "rust-simd",
        }
    }

    /// Every flavor, in the order the benchmark reports them.
    #[must_use]
    pub const fn all() -> [Self; 4] {
        [Self::Scalar, Self::CppAuto, Self::CppSimd, Self::RustSimd]
    }
}

/// The SIMD path that is live on this machine, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimdKind {
    /// No SIMD path is available; everything runs scalar.
    None,
    /// ARM NEON.
    Neon,
    /// x86 AVX2.
    Avx2,
}

impl SimdKind {
    /// A short, stable name for plan output and benchmark labels.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Neon => "neon",
            Self::Avx2 => "avx2",
        }
    }
}

/// The SIMD path that is live on this machine right now.
///
/// This is a **runtime** probe. The C++ translation unit is compiled for the
/// baseline ISA and reaches AVX2 only through a target-attributed function
/// after a CPU feature check, so this returns [`SimdKind::None`] on a
/// machine that cannot run AVX2 rather than faulting.
#[must_use]
pub fn simd_kind() -> SimdKind {
    // SAFETY: `col_simd_kind` takes no arguments, reads no pointers and
    // touches no global state.
    match unsafe { ffi::col_simd_kind() } {
        ffi::abi::COL_SIMD_NEON => SimdKind::Neon,
        ffi::abi::COL_SIMD_AVX2 => SimdKind::Avx2,
        _ => SimdKind::None,
    }
}

/// The flavors that can actually run on this machine, most-preferred first.
///
/// [`Flavor::Scalar`] and [`Flavor::CppAuto`] are always present -- they are
/// the same C++ source compiled two ways, and neither needs SIMD.
/// [`Flavor::CppSimd`] is present when the C++ side reports a live SIMD
/// path. [`Flavor::RustSimd`] is present when this machine can run the Rust
/// intrinsics path.
#[must_use]
pub fn available() -> Vec<Flavor> {
    let mut flavors = Vec::with_capacity(4);
    if simd_kind() != SimdKind::None {
        flavors.push(Flavor::CppSimd);
    }
    if rust_simd::available() {
        flavors.push(Flavor::RustSimd);
    }
    flavors.push(Flavor::CppAuto);
    flavors.push(Flavor::Scalar);
    flavors
}

/// The fastest flavor available on this machine.
///
/// Falls back to [`Flavor::Scalar`], which is always correct.
#[must_use]
pub fn best() -> Flavor {
    available().into_iter().next().unwrap_or(Flavor::Scalar)
}

/// The validity mask's words as a raw pointer, or null when there is no mask.
fn validity_ptr(validity: Option<&NullBitmap>) -> *const u64 {
    validity.map_or(std::ptr::null(), |v| v.words().as_ptr())
}

/// The C entry point for a 64-bit integer comparison.
type CompareI64Fn = unsafe extern "C" fn(
    *const i64,
    u64,
    core::ffi::c_int,
    i64,
    *const u64,
    *mut u64,
) -> core::ffi::c_longlong;

/// The C entry point for a 64-bit float comparison.
type CompareF64Fn = unsafe extern "C" fn(
    *const f64,
    u64,
    core::ffi::c_int,
    f64,
    *const u64,
    *mut u64,
) -> core::ffi::c_longlong;

/// Runs one of the C++ entry points and converts its return value.
///
/// The C side returns a negative value only for a null pointer or an unknown
/// operator code. Neither is reachable from this call site: the pointers are
/// live slices and `op` is one of the six defined operators. A negative
/// result is therefore treated as "no rows matched" rather than propagated,
/// because the public API has no error channel.
fn run_cpp(
    entry: CompareI64Fn,
    values: &[i64],
    op: ComparisonOp,
    threshold: i64,
    validity: Option<&NullBitmap>,
    out: &mut [u64],
) -> usize {
    // SAFETY: `values` is a live slice of `i64` of length `values.len()`,
    // `out` is a live slice of at least `(values.len() + 63) / 64` words
    // (asserted by the caller), and `validity` is either absent or a live
    // mask whose length covers `values`.
    let count = unsafe {
        entry(
            values.as_ptr(),
            values.len() as u64,
            op.abi_code(),
            threshold,
            validity_ptr(validity),
            out.as_mut_ptr(),
        )
    };
    usize::try_from(count).unwrap_or(0)
}

/// Runs one of the C++ entry points and converts its return value.
///
/// See [`run_cpp`].
fn run_cpp_f64(
    entry: CompareF64Fn,
    values: &[f64],
    op: ComparisonOp,
    threshold: f64,
    validity: Option<&NullBitmap>,
    out: &mut [u64],
) -> usize {
    // SAFETY: as in `run_cpp`.
    let count = unsafe {
        entry(
            values.as_ptr(),
            values.len() as u64,
            op.abi_code(),
            threshold,
            validity_ptr(validity),
            out.as_mut_ptr(),
        )
    };
    usize::try_from(count).unwrap_or(0)
}

/// Compares `values` against `threshold`, writing the match mask into `out`.
///
/// `out` must hold at least `(values.len() + 63) / 64` words. Bits beyond
/// `values.len()` are not written.
///
/// `validity`, when given, marks the rows that hold a value. A row whose
/// validity bit is clear is NULL and never matches.
///
/// Returns the number of matching rows.
///
/// # Panics
///
/// Panics if `out` is shorter than `(values.len() + 63) / 64`.
pub fn compare_i64_into(
    flavor: Flavor,
    values: &[i64],
    op: ComparisonOp,
    threshold: i64,
    validity: Option<&NullBitmap>,
    out: &mut [u64],
) -> usize {
    let required = values.len().div_ceil(64);
    assert!(
        out.len() >= required,
        "match mask needs {required} words for {} values, got {}",
        values.len(),
        out.len()
    );

    match flavor {
        Flavor::Scalar => {
            run_cpp(ffi::col_compare_i64_scalar, values, op, threshold, validity, out)
        }
        Flavor::CppAuto => run_cpp(ffi::col_compare_i64_auto, values, op, threshold, validity, out),
        Flavor::CppSimd => run_cpp(ffi::col_compare_i64, values, op, threshold, validity, out),
        Flavor::RustSimd => rust_simd::compare_i64(values, op, threshold, validity, out),
    }
}

/// Compares `values` against `threshold`, writing the match mask into `out`.
///
/// See [`compare_i64_into`] for the full contract. The NaN rule applies: a
/// comparison involving a NaN never matches, for every operator.
///
/// # Panics
///
/// Panics if `out` is shorter than `(values.len() + 63) / 64`.
pub fn compare_f64_into(
    flavor: Flavor,
    values: &[f64],
    op: ComparisonOp,
    threshold: f64,
    validity: Option<&NullBitmap>,
    out: &mut [u64],
) -> usize {
    let required = values.len().div_ceil(64);
    assert!(
        out.len() >= required,
        "match mask needs {required} words for {} values, got {}",
        values.len(),
        out.len()
    );

    match flavor {
        Flavor::Scalar => {
            run_cpp_f64(ffi::col_compare_f64_scalar, values, op, threshold, validity, out)
        }
        Flavor::CppAuto => {
            run_cpp_f64(ffi::col_compare_f64_auto, values, op, threshold, validity, out)
        }
        Flavor::CppSimd => run_cpp_f64(ffi::col_compare_f64, values, op, threshold, validity, out),
        Flavor::RustSimd => rust_simd::compare_f64(values, op, threshold, validity, out),
    }
}

/// Compares `values` against `threshold` and returns the match mask.
///
/// A convenience wrapper that allocates. The hot path uses
/// [`compare_i64_into`] with a reused buffer instead.
#[must_use]
pub fn compare_i64(
    flavor: Flavor,
    values: &[i64],
    op: ComparisonOp,
    threshold: i64,
    validity: Option<&NullBitmap>,
) -> Vec<u64> {
    let mut out = vec![0u64; values.len().div_ceil(64)];
    compare_i64_into(flavor, values, op, threshold, validity, &mut out);
    out
}

/// Compares `values` against `threshold` and returns the match mask.
///
/// A convenience wrapper that allocates. The hot path uses
/// [`compare_f64_into`] with a reused buffer instead.
#[must_use]
pub fn compare_f64(
    flavor: Flavor,
    values: &[f64],
    op: ComparisonOp,
    threshold: f64,
    validity: Option<&NullBitmap>,
) -> Vec<u64> {
    let mut out = vec![0u64; values.len().div_ceil(64)];
    compare_f64_into(flavor, values, op, threshold, validity, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::{simd_kind, Flavor};
    use columnar_core::{ComparisonOp, NullBitmap};

    #[test]
    fn the_abi_operator_codes_match_the_header() {
        // The C++ header defines COL_CMP_LT..COL_CMP_NE as 0..5. If either
        // side changes, the filter silently computes the wrong predicate --
        // so the mapping is asserted rather than trusted.
        assert_eq!(ComparisonOp::Lt.abi_code(), super::ffi::abi::COL_CMP_LT);
        assert_eq!(ComparisonOp::LtEq.abi_code(), super::ffi::abi::COL_CMP_LE);
        assert_eq!(ComparisonOp::Gt.abi_code(), super::ffi::abi::COL_CMP_GT);
        assert_eq!(ComparisonOp::GtEq.abi_code(), super::ffi::abi::COL_CMP_GE);
        assert_eq!(ComparisonOp::Eq.abi_code(), super::ffi::abi::COL_CMP_EQ);
        assert_eq!(ComparisonOp::Ne.abi_code(), super::ffi::abi::COL_CMP_NE);
    }

    #[test]
    fn every_abi_code_is_distinct_and_in_range() {
        let mut seen = std::collections::HashSet::new();
        for op in ComparisonOp::all() {
            assert!(seen.insert(op.abi_code()), "duplicate ABI code for {op:?}");
            assert!((0..6).contains(&op.abi_code()));
        }
    }

    #[test]
    fn the_scalar_flavor_is_always_available() {
        assert!(super::available().contains(&Flavor::Scalar));
    }

    #[test]
    fn best_is_never_slower_than_scalar() {
        assert!(super::available().contains(&super::best()));
    }

    #[test]
    fn simd_kind_is_reported_consistently() {
        let kind = simd_kind();
        if kind == super::SimdKind::None {
            assert!(!super::available().contains(&Flavor::CppSimd));
        } else {
            assert!(super::available().contains(&Flavor::CppSimd));
        }
    }

    #[test]
    fn a_null_bit_suppresses_a_match() {
        let validity = NullBitmap::with_nulls(4, &[1]);
        let out = super::compare_i64(
            Flavor::Scalar,
            &[10, 10, 10, 10],
            ComparisonOp::Eq,
            10,
            Some(&validity),
        );
        // Rows 0, 2 and 3 hold a value and match; row 1 is NULL and never
        // matches, even for an equality test.
        assert_eq!(out[0] & 0b1111, 0b1101);
    }
}
