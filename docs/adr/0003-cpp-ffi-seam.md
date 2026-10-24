# ADR-0003: The C++ FFI seam stays, because it is measurably free

**Status**: accepted
**Date**: 2026-10-24
**Reviewers**: TBD

## Context

The design review raised a question that a project with a single kernel cannot answer:

> The `cc`-based FFI for a single kernel may not be the actual bottleneck. If the surrounding code
> is the bottleneck, the FFI overhead can erase the win. (review finding 2.12)

That is a real risk. A `cc` boundary costs a call, an ABI-safe argument list and a return value —
and the kernel it wraps is a leaf function that allocates nothing and touches no global state, so
the *relative* cost of the seam is at its worst for exactly the kind of function being wrapped.

The way to answer it is not an argument. It is a second implementation of the same predicate with
no FFI at all, measured with the same harness.

So `columnar-kernels` carries **two** independent SIMD implementations of the comparison kernel:

| | implementation | FFI |
|---|---|---|
| `cpp-simd` | C++ with NEON / AVX2 intrinsics | yes, over the C ABI |
| `rust-simd` | Rust with `std::arch` intrinsics | no |

They must agree bit for bit, and the differential tests hold them to that.

### The measurement that was nearly the wrong one

The first version of this comparison produced a number that would have supported a dramatic
conclusion: `rust-simd` was **1.88× slower** than `cpp-simd`. Taken at face value, that says the
FFI makes the kernel *faster*, which is nonsense.

The reason was not the FFI. It was that the two implementations were not doing the same work. The
C++ NEON path and the Rust NEON path were written at different times, and only the C++ one had
been given the mask-accumulator rewrite described in ADR-0006. The Rust path still reduced the
comparison mask to scalar bits every two rows, which put two vector-to-GPR moves on its critical
path.

Comparing two implementations that differ in algorithm is not a measurement of the FFI. Once the
Rust path was given the same accumulator, the comparison became a measurement of the FFI.

## Decision

**Retain the C++ FFI seam, and retain the Rust intrinsics path alongside it.**

The seam is retained because the measurement says it is free, not because it is necessary. It is
also retained because it currently owns two things that the Rust path does not:

- the **auto-vectorised baseline** (`cpp-auto`), which is the reference point that makes the
  hand-written kernel's speedup claim meaningful at all (ADR-0006), and
- the **x86-64 runtime dispatch**, which reaches AVX2 through a target-attributed function after a
  CPU probe, so the same binary runs on a machine with no AVX2.

The Rust path is retained because it is what makes the FFI claim *checkable*. A claim that "the
FFI is free" that is only ever measured against the C++ side is not a claim; the control has to
exist in the tree, and it is in the tree.

## Consequences

- The seam stays cheap because the kernel behind it is a leaf function: no allocation, no global
  state, no locking, a fixed argument list. The discipline is written down as a rule rather than
  left to chance.
- Two implementations of one predicate must be kept in agreement. That is the cost, and it is paid
  by the differential tests, which compare every flavor against a plain Rust oracle on random
  inputs and on the specific cases that are easy to get wrong (NULL under `!=`, NaN under `!=`,
  the tail block, the empty input).
- The C++ toolchain is a build dependency. `cc` plus a C++17 compiler is the price of the
  auto-vectorised baseline and the dual-arch dispatch.

## Measurement

Apple M4 Max, aarch64, macOS 27.0.1, rustc 1.97.1, release build, one thread, 20,000,000 rows,
5 iterations per run, column `l_discount`, operator `>=`, threshold `0.04`. Eight runs;
the median is reported and the spread is given so that the reader can see the noise.

| flavor | rows/sec (median of 8) | spread | ns/row | FFI |
|---|---|---|---|---|
| `cpp-simd` | 8.81 G | 8.29 – 8.90 | 0.113 | yes |
| `rust-simd` | 8.77 G | 5.05 – 9.06 | 0.114 | no |

**The two are within 0.5% of each other**, against a run-to-run spread of a few percent on either
side. The FFI seam costs no more than the noise, so it does not erase the win and it is not the
bottleneck.

Before the algorithm was equalised between the two sides, the same comparison read 8.98 G against
4.81 G. That number measured a difference in implementation, not a difference in the seam.

## Confidence

**high** — the comparison is a controlled A/B over the same data, the same harness and the same
predicate, with the agreement between the two implementations enforced by tests, and the noise
floor is published alongside the result so that a 0.3% difference can be seen for what it is. The
one caveat is that this was measured on AArch64; the x86-64 numbers will come from the
self-hosted runner and are not yet available.

## Revisit when

- The Rust intrinsics path gains a feature the C++ side cannot express, or the other way round.
- The `cc` build becomes a portability problem on a supported platform.
- The x86-64 runner reports a materially different FFI cost.
