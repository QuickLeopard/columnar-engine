//! The browser build of the comparison kernel.
//!
//! This crate compiles to WebAssembly and runs the **engine's own kernel**:
//! `columnar-kernels`, the same crate the native build uses, with the same
//! oracle as its reference and the same semantics — a NULL never matches, a
//! NaN never matches, for every operator including `!=`.
//!
//! On the browser target the kernel's SIMD path is WebAssembly's `simd128`,
//! which is a **third** SIMD path beside NEON and AVX2. The C++ translation
//! units are not built for `wasm32` — see `columnar-kernels`' build script —
//! so the browser's reference is the same plain Rust oracle every other
//! target is held to.
//!
//! # The ABI
//!
//! The module exports a handful of functions and no generated glue: there is
//! no `wasm-bindgen` in this repository's dependencies, and a build step that
//! needs `npm` is a build step a reader may not be able to run. The page
//! loads the `.wasm` directly.
//!
//! | export | what it does |
//! |---|---|
//! | [`columnar_web_prepare`] | rebuilds the column from the histogram |
//! | [`columnar_web_run`] | runs one implementation over it, `iterations` times |
//! | [`columnar_web_masks_agree`] | compares the two masks, bit for bit |
//! | [`columnar_web_simd_kind`] | says whether the `simd128` path is live |
//!
//! The **clock is the browser's**. `std::time::Instant` is not available on
//! `wasm32-unknown-unknown` — there is no clock in that platform — so the
//! page times the calls with `performance.now()` and the module does not
//! pretend to know how long anything took.
//!
//! # What the page is allowed to publish
//!
//! The page compares the `simd128` mask with the oracle's mask **before** it
//! prints anything, and prints no throughput number if they disagree. The
//! match count the browser reports is the native benchmark's, because the
//! column is the native column's multiset — see [`histogram`].

#![deny(missing_docs)]

pub mod column;
pub mod histogram;

use std::cell::RefCell;

use columnar_core::ComparisonOp;
use columnar_kernels::Flavor;

/// Which implementation a run should use: the plain Rust oracle.
const ORACLE: u32 = 0;

/// Which implementation a run should use: the Rust intrinsics path, which on
/// the browser is `simd128`.
const SIMD: u32 = 1;

/// The state the module keeps between calls.
#[derive(Debug, Default)]
struct State {
    /// The column the runs measure over.
    column: Vec<f64>,
    /// The oracle's mask from the last oracle run.
    oracle_mask: Vec<u64>,
    /// The `simd128` path's mask from the last `simd128` run.
    simd_mask: Vec<u64>,
}

thread_local! {
    /// The module's state.
    ///
    /// A WebAssembly module in a browser is single-threaded, so this is one
    /// buffer rather than a lock.
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// Rebuilds the column and returns the number of rows written.
///
/// The column is the canonical one from [`histogram`]: the same multiset as
/// the native benchmark's, in a deterministic interleaved order. Asking for
/// more rows than the canonical column has truncates it.
#[no_mangle]
pub extern "C" fn columnar_web_prepare(rows: u32) -> u32 {
    let rows = usize::try_from(rows).unwrap_or(0);
    let column = column::build(rows);
    let words = rows.div_ceil(64);
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.column = column;
        state.oracle_mask = vec![0; words];
        state.simd_mask = vec![0; words];
    });
    u32::try_from(rows).unwrap_or(0)
}

/// Runs one implementation over the prepared column.
///
/// `flavor` is 0 for the oracle and 1 for the `simd128` path. The column is
/// scanned `iterations` times and the number of matching rows is returned —
/// the same count every time, which is the property the page checks.
///
/// The mask the run produced is kept, so that
/// [`columnar_web_masks_agree`] can compare the two implementations bit for
/// bit afterwards.
#[no_mangle]
pub extern "C" fn columnar_web_run(flavor: u32, rows: u32, iterations: u32, threshold: f64) -> u64 {
    let rows = usize::try_from(rows).unwrap_or(0);
    let iterations = usize::try_from(iterations).unwrap_or(1).max(1);
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if rows > state.column.len() {
            return 0;
        }
        // The three fields are borrowed apart, so the column can be read
        // while the mask is written.
        let State { column, oracle_mask, simd_mask } = &mut *state;
        let values = column.get(..rows).unwrap_or(&[]);
        let mask: &mut [u64] = match flavor {
            SIMD => simd_mask.as_mut_slice(),
            ORACLE => oracle_mask.as_mut_slice(),
            // An unknown flavor id is a page that is talking to a module it
            // does not understand. Matching nothing is the safe answer.
            _ => return 0,
        };
        mask.fill(0);
        let mut matched = 0usize;
        for _ in 0..iterations {
            matched = if flavor == SIMD {
                columnar_kernels::compare_f64_into(
                    Flavor::RustSimd,
                    values,
                    ComparisonOp::GtEq,
                    threshold,
                    None,
                    mask,
                )
            } else {
                columnar_kernels::oracle::compare_f64(
                    values,
                    ComparisonOp::GtEq,
                    threshold,
                    None,
                    mask,
                )
            };
        }
        u64::try_from(matched).unwrap_or(0)
    })
}

/// Whether the oracle's mask and the `simd128` path's mask are **identical**.
///
/// Bit for bit, including the padding bits past the last row. This is the
/// check the page makes before it prints anything: an implementation that
/// computed a different predicate would otherwise publish a faster, wrong
/// number.
#[no_mangle]
pub extern "C" fn columnar_web_masks_agree() -> u32 {
    STATE.with(|state| {
        let state = state.borrow();
        if state.oracle_mask.len() != state.simd_mask.len() {
            return 0;
        }
        u32::from(state.oracle_mask == state.simd_mask)
    })
}

