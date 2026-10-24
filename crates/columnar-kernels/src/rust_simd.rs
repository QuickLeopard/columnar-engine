//! The Rust intrinsics path.
//!
//! This is a second, independent SIMD implementation of the same predicate,
//! written in Rust rather than C++. It exists for one reason, and that
//! reason is a question the design review asked and the project must answer
//! with a measurement rather than an assertion:
//!
//! > *The `cc`-based FFI for a single kernel may not be the actual
//! > bottleneck. If the surrounding code is the bottleneck, the FFI overhead
//! > can erase the win.* (review finding 2.12)
//!
//! If the Rust intrinsics path and the C++ FFI path measure the same, the
//! FFI seam costs nothing measurable and stays in. If the Rust path is
//! materially faster, the FFI is the bottleneck and the C++ story has to go.
//! Both numbers are published in `PERF.md`, and the decision is recorded in
//! `docs/adr/0003-cpp-ffi-seam.md`.
//!
//! # Safety
//!
//! The functions in this module use `unsafe` intrinsics. Every call site is
//! inside a function that has already established the pointer's validity and
//! alignment, and the `SAFETY:` comments say which invariant is being relied
//! on. Rust 1.82 removed the implicit `unsafe` block from the body of a
//! `#[target_feature]` function, so each intrinsic call is wrapped
//! explicitly.

// `op.abi_code()` returns an `i32` in the range 0..=5 by construction -- the
// six comparison operators are the only codes the ABI defines -- so the
// narrowing to `u8` below is exact.
#![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]

use columnar_core::{ComparisonOp, NullBitmap};
use std::sync::atomic::{AtomicU64, Ordering};

/// Rows processed by the Rust intrinsics path.
///
/// The C++ side has the same counter on its AVX2 path, and the two exist for
/// the same reason: a report of "this path is live" is not the same fact as
/// "these instructions ran". See [`crate::avx2_rows_processed`].
static ROWS_PROCESSED: AtomicU64 = AtomicU64::new(0);

/// How many rows the Rust intrinsics path has processed since the process
/// started.
///
/// Zero when the path is not available, and when it has not run.
#[must_use]
pub fn rows_processed() -> u64 {
    ROWS_PROCESSED.load(Ordering::Relaxed)
}

/// Operator codes as `u8`, matching `ComparisonOp::abi_code()`.
///
/// These become const-generic parameters, so the operator is resolved at
/// compile time and the hot loop contains no switch.
mod op {
    pub(super) const LT: u8 = 0;
    pub(super) const LE: u8 = 1;
    pub(super) const GT: u8 = 2;
    pub(super) const GE: u8 = 3;
    pub(super) const EQ: u8 = 4;
    pub(super) const NE: u8 = 5;
}

/// Whether this machine can run the Rust intrinsics path.
///
/// The `COLUMNAR_FORCE_SIMD` override can turn this on for an ISA the CPU
/// probe does not advertise -- which is how the AVX2 path is correctness
/// tested on an ARM64 Mac under Rosetta 2, where the instructions execute
/// correctly but CPUID says the host has no AVX2. See [`crate::force`].
///
/// An override naming an ISA this binary was not compiled for is ignored:
/// the module for it does not exist.
#[must_use]
pub fn available() -> bool {
    use crate::force::{forced, ForcedSimd};
    match forced() {
        ForcedSimd::Auto => native_available(),
        ForcedSimd::None => false,
        ForcedSimd::Neon => cfg!(target_arch = "aarch64"),
        ForcedSimd::Avx2 => cfg!(target_arch = "x86_64"),
    }
}

/// Whether this machine's own CPU features cover the path, with no override.
///
/// This is the probe as it is in production: it is what decides whether the
/// SIMD path is safe to execute.
fn native_available() -> bool {
    #[cfg(target_arch = "aarch64")]
    {
        // NEON is architecturally mandatory on AArch64; there is no variant
        // of the target without it.
        true
    }
    #[cfg(target_arch = "x86_64")]
    {
        std::is_x86_feature_detected!("avx2")
    }
    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        false
    }
}

/// Compares `values` against `threshold` using the Rust intrinsics path.
///
/// Returns the number of matching rows. Falls back to the oracle when the
/// machine cannot run the intrinsics, so the result is always correct.
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
    if !available() {
        return crate::oracle::compare_i64(values, op, threshold, validity, out);
    }
    ROWS_PROCESSED.fetch_add(values.len() as u64, Ordering::Relaxed);
    #[cfg(target_arch = "aarch64")]
    {
        neon_i64(values, op, threshold, validity, out)
    }
    #[cfg(target_arch = "x86_64")]
    {
        avx2_i64(values, op, threshold, validity, out)
    }
    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        crate::oracle::compare_i64(values, op, threshold, validity, out)
    }
}

