//! `CodSpeed` benchmarks for the comparison kernels.
//!
//! Every flavor of the kernel -- the C++ scalar baseline, the C++
//! auto-vectorised build, the hand-written C++ SIMD path and the Rust
//! intrinsics path -- is measured on the same input, for both value types,
//! with and without a validity mask. The oracle is measured too, because it is
//! the reference the others are held to.
//!
//! The inputs are deterministic: a fixed-seed xorshift fills the columns, so
//! two runs of the benchmark see the same bytes.

use columnar_core::{ComparisonOp, NullBitmap};
use columnar_kernels::{compare_f64_into, compare_i64_into, oracle, Flavor};
use divan::{black_box, Bencher};

/// Rows per column: one engine batch's worth, sixteen times over.
const ROWS: usize = 65_536;

/// Every implementation the engine can dispatch to.
const FLAVORS: [Flavor; 4] = Flavor::all();

fn main() {
    divan::main();
}

/// A deterministic xorshift stream.
fn xorshift(seed: u64) -> impl Iterator<Item = u64> {
    let mut state = seed;
    std::iter::repeat_with(move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    })
}

/// Integers in `[0, 10_000)`.
fn int_values() -> Vec<i64> {
    xorshift(0x5EED)
        .take(ROWS)
        .map(|word| i64::try_from(word % 10_000).expect("the value fits"))
        .collect()
}

/// Doubles in `[0, 1_000)` with two decimals, like a price column.
fn float_values() -> Vec<f64> {
    xorshift(0xF10A7)
        .take(ROWS)
        .map(|word| f64::from(u32::try_from(word % 100_000).expect("the value fits")) / 100.0)
        .collect()
}

/// A validity mask with every seventh row NULL.
fn validity() -> NullBitmap {
    let nulls: Vec<usize> = (0..ROWS).step_by(7).collect();
    NullBitmap::with_nulls(ROWS, &nulls)
}

/// The match mask buffer the hot path reuses.
fn mask() -> Vec<u64> {
    vec![0; ROWS.div_ceil(64)]
}

#[divan::bench(args = FLAVORS)]
fn compare_i64_lt(bencher: Bencher<'_, '_>, flavor: Flavor) {
    let values = int_values();
    let mut out = mask();
    bencher.bench_local(|| {
        compare_i64_into(
            flavor,
            black_box(&values),
            ComparisonOp::Lt,
            black_box(5_000),
            None,
            &mut out,
        )
    });
}

#[divan::bench(args = FLAVORS)]
fn compare_i64_eq_nullable(bencher: Bencher<'_, '_>, flavor: Flavor) {
    let values = int_values();
    let validity = validity();
    let mut out = mask();
    bencher.bench_local(|| {
        compare_i64_into(
            flavor,
            black_box(&values),
            ComparisonOp::Eq,
            black_box(42),
            Some(&validity),
            &mut out,
        )
    });
}

#[divan::bench(args = FLAVORS)]
fn compare_f64_gte(bencher: Bencher<'_, '_>, flavor: Flavor) {
    let values = float_values();
    let mut out = mask();
    bencher.bench_local(|| {
        compare_f64_into(
            flavor,
            black_box(&values),
            ComparisonOp::GtEq,
            black_box(500.0),
            None,
            &mut out,
        )
    });
}

#[divan::bench(args = FLAVORS)]
fn compare_f64_lte_nullable(bencher: Bencher<'_, '_>, flavor: Flavor) {
    let values = float_values();
    let validity = validity();
    let mut out = mask();
    bencher.bench_local(|| {
        compare_f64_into(
            flavor,
            black_box(&values),
            ComparisonOp::LtEq,
            black_box(250.0),
            Some(&validity),
            &mut out,
        )
    });
}

#[divan::bench]
fn oracle_i64_lt(bencher: Bencher<'_, '_>) {
    let values = int_values();
    let mut out = mask();
    bencher.bench_local(|| {
        oracle::compare_i64(black_box(&values), ComparisonOp::Lt, black_box(5_000), None, &mut out)
    });
}

#[divan::bench]
fn oracle_f64_gte(bencher: Bencher<'_, '_>) {
    let values = float_values();
    let mut out = mask();
    bencher.bench_local(|| {
        oracle::compare_f64(
            black_box(&values),
            ComparisonOp::GtEq,
            black_box(500.0),
            None,
            &mut out,
        )
    });
}