/// Whether the `simd128` path is live in this module.
///
/// 1 for yes, 0 for no. The page prints this, because a page that measured
/// the oracle twice and called it a SIMD comparison would be lying.
#[no_mangle]
pub extern "C" fn columnar_web_simd_kind() -> u32 {
    u32::from(columnar_kernels::simd_kind() == columnar_kernels::SimdKind::WasmSimd128)
}

/// The number of rows that must match, for a row count the histogram pins.
///
/// The browser's column is the native column's multiset, so the browser's
/// count must be this number. It is `0` for a row count the histogram does
/// not pin — one that ends inside a segment, where the interleaved order
/// makes the count vary.
#[no_mangle]
pub extern "C" fn columnar_web_expected_matches(rows: u32) -> u64 {
    let rows = usize::try_from(rows).unwrap_or(0);
    histogram::expected_matches(rows).unwrap_or(0)
}

/// The number of rows in the prepared column.
#[no_mangle]
pub extern "C" fn columnar_web_rows() -> u32 {
    STATE.with(|state| u32::try_from(state.borrow().column.len()).unwrap_or(0))
}

/// The threshold the benchmark compares against.
#[no_mangle]
pub extern "C" fn columnar_web_threshold() -> f64 {
    histogram::THRESHOLD
}

/// The values that are easy to get wrong.
///
/// A NaN, the two zeros, the two infinities, and the values either side of
/// the thresholds. This is the array the edge check runs over.
const EDGE_VALUES: [f64; 16] = [
    f64::NAN,
    -0.0,
    0.0,
    f64::INFINITY,
    f64::NEG_INFINITY,
    1.0,
    -1.0,
    0.5,
    -0.5,
    0.04,
    0.05,
    0.03,
    0.07,
    1e-300,
    -1e-300,
    0.040_000_000_000_000_01,
];

/// The thresholds the edge check compares against.
///
/// The last one is a NaN, which must never match — for any operator,
/// including `!=`.
const EDGE_THRESHOLDS: [f64; 8] =
    [0.04, 0.0, -0.0, 1.0, f64::INFINITY, f64::NEG_INFINITY, 0.040_000_000_000_000_01, f64::NAN];

/// Checks the `simd128` path against the oracle over **every** operator, on
/// the values that are easy to get wrong.
///
/// This is the browser's differential test, and it runs in the reader's
/// browser rather than in CI — because CI has no browser, and the path it
/// checks only exists in a browser.
///
/// It covers:
///
/// - all six operators, including `!=`;
/// - a NaN value and a NaN threshold, neither of which may ever match;
/// - the two zeros and the two infinities;
/// - a NULL row, which must never match, for any operator.
///
/// Returns 1 when the two paths agree on every mask, bit for bit, and 0 the
/// first time they do not.
#[no_mangle]
pub extern "C" fn columnar_web_check_edges() -> u32 {
    let validity = columnar_core::NullBitmap::with_nulls(EDGE_VALUES.len(), &[1, 7]);
    let words = columnar_core::NullBitmap::words_for(EDGE_VALUES.len());
    let mut oracle_mask = vec![0u64; words];
    let mut simd_mask = vec![0u64; words];

    for op in ComparisonOp::all() {
        for threshold in EDGE_THRESHOLDS {
            oracle_mask.fill(0);
            simd_mask.fill(0);
            let oracle = columnar_kernels::oracle::compare_f64(
                &EDGE_VALUES,
                op,
                threshold,
                Some(&validity),
                &mut oracle_mask,
            );
            let simd = columnar_kernels::compare_f64_into(
                Flavor::RustSimd,
                &EDGE_VALUES,
                op,
                threshold,
                Some(&validity),
                &mut simd_mask,
            );
            if oracle != simd || oracle_mask != simd_mask {
                return 0;
            }
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::histogram::{BENCH_MATCHES, BENCH_ROWS, FULL_MATCHES, THRESHOLD};

    #[test]
    fn the_prepared_column_matches_the_native_benchmark() {
        assert_eq!(columnar_web_prepare(u32::try_from(BENCH_ROWS).unwrap()), 20_000_000);
        let matched = columnar_web_run(ORACLE, 20_000_000, 1, THRESHOLD);
        assert_eq!(matched, BENCH_MATCHES);
        let simd = columnar_web_run(SIMD, 20_000_000, 1, THRESHOLD);
        assert_eq!(simd, BENCH_MATCHES);
        assert_eq!(columnar_web_masks_agree(), 1);
    }

    #[test]
    fn the_six_million_row_column_matches_the_native_column() {
        columnar_web_prepare(6_000_000);
        let matched = columnar_web_run(ORACLE, 6_000_000, 1, THRESHOLD);
        assert_eq!(matched, FULL_MATCHES);
    }

    /// The agreement check **can fail**, and says so.
    ///
    /// This is the property the page depends on: it publishes a throughput
    /// only when the two masks are identical, so a check that always said
    /// "yes" would publish a number for an answer nobody verified. Here the
    /// two runs are given different thresholds, so their masks differ, and
    /// the check has to notice.
    #[test]
    fn the_agreement_check_fails_when_the_masks_differ() {
        columnar_web_prepare(1_000_000);
        let oracle = columnar_web_run(ORACLE, 1_000_000, 1, 0.04);
        let simd = columnar_web_run(SIMD, 1_000_000, 1, 0.05);
        assert_ne!(oracle, simd, "the two thresholds must match different rows");
        assert_eq!(columnar_web_masks_agree(), 0);
    }
}