/// Compares `values` against `threshold` using the Rust intrinsics path.
///
/// Returns the number of matching rows.
///
/// # Panics
///
/// Panics if `out` is shorter than `(values.len() + 63) / 64`.
pub fn compare_f64(
    values: &[f64],
    op: ComparisonOp,
    threshold: f64,
    validity: Option<&NullBitmap>,
    out: &mut [u64],
) -> usize {
    if !available() {
        return crate::oracle::compare_f64(values, op, threshold, validity, out);
    }
    ROWS_PROCESSED.fetch_add(values.len() as u64, Ordering::Relaxed);
    #[cfg(target_arch = "aarch64")]
    {
        neon_f64(values, op, threshold, validity, out)
    }
    #[cfg(target_arch = "x86_64")]
    {
        avx2_f64(values, op, threshold, validity, out)
    }
    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        crate::oracle::compare_f64(values, op, threshold, validity, out)
    }
}

/// ANDs a block's match mask with the validity mask and writes it out.
#[inline]
fn finish_block(mask: u64, validity: Option<&NullBitmap>, block: usize, out: &mut [u64]) -> usize {
    let mask = match validity {
        Some(v) => {
            let words = v.words();
            let word = words.get(block).copied().unwrap_or(0);
            mask & word
        }
        None => mask,
    };
    if let Some(slot) = out.get_mut(block) {
        *slot = mask;
    }
    mask.count_ones() as usize
}

/// Computes the raw match mask for a tail block, using the oracle.
///
/// The returned mask covers only the tail rows and does **not** have the
/// validity mask applied: `finish_block` does that, so the validity mask is
/// applied in exactly one place for both the SIMD and the tail path.
fn tail_mask_i64(values: &[i64], op: ComparisonOp, threshold: i64) -> u64 {
    let mut words = vec![0u64; values.len().div_ceil(64)];
    crate::oracle::compare_i64(values, op, threshold, None, &mut words);
    words.first().copied().unwrap_or(0)
}

/// Computes the raw match mask for a tail block, using the oracle.
///
/// See [`tail_mask_i64`] for why the validity mask is applied later.
fn tail_mask_f64(values: &[f64], op: ComparisonOp, threshold: f64) -> u64 {
    let mut words = vec![0u64; values.len().div_ceil(64)];
    crate::oracle::compare_f64(values, op, threshold, None, &mut words);
    words.first().copied().unwrap_or(0)
}

// ---------------------------------------------------------------------------
// AArch64 / NEON
// ---------------------------------------------------------------------------

#[cfg(target_arch = "aarch64")]
mod neon_impl {
    use super::op;
    use core::arch::aarch64::{
        int64x2_t, uint64x2_t, vaddq_s64, vceqq_f64, vceqq_s64, vcgtq_f64, vcgtq_s64, vcltq_f64,
        vcltq_s64, vdupq_n_f64, vdupq_n_s64, vdupq_n_u64, veorq_u64, vgetq_lane_u64, vld1q_f64,
        vld1q_s64, vorrq_u64, vsetq_lane_s64, vshlq_u64, vshrq_n_u64,
    };

