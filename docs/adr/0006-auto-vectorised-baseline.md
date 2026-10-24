# ADR-0006: The "scalar" baseline was not scalar — and the fix was the mask accumulator

**Status**: accepted
**Date**: 2026-10-24
**Reviewers**: TBD

## Context

This project publishes one headline number: how much faster the vectorised comparison kernel is
than the scalar reference. The first version of that number was wrong, and it was wrong in the
way that makes a benchmark look better than it is.

The baseline was a C++ loop that computed the predicate and folded the result into a bit mask. It
was called "the scalar reference". Disassembling the compiled object showed it was not scalar at
all:

```
$ xcrun llvm-objdump -d --no-show-raw-insn target/release/build/columnar-kernels-*/out/*-comparison.o
```

Inside the symbol `col_compare_i64_auto` — the function that *was* supposed to be the baseline —
the compiler had emitted 296 vector instructions: `fcmgt.2d`, `ushl.2d`, `movi.2d`, `dup.2d`,
`orr.16b`. Clang had auto-vectorised the loop.

So the published ratio was comparing **two vectorised implementations** and calling it a
vectorisation speedup. The baseline had been silently upgraded to a competitor.

### Finding 1: a baseline you have not disassembled is not a baseline

The fix for the measurement was mechanical: the same scalar source is compiled twice, into two
translation units, with different flags.

| translation unit | flags | exposed as |
|---|---|---|
| `comparison.cpp` | ordinary | `col_compare_*_auto` — the compiler vectorises it |
| `scalar.cpp` | `-fno-vectorize` | `col_compare_*_scalar` — genuinely scalar |

The verification is not a claim, it is a disassembly: the `-fno-vectorize` object contains **zero**
vectorised loop instructions, and the ordinary object contains ~300.

With an honest baseline in place, the first corrected measurement was:

| flavor | vs scalar | vs compiler-vectorised |
|---|---|---|
| `scalar` (real) | 1.00× | 0.61× |
| `cpp-auto` (compiler) | 1.68× | 1.00× |
| `cpp-simd` (hand-written NEON) | 2.47× | **0.84×** |

The hand-written NEON kernel was **slower than the compiler's**. 8.63 G rows/sec against
5.87 G rows/sec — it lost.

### Finding 2: why the hand-written kernel lost

The hand-written path reduced the comparison mask to scalar bits every two rows:

```cpp
const uint64x2_t shifted = vshrq_n_u64(cmp, 63);
const uint64_t b0 = vgetq_lane_u64(shifted, 0) & 1u;   // umov
const uint64_t b1 = vgetq_lane_u64(shifted, 1) & 1u;   // umov
mask |= static_cast<uint64_t>(b0 | (b1 << 1)) << (k * 2);
```

Two vector-to-GPR moves per two rows, each with real latency, on the critical path of the mask
accumulation. The compiler's version did not do this: it kept the mask in a 128-bit register and
extracted once per 64-row block.

### Finding 3: the fix

The mask can be accumulated in a vector register if the two 64-bit halves are given the same bit
numbering as the final mask. Lane 0 collects the even-numbered rows at their own bit positions,
lane 1 collects the odd-numbered rows at their own bit positions, and the two halves are OR-ed
together once, at the end of the block:

```cpp
struct NeonMaskAccumulator {
    uint64x2_t lanes;   // the two halves of the block's mask
    uint64x2_t shifts;  // {2k, 2k+1}, stepped by two each iteration

    inline void push(const uint64x2_t cmp) {
        const uint64x2_t bit = vshrq_n_u64(cmp, 63);   // 0 or 1 per lane
        lanes = vorrq_u64(lanes, vshlq_u64(bit, shifts));
        shifts = vaddq_u64(shifts, vdupq_n_u64(2));
    }

    inline uint64_t finish() const {
        return vgetq_lane_u64(lanes, 0) | vgetq_lane_u64(lanes, 1);
    }
};
```

The extraction count drops from 64 per block to 1. The same change was applied to the Rust
intrinsics path, so that the two remain comparable implementations of one algorithm (ADR-0003).

After the fix the hand-written NEON kernel went from **0.84×** the compiler's to **1.50×**.

## Decision

**Always publish two baselines, and never publish a speedup against an unnamed baseline.**

- The `scalar` flavor is compiled with `-fno-vectorize` and answers *"how much is vectorisation
  worth?"*.
- The `cpp-auto` flavor is the same source with the vectoriser left on and answers the harder
  question: *"does the hand-written kernel beat what the compiler does with the same loop?"*

The second is the number that decides whether the hand-written kernel is worth its complexity. A
hand-written SIMD kernel that does not beat the compiler has not earned its keep, and a project
that only published the first ratio would never have noticed that its kernel was losing.

The same discipline applies to the Rust intrinsics path: it was rewritten with the accumulator not
because Rust needed it, but because leaving the two implementations on different algorithms would
have made the FFI comparison meaningless (ADR-0003).

## Consequences

- The published kernel table has two ratio columns instead of one, and the baseline row is
  labelled for what it is.
- `scalar_impl.h` exists so the same source can be compiled twice; the flags live in `build.rs`,
  because a header cannot carry them.
- Every future kernel needs two baselines measured against it, and the build has to keep both
  translation units alive.
- The disassembly check is now part of the story, not a one-off investigation: the claim "the
  baseline is scalar" is backed by an instruction histogram, not by the name of a function.

## Measurement

Apple M4 Max, aarch64, macOS 27.0.1, rustc 1.97.1, release build, one thread, 20,000,000 rows,
5 iterations per run, column `l_discount`, operator `>=`, threshold `0.04`. Eight runs;
the median is reported and the spread is given so that the reader can see the noise.

| flavor | rows/sec (median of 8) | ns/row | vs scalar | vs cpp-auto |
|---|---|---|---|---|
| `scalar` (`-fno-vectorize`) | 3.51 G | 0.285 | 1.00× | 0.60× |
| `cpp-auto` (compiler) | 5.87 G | 0.170 | 1.67× | 1.00× |
| `cpp-simd` (hand-written NEON) | 8.81 G | 0.113 | 2.51× | **1.50×** |
| `rust-simd` (Rust intrinsics) | 8.77 G | 0.114 | 2.50× | 1.49× |

> These are the figures this decision rested on, and an ADR is a record rather than a live
> dashboard. A later 8-run set on the same machine reproduced them to within 2% — 3.48 G, 5.75 G,
> 8.73 G, 8.72 G, with the same 2.50× and 1.50× ratios. The current figures are in
> [`PERF.md`](../../PERF.md); the decision this ADR records does not change.

All four flavors agree on 12,726,115 matching rows, and the differential tests compare the
masks bit for bit, not just the counts.

The hand-written kernel before the accumulator rewrite measured 4.81 G rows/sec (0.84× the
compiler). The rewrite moved it from 4.81 G to 8.81 G — a 1.83× improvement in the kernel, from
removing two instructions per two rows.

## Confidence

**high** for the diagnosis and the fix: the instruction histogram explains the gap, the rewrite is
small and local, and the effect is 1.79× on the kernel — far too large to be noise. The
measurement is on AArch64 only; the AVX2 path has the same structure (it uses `_mm256_movemask_pd`,
which extracts directly and never had the problem) but its numbers are not yet measured, because
there is no x86-64 machine. Its *correctness* has since been verified, on an ARM64 Mac under
Rosetta 2 — see [ADR-0009](0009-avx2-without-x86-hardware.md).

## Revisit when

- The x86-64 runner reports its numbers, and the AVX2 path needs the same two-baseline treatment.
- A future kernel is added to the crate and needs the same treatment.
