# columnar-kernels

**The comparison kernels: deciding which rows of a column satisfy a predicate.** This crate owns
the single hottest operation in the engine. Every implementation of that one operation must agree
with every other one, **bit for bit** — and the differential suite compares the **masks**, not the
counts.

| Implementation | What it is | Why it exists |
|---|---|---|
| `oracle::compare_i64` / `oracle::compare_f64` | a plain Rust loop | the correctness reference |
| C++ scalar | the same predicate, branchless, compiled with `-fno-vectorize` | the A/B **baseline**: what vectorisation is worth |
| C++ auto | the same source, compiled with ordinary flags | what the compiler does on its own — the bar the hand-written kernel has to clear |
| C++ SIMD | `NEON` on `AArch64`, `AVX2` on x86-64, over the FFI seam | the headline kernel |
| Rust SIMD | `std::arch` intrinsics, no FFI | measures what the FFI seam costs |

`Flavor` is the public selection enum: what a caller picks.

---

## A runnable example

```rust
use columnar_core::{ComparisonOp, NullBitmap};
use columnar_kernels::{best, compare_i64, oracle, Flavor};

fn main() {
    // Four rows: two match, one is NULL, and one does not match.
    let values = [10_i64, 10, 20, 30];
    let validity = NullBitmap::with_nulls(4, &[2]);

    // The oracle is the reference: a plain Rust loop. It writes the mask and
    // returns the number of rows that matched.
    let mut oracle_mask = [0u64; 1];
    let matched = oracle::compare_i64(
        &values,
        ComparisonOp::Eq,
        10,
        Some(&validity),
        &mut oracle_mask,
    );

    // The kernel is the fast path, and it agrees with the oracle bit for bit.
    let kernel_mask = compare_i64(best(), &values, ComparisonOp::Eq, 10, Some(&validity));

    assert_eq!(matched, 2);
    assert_eq!(oracle_mask, kernel_mask[..]);

    // The flavors this machine can run, most-preferred first.
    let flavors = columnar_kernels::available();
    assert_eq!(best(), flavors.first().copied().unwrap_or(Flavor::Scalar));
}
```

The hot path does not allocate its mask: `compare_i64_into` writes into a buffer the caller owns
and reuses, and `compare_i64` is the convenience wrapper that allocates one for you.

---

## The guarantees

**Every implementation agrees, bit for bit.** The differential suite generates random inputs and
compares the masks the implementations produce, not the number of rows that matched. Two
implementations that counted the same rows but set different bits would fail it.

**A NULL never matches.** A row whose validity bit is clear is not a candidate for any comparison
at all, for every operator, `!=` included. The mask says which rows the comparison is allowed to
look at.

**A NaN never matches.** A comparison whose value or whose threshold is a NaN never matches, for
every operator, `!=` included. This is "unknown propagates", and it is stricter than SQL's
treatment of NaN.

**The mask layout is the contract.** Row `i` lives in word `i / 64` at bit `i % 64`,
least-significant-bit first — the layout `DuckDB`, `Arrow` and the Parquet validity encoding all
use, so the mask goes to a comparator without a repack.

**The operator's C ABI code is pinned.** `ComparisonOp::abi_code` is the integer the C++ side
switches on, and a test asserts the mapping against the C++ header. Changing one without changing
the other silently changes what the filter does.

**There are two baselines, not one.** The scalar reference is compiled twice from the same
source: once with the vectoriser switched off (`Flavor::Scalar`) and once with it on
(`Flavor::CppAuto`). The A/B publishes both ratios — `scalar → cpp-simd` for what vectorisation
is worth, and `cpp-auto → cpp-simd` for whether the hand-written kernel beats what the compiler
does with the same loop. The second question has no predetermined answer.

---

## The boundaries

**The kernel decides one thing.** Which rows match. It does not open a file, does not decode
Parquet, does not gather a column and does not aggregate. Everything around the comparison belongs
to the caller.

**The output is a bit mask.** The kernel says which rows survived; it does not own them, and it
does not return the values.

**The crate reads nothing.** It opens no file and speaks no transport. Its input is a slice of
values and an optional validity mask, both owned by the caller.

**The FFI seam is `unsafe`, and this is the only crate in the workspace that has any.** Both
`columnar-core` and `columnar-engine` carry `#![deny(unsafe_code)]`. A panic must never unwind
across `extern "C"`, which is why the release profile aborts on panic. The `ffi` module is not
compiled for the browser target.

**The browser target has no C++ at all.** On `wasm32` the C++ flavors are absent from
`available()`, and the Rust intrinsics path is the only SIMD path there is.

**`Flavor::RustSimd` exists to be compared, not to be preferred.** It runs the same predicate
through Rust intrinsics with no FFI. If it and `Flavor::CppSimd` measure the same, the seam is
free and the C++ stays; if it does not, the C++ story has to go. Both numbers are published.

---

## Where to go next

The **embedding guide** —
[`docs/embedding.md`](https://github.com/QuickLeopard/columnar-engine/blob/main/docs/embedding.md) —
covers the crate that calls these kernels: the knobs it exposes, and the plan output that reports
which flavor ran.

The **measured numbers** are in
[`PERF.md`](https://github.com/QuickLeopard/columnar-engine/blob/main/PERF.md), which publishes the
kernel's A/B and the conditions it was taken under.