    /// Computes the match mask for one 64-row block of int64 values.
    ///
    /// The mask is accumulated in a 128-bit vector and reduced to 64 bits once,
    /// at the end of the block. Reducing it per pair of rows -- the way the
    /// first version of this kernel did -- put two vector-to-GPR moves on the
    /// critical path every two rows, and the kernel came out slower than the
    /// same predicate compiled from C++ with the vectoriser left on.
    ///
    /// # Safety
    ///
    /// `values` must be readable for 64 elements. NEON is architecturally
    /// mandatory on `AArch64`, so the unaligned load is always available.
    #[target_feature(enable = "neon")]
    pub(super) unsafe fn block_i64<const OP: u8>(values: *const i64, threshold: i64) -> u64 {
        // SAFETY: the caller guarantees 64 readable elements from `values`;
        // `k` stays below 64, so every load stays in bounds.
        unsafe {
            let mut lanes: uint64x2_t = vdupq_n_u64(0);
            // Lane 0 holds the even rows and lane 1 the odd rows, and both
            // halves use the final mask's own bit numbering. The shift
            // amounts are therefore {2k, 2k+1}, carried in a vector that
            // steps by two each iteration.
            // `vshlq_u64` takes the shift amounts as a signed vector.
            let mut shifts: int64x2_t = vsetq_lane_s64(1, vdupq_n_s64(0), 1);
            let step: int64x2_t = vdupq_n_s64(2);
            let mut k = 0usize;
            while k < 64 {
                let a = vld1q_s64(values.add(k));
                let b = vdupq_n_s64(threshold);
                // Rust's NEON comparison intrinsics return the mask as
                // `uint64x2_t` directly, with no reinterpret needed.
                let cmp: uint64x2_t = if OP == op::LT {
                    vcltq_s64(a, b)
                } else if OP == op::LE {
                    vorrq_u64(vcltq_s64(a, b), vceqq_s64(a, b))
                } else if OP == op::GT {
                    vcgtq_s64(a, b)
                } else if OP == op::GE {
                    vorrq_u64(vcgtq_s64(a, b), vceqq_s64(a, b))
                } else if OP == op::EQ {
                    vceqq_s64(a, b)
                } else {
                    veorq_u64(vceqq_s64(a, b), vdupq_n_u64(!0u64))
                };
                // Each lane of `cmp` is all-ones or all-zeros. Shift it right
                // by 63 to reduce it to a single bit, then place that bit at
                // the row's own position with a vector shift.
                let bit = vshrq_n_u64(cmp, 63);
                lanes = vorrq_u64(lanes, vshlq_u64(bit, shifts));
                shifts = vaddq_s64(shifts, step);
                k += 2;
            }
            vgetq_lane_u64(lanes, 0) | vgetq_lane_u64(lanes, 1)
        }
    }

    /// Computes the match mask for one 64-row block of float64 values.
    ///
    /// # Safety
    ///
    /// `values` must be readable for 64 elements.
    #[target_feature(enable = "neon")]
    pub(super) unsafe fn block_f64<const OP: u8>(values: *const f64, threshold: f64) -> u64 {
        // SAFETY: the caller guarantees 64 readable elements from `values`;
        // `k` stays below 64, so every load stays in bounds.
        unsafe {
            let mut lanes: uint64x2_t = vdupq_n_u64(0);
            // `vshlq_u64` takes the shift amounts as a signed vector.
            let mut shifts: int64x2_t = vsetq_lane_s64(1, vdupq_n_s64(0), 1);
            let step: int64x2_t = vdupq_n_s64(2);
            let mut k = 0usize;
            while k < 64 {
                let a = vld1q_f64(values.add(k));
                let b = vdupq_n_f64(threshold);
                let cmp: uint64x2_t = if OP == op::LT {
                    vcltq_f64(a, b)
                } else if OP == op::LE {
                    vorrq_u64(vcltq_f64(a, b), vceqq_f64(a, b))
                } else if OP == op::GT {
                    vcgtq_f64(a, b)
                } else if OP == op::GE {
                    vorrq_u64(vcgtq_f64(a, b), vceqq_f64(a, b))
                } else if OP == op::EQ {
                    vceqq_f64(a, b)
                } else {
                    // Ordered not-equal: false when either operand is NaN.
                    vorrq_u64(vcgtq_f64(a, b), vcltq_f64(a, b))
                };
                let bit = vshrq_n_u64(cmp, 63);
                lanes = vorrq_u64(lanes, vshlq_u64(bit, shifts));
                shifts = vaddq_s64(shifts, step);
                k += 2;
            }
            vgetq_lane_u64(lanes, 0) | vgetq_lane_u64(lanes, 1)
        }
    }
}

#[cfg(target_arch = "aarch64")]
fn neon_i64(
    values: &[i64],
    op: ComparisonOp,
    threshold: i64,
    validity: Option<&NullBitmap>,
    out: &mut [u64],
) -> usize {
    let blocks = values.len().div_ceil(64);
    let mut count = 0usize;
    let ptr = values.as_ptr();
    for block in 0..blocks {
        let remaining = values.len() - block * 64;
        if remaining < 64 {
            // Tail block: fewer than 64 rows remain. The oracle handles it so
            // the SIMD path never reads past the end of the slice.
            let tail = &values[block * 64..];
            let raw = tail_mask_i64(tail, op, threshold);
            count += finish_block(raw, validity, block, out);
            continue;
        }
        // SAFETY: 64 elements are readable from `ptr + block * 64`.
        let mask = unsafe {
            match op.abi_code() as u8 {
                op::LT => neon_impl::block_i64::<{ op::LT }>(ptr.add(block * 64), threshold),
                op::LE => neon_impl::block_i64::<{ op::LE }>(ptr.add(block * 64), threshold),
                op::GT => neon_impl::block_i64::<{ op::GT }>(ptr.add(block * 64), threshold),
                op::GE => neon_impl::block_i64::<{ op::GE }>(ptr.add(block * 64), threshold),
                op::EQ => neon_impl::block_i64::<{ op::EQ }>(ptr.add(block * 64), threshold),
                _ => neon_impl::block_i64::<{ op::NE }>(ptr.add(block * 64), threshold),
            }
        };
        count += finish_block(mask, validity, block, out);
    }
    count
}

