# ADR-0002: Dual-architecture SIMD — NEON on the development machine, AVX2 on a self-hosted runner

**Status**: accepted
**Date**: 2026-10-23
**Reviewers**: TBD

## Context

The portfolio blueprint that this project implements specifies the headline kernel as an AVX2
filter kernel, quoted at 6.7× the scalar reference on an x86-64 host.

The development machine is an **Apple M4 Max (aarch64)**, and no x86-64 host is available. That
creates three possible designs:

1. **AVX2 only.** Keep the blueprint's headline verbatim. Develop against the scalar path locally
   and benchmark on a rented x86-64 box.
2. **NEON only.** Port the kernel to NEON, which is what the development machine actually has.
   Every number is reproducible on the machine the code is written on, but the x86-64 story is
   gone.
3. **Both.** Write one kernel source that compiles to NEON on AArch64 and AVX2 on x86-64, measure
   NEON locally, and measure AVX2 on a self-hosted x86-64 runner.

The review that hardened this design is explicit that the kernel must have a runtime feature probe
and a scalar fallback (finding 2.4), because "Apple Silicon, ARM servers, and most CI runners
cannot run the AVX2 kernel". A design that only works on x86-64-with-AVX2 is not a design that
satisfies that finding.

## Decision

**Both.** The comparison kernel is written once and compiled twice.

- A **NEON** path for AArch64, where NEON is architecturally mandatory and needs no probe.
- An **AVX2** path for x86-64, reached through `__attribute__((target("avx2")))` after a
  `__builtin_cpu_supports("avx2")` check. The translation unit is **never** compiled with a
  global `-mavx2` (see ADR-0004).
- A **scalar** reference, always present, used both as the A/B baseline and as the fallback on a
  machine with neither path.

The Rust intrinsics path mirrors the same three-way split, so that the FFI seam's cost can be
measured on both architectures (ADR-0003).

CI runs the benchmark on two runner classes: `macos-latest` for the NEON path, and a
self-hosted x86-64 runner for the AVX2 path. The self-hosted job is a separate workflow that only
runs when a runner with the `self-hosted, linux, x64` label is registered — so the repository
builds and tests correctly for anyone who clones it, and the AVX2 numbers are published by whoever
has the runner.

## Consequences

**Easier**: every number on the development machine is reproducible by the person who wrote the
code. The x86-64 audience is not abandoned. The scalar-fallback requirement of finding 2.4 is
satisfied by construction rather than by an afterthought.

**Harder**: two SIMD implementations to write, test and keep in agreement — which is why the
differential test suite is a release gate and not an optional extra. The published numbers are
**not portable between architectures**, and each number must be published with the architecture it
was measured on. That is a feature: a number without its machine is a claim.

**Given up**: the single headline number of the blueprint. The published figure becomes "the
kernel is N× the scalar reference on NEON" and "M× on AVX2", not one figure. This is the
consequence the review's finding 2.13 already demanded in a different form: the honest framing is
always narrower than the impressive one.

## Measurement

The two figures are published in `PERF.md`, each labelled with the architecture, the CPU model and
the SIMD path in use. The scalar baseline is the same code compiled without vectorisation, run
in the same process on the same data — so the ratio measures vectorisation, not algorithm
difference.

The self-hosted AVX2 runner is **not yet registered**; the AVX2 column of `PERF.md` is therefore
marked as not-yet-measured rather than estimated. Publishing an estimated number would be
exactly the failure mode this project exists to avoid.

## Confidence

**high** — for the NEON path, which is measured. **medium** — for the AVX2 path, whose code is
written and reviewed but whose number is pending a runner. The AVX2 implementation is structurally
identical to the NEON one (same block decomposition, same mask assembly, same validity handling)
and shares the differential test suite, so the correctness risk is low; the *performance* claim is
simply not yet made.
