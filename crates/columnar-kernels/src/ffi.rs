//! Raw bindings to the C++ comparison kernel.
//!
//! This module is the only place in the workspace that touches raw pointers
//! into C++. Everything above it goes through the safe API in
//! [`crate::compare_i64_into`] and [`crate::compare_f64_into`].
//!
//! # Safety
//!
//! Every function here assumes:
//!
//! - `values` points to at least `n` readable elements of the right type.
//! - `out` points to at least `(n + 63) / 64` writable `u64` words.
//! - `validity`, when non-null, points to at least `(n + 63) / 64` readable
//!   `u64` words.
//!
//! The safe wrappers establish all three before calling.

use core::ffi::{c_double, c_int, c_longlong};

/// The operator codes, mirroring `comparison.h`.
///
/// These must match `columnar_core::ComparisonOp::abi_code()`. A test in this
/// crate asserts the two agree, because a silent mismatch would change what
/// the filter computes rather than what it prints.
///
/// They are `dead_code` outside that test on purpose: they exist to pin the
/// ABI against the header, not to be called.
#[allow(dead_code)]
pub(crate) mod abi {
    use core::ffi::c_int;

    /// Less than.
    pub(crate) const COL_CMP_LT: c_int = 0;
    /// Less than or equal.
    pub(crate) const COL_CMP_LE: c_int = 1;
    /// Greater than.
    pub(crate) const COL_CMP_GT: c_int = 2;
    /// Greater than or equal.
    pub(crate) const COL_CMP_GE: c_int = 3;
    /// Equal.
    pub(crate) const COL_CMP_EQ: c_int = 4;
    /// Not equal.
    pub(crate) const COL_CMP_NE: c_int = 5;

    /// No SIMD path is live.
    pub(crate) const COL_SIMD_NONE: c_int = 0;
    /// ARM NEON is live.
    pub(crate) const COL_SIMD_NEON: c_int = 1;
    /// x86 AVX2 is live.
    pub(crate) const COL_SIMD_AVX2: c_int = 2;
}

unsafe extern "C" {
    /// See `comparison.h`.
    pub(crate) fn col_compare_i64(
        values: *const i64,
        n: u64,
        op: c_int,
        threshold: i64,
        validity: *const u64,
        out: *mut u64,
    ) -> c_longlong;

    /// See `comparison.h`.
    pub(crate) fn col_compare_f64(
        values: *const c_double,
        n: u64,
        op: c_int,
        threshold: c_double,
        validity: *const u64,
        out: *mut u64,
    ) -> c_longlong;

    /// The scalar reference compiled *without* vectorisation; never dispatches
    /// to a SIMD path.
    ///
    /// This is the baseline that answers "how much is vectorisation worth?".
    pub(crate) fn col_compare_i64_scalar(
        values: *const i64,
        n: u64,
        op: c_int,
        threshold: i64,
        validity: *const u64,
        out: *mut u64,
    ) -> c_longlong;

    /// The scalar reference compiled *without* vectorisation; never dispatches
    /// to a SIMD path.
    ///
    /// This is the baseline that answers "how much is vectorisation worth?".
    pub(crate) fn col_compare_f64_scalar(
        values: *const c_double,
        n: u64,
        op: c_int,
        threshold: c_double,
        validity: *const u64,
        out: *mut u64,
    ) -> c_longlong;

    /// The same scalar reference, compiled with ordinary flags so the compiler
    /// vectorises it; never dispatches to a SIMD path.
    ///
    /// This is the baseline the hand-written SIMD kernel has to beat to be
    /// worth anything: it is what you get for free by writing the loop in
    /// plain C++.
    pub(crate) fn col_compare_i64_auto(
        values: *const i64,
        n: u64,
        op: c_int,
        threshold: i64,
        validity: *const u64,
        out: *mut u64,
    ) -> c_longlong;

    /// The same scalar reference, compiled with ordinary flags so the compiler
    /// vectorises it; never dispatches to a SIMD path.
    ///
    /// This is the baseline the hand-written SIMD kernel has to beat to be
    /// worth anything: it is what you get for free by writing the loop in
    /// plain C++.
    pub(crate) fn col_compare_f64_auto(
        values: *const c_double,
        n: u64,
        op: c_int,
        threshold: c_double,
        validity: *const u64,
        out: *mut u64,
    ) -> c_longlong;

    /// Which SIMD path is live on this machine.
    pub(crate) fn col_simd_kind() -> c_int;
}