#[cfg(target_arch = "aarch64")]
fn neon_f64(
    values: &[f64],
    op: ComparisonOp,
    threshold: f64,
    validity: Option<&NullBitmap>,
    out: &mut [u64],
) -> usize {
    let blocks = values.len().div_ceil(64);
    let mut count = 0usize;
    let ptr = values.as_ptr();
    for block in 0..blocks {
        let remaining = values.len() - block * 64;
        if remaining < 64 {
            let tail = &values[block * 64..];
            let raw = tail_mask_f64(tail, op, threshold);
            count += finish_block(raw, validity, block, out);
            continue;
        }
        // SAFETY: 64 elements are readable from `ptr + block * 64`.
        let mask = unsafe {
            match op.abi_code() as u8 {
                op::LT => neon_impl::block_f64::<{ op::LT }>(ptr.add(block * 64), threshold),
                op::LE => neon_impl::block_f64::<{ op::LE }>(ptr.add(block * 64), threshold),
                op::GT => neon_impl::block_f64::<{ op::GT }>(ptr.add(block * 64), threshold),
                op::GE => neon_impl::block_f64::<{ op::GE }>(ptr.add(block * 64), threshold),
                op::EQ => neon_impl::block_f64::<{ op::EQ }>(ptr.add(block * 64), threshold),
                _ => neon_impl::block_f64::<{ op::NE }>(ptr.add(block * 64), threshold),
            }
        };
        count += finish_block(mask, validity, block, out);
    }
    count
}

// ---------------------------------------------------------------------------
// x86-64 / AVX2
// ---------------------------------------------------------------------------

#[cfg(target_arch = "x86_64")]
mod avx2_impl {
    use super::op;
    use core::arch::x86_64::{
        _mm256_castsi256_pd, _mm256_cmp_pd, _mm256_cmpeq_epi64, _mm256_cmpgt_epi64,
        _mm256_loadu_pd, _mm256_loadu_si256, _mm256_movemask_pd, _mm256_or_si256,
        _mm256_set1_epi64x, _mm256_set1_pd, _mm256_xor_si256, _CMP_EQ_OQ, _CMP_GE_OQ, _CMP_GT_OQ,
        _CMP_LE_OQ, _CMP_LT_OQ, _CMP_NEQ_OQ,
    };

    /// Computes the match mask for one 64-row block of int64 values.
    ///
    /// # Safety
    ///
    /// `values` must be readable for 64 elements. The caller must have
    /// established that the CPU supports AVX2.
    #[target_feature(enable = "avx2")]
    pub(super) unsafe fn block_i64<const OP: u8>(values: *const i64, threshold: i64) -> u64 {
        let mut mask = 0u64;
        let mut k = 0usize;
        let b = _mm256_set1_epi64x(threshold);
        while k < 64 {
            // SAFETY: the caller guarantees 64 readable elements from `values`;
            // `k` stays below 64, so the load stays in bounds.
            let cmp = unsafe {
                let a = _mm256_loadu_si256(values.add(k).cast());
                if OP == op::LT {
                    _mm256_cmpgt_epi64(b, a)
                } else if OP == op::LE {
                    _mm256_or_si256(_mm256_cmpgt_epi64(b, a), _mm256_cmpeq_epi64(a, b))
                } else if OP == op::GT {
                    _mm256_cmpgt_epi64(a, b)
                } else if OP == op::GE {
                    _mm256_or_si256(_mm256_cmpgt_epi64(a, b), _mm256_cmpeq_epi64(a, b))
                } else if OP == op::EQ {
                    _mm256_cmpeq_epi64(a, b)
                } else {
                    _mm256_xor_si256(_mm256_cmpeq_epi64(a, b), _mm256_set1_epi64x(-1))
                }
            };
            // `movemask` yields one bit per 64-bit lane -- four bits -- and
            // lane `i` holds row `k + i`, so the block is four rows per step.
            let bits = _mm256_movemask_pd(_mm256_castsi256_pd(cmp)) as u32;
            mask |= u64::from(bits) << k;
            k += 4;
        }
        mask
    }

