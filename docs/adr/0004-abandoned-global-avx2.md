# ADR-0004: Abandoned — a global `-mavx2` build, and `vshrn_n_u64` for lane extraction

**Status**: abandoned
**Date**: 2026-10-23
**Reviewers**: TBD

## Context

Two approaches to the x86-64 SIMD path were tried and abandoned.

### The global `-mavx2` build

The obvious way to get AVX2 in the C++ translation unit is to compile the whole file with
`-mavx2`:

```cpp
// build.rs, the version that was not shipped
build.flag_if_supported("-mavx2").compile("columnar_comparison");
```

### `vshrn_n_u64` for lane extraction

The SIMD comparison produces a mask with all-ones or all-zeros in each 64-bit lane. The natural
ACLE spelling for "reduce each lane to its top bit" is a narrowing shift:

```cpp
const uint32x2_t narrow = vshrn_n_u64(cmp, 63);
```

## Decision

**Both abandoned.**

### The global `-mavx2` build

**Abandoned.** The translation unit is compiled for the baseline ISA, and the AVX2 path is reached
through `__attribute__((target("avx2")))` on the individual functions, guarded by
`__builtin_cpu_supports("avx2")`.

**Why**: a translation unit compiled with `-mavx2` may emit AVX2 instructions *anywhere* in it,
including in the scalar fallback that exists precisely for machines without AVX2. On such a
machine the process would fault before it had a chance to detect the absence and fall back. The
kernel would not be portable at all — it would be an x86-64-with-AVX2-only kernel wearing a
runtime-dispatch hat, which is the exact failure mode the review's finding 2.4 warns about.

**What it cost**: the AVX2 functions need an explicit `__attribute__((target("avx2")))` on each
one, and the `build.rs` needs a comment explaining why the obvious flag is absent. That is a
small, one-off cost for a kernel that actually runs on non-AVX2 hardware.

### `vshrn_n_u64` for lane extraction

**Abandoned.** The ACLE `vshrn_n_u64` shift range is `[1, 32]`, because it narrows 64-bit lanes to
32-bit lanes. A shift of 63 is not expressible. The replacement is `vshrq_n_u64` followed by
reading the two lanes separately.

**What it cost**: one compile error, caught by the compiler, and one more step in the mask
assembly that has to be understood by anyone reviewing the kernel.

## Consequences

The AVX2 path is a few lines longer than it would otherwise be, and the `build.rs` carries a
comment where a flag would be expected. Both are the price of a kernel that runs correctly on a
machine without AVX2.

## Measurement

The cost of the abandoned approach is not a benchmark number; it is a fault on a non-AVX2
machine, which is exactly the kind of failure that a benchmark on the development machine cannot
reveal. The scalar path is exercised by the differential test suite on every machine, which is
the mechanism that keeps this honest.

The `vshrn_n_u64` failure was caught by the C++ compiler at build time — the same build that
runs in CI on every platform.

## Confidence

**high** — the portability argument is a property of how code generation works, not a measurement,
and the lane-extraction failure was deterministic and immediate.