    /// Computes the match mask for one 64-row block of float64 values.
    ///
    /// # Safety
    ///
    /// `values` must be readable for 64 elements. The caller must have
    /// established that the CPU supports AVX2.
    #[target_feature(enable = "avx2")]
    pub(super) unsafe fn block_f64<const OP: u8>(values: *const f64, threshold: f64) -> u64 {
        let mut mask = 0u64;
        let mut k = 0usize;
        let b = _mm256_set1_pd(threshold);
        while k < 64 {
            // SAFETY: the caller guarantees 64 readable elements from `values`.
            let cmp = unsafe {
                let a = _mm256_loadu_pd(values.add(k));
                if OP == op::LT {
                    _mm256_cmp_pd(a, b, _CMP_LT_OQ)
                } else if OP == op::LE {
                    _mm256_cmp_pd(a, b, _CMP_LE_OQ)
                } else if OP == op::GT {
                    _mm256_cmp_pd(a, b, _CMP_GT_OQ)
                } else if OP == op::GE {
                    _mm256_cmp_pd(a, b, _CMP_GE_OQ)
                } else if OP == op::EQ {
                    _mm256_cmp_pd(a, b, _CMP_EQ_OQ)
                } else {
                    _mm256_cmp_pd(a, b, _CMP_NEQ_OQ)
                }
            };
            let bits = _mm256_movemask_pd(cmp) as u32;
            mask |= u64::from(bits) << k;
            k += 4;
        }
        mask
    }
}

#[cfg(target_arch = "x86_64")]
fn avx2_i64(
    values: &[i64],
    op: ComparisonOp,
    threshold: i64,
    validity: Option<&NullBitmap>,
    out: &mut [u64],
) -> usize {
    let blocks = values.len().div_ceil(64);
    let mut count = 0usize;
    let ptr = values.as_ptr();
    for block in 0..blocks {
        let remaining = values.len() - block * 64;
        if remaining < 64 {
            let tail = &values[block * 64..];
            let raw = tail_mask_i64(tail, op, threshold);
            count += finish_block(raw, validity, block, out);
            continue;
        }
        // SAFETY: 64 elements are readable from `ptr + block * 64`, and
        // `available()` established that the CPU supports AVX2.
        let mask = unsafe {
            match op.abi_code() as u8 {
                op::LT => avx2_impl::block_i64::<{ op::LT }>(ptr.add(block * 64), threshold),
                op::LE => avx2_impl::block_i64::<{ op::LE }>(ptr.add(block * 64), threshold),
                op::GT => avx2_impl::block_i64::<{ op::GT }>(ptr.add(block * 64), threshold),
                op::GE => avx2_impl::block_i64::<{ op::GE }>(ptr.add(block * 64), threshold),
                op::EQ => avx2_impl::block_i64::<{ op::EQ }>(ptr.add(block * 64), threshold),
                _ => avx2_impl::block_i64::<{ op::NE }>(ptr.add(block * 64), threshold),
            }
        };
        count += finish_block(mask, validity, block, out);
    }
    count
}

#[cfg(target_arch = "x86_64")]
fn avx2_f64(
    values: &[f64],
    op: ComparisonOp,
    threshold: f64,
    validity: Option<&NullBitmap>,
    out: &mut [u64],
) -> usize {
    let blocks = values.len().div_ceil(64);
    let mut count = 0usize;
    let ptr = values.as_ptr();
    for block in 0..blocks {
        let remaining = values.len() - block * 64;
        if remaining < 64 {
            let tail = &values[block * 64..];
            let raw = tail_mask_f64(tail, op, threshold);
            count += finish_block(raw, validity, block, out);
            continue;
        }
        // SAFETY: 64 elements are readable from `ptr + block * 64`, and
        // `available()` established that the CPU supports AVX2.
        let mask = unsafe {
            match op.abi_code() as u8 {
                op::LT => avx2_impl::block_f64::<{ op::LT }>(ptr.add(block * 64), threshold),
                op::LE => avx2_impl::block_f64::<{ op::LE }>(ptr.add(block * 64), threshold),
                op::GT => avx2_impl::block_f64::<{ op::GT }>(ptr.add(block * 64), threshold),
                op::GE => avx2_impl::block_f64::<{ op::GE }>(ptr.add(block * 64), threshold),
                op::EQ => avx2_impl::block_f64::<{ op::EQ }>(ptr.add(block * 64), threshold),
                _ => avx2_impl::block_f64::<{ op::NE }>(ptr.add(block * 64), threshold),
            }
        };
        count += finish_block(mask, validity, block, out);
    }
    count
}
