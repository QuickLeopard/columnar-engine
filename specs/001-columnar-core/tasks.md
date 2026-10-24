# Tasks 001 — Vectorized engine core

**Date**: 2026-10-23 · **Revised**: 2026-10-24
**Spec**: `spec.md` | **Plan**: `plan.md`

Status: `[x]` done · `[~]` in progress · `[ ]` not started

## T1 — The vocabulary (`columnar-core`)

- [x] T1.1 Physical types (`Int64`, `Float64`, `Utf8`) and scalar values.
- [x] T1.2 Validity masks: bit-packed, LSB-first, canonical padding, recomputed null count.
- [x] T1.3 Encodings: plain and dictionary forms per physical type, plus RLE for booleans.
- [x] T1.4 Column chunks: name, encoding, validity, row count, decode and gather.
- [x] T1.5 Selection vectors: index and bitmap representations, density dispatch at a named
      threshold, canonical padding.
- [x] T1.6 Batches: the unit of work, bounded at 2048 rows.
- [x] T1.7 Predicates: the six comparison operators and their stable ABI codes.
- [x] T1.8 Unit tests for every type, including canonicalisation and dispatch.

## T2 — The kernel (`columnar-kernels`)

- [x] T2.1 The Rust oracle — the correctness reference.
- [x] T2.2 The C ABI: the SIMD dispatch entry points, **two** scalar variants of each, and a SIMD
      probe. (Revised: one scalar variant was not enough — see T8.2.)
- [x] T2.3 The C++ branchless scalar reference, in a header so it can be compiled twice.
- [x] T2.4 The C++ NEON path, with partial-block handling.
- [x] T2.5 The C++ AVX2 path behind `__attribute__((target("avx2")))` and a runtime probe.
- [x] T2.6 The Rust intrinsics path — the FFI-seam measurement.
- [x] T2.7 The public flavors: `Scalar`, `CppAuto`, `CppSimd`, `RustSimd`. (Revised: see T8.3.)
- [x] T2.8 The differential test suite: all flavors agree with the oracle, bit for bit.
- [x] T2.9 Explicit tests: NULL rows, NaN values, NaN thresholds, block boundaries, empty input,
      bits beyond the end of the input.

## T3 — The dataset (`columnar-testdata`)

- [x] T3.1 splitmix64 + xoshiro256** — the PRNG, written here because `rand` is banned.
- [x] T3.2 The `lineitem` schema, in the exact column order the engine expects.
- [x] T3.3 The row generator, with the TPC-H value distributions.
- [x] T3.4 The Parquet writer, with dictionary encoding on the seven low-cardinality columns.
- [x] T3.5 The JSON manifest: seed, scale factor, rows, bytes, null rate, provenance.
- [x] T3.6 Tests: determinism, schema, dictionary encoding, null rate, value ranges.

## T4 — The engine (`columnar-engine`)

- [x] T4.1 The Parquet reader: Arrow arrays to the engine's own column chunks.
- [x] T4.2 The filter operator, using the kernel and building a selection vector.
- [x] T4.3 The project operator, gathering by the selection vector.
- [x] T4.4 The hash aggregate, with deterministic group ordering and NULL-aware aggregates.
- [x] T4.5 The in-place `COUNT(*)` path.
- [x] T4.6 The plan: the pinned optimization order, and column resolution up front.
- [x] T4.7 The memory budget: the estimate, and the refusal before any work.
- [x] T4.8 The plan output: per-stage rows and time, materialised columns, budget, flavor.
- [x] T4.9 Tests: filter semantics, NULL handling, density dispatch, late materialization,
      aggregate correctness, budget refusal.

## T5 — The harness (`columnar-bench`)

- [x] T5.1 The kernel A/B harness: every flavor, on the real dataset, with a warm-up pass, a
      median over the timed passes, one reused output buffer, and a self-check that every flavor
      agrees on the match count. (Revised: not `criterion` — a hand-rolled timer, because the
      harness has to assert cross-flavor agreement, and `criterion` has no way to express that.)
- [x] T5.2 The machine spec capture: CPU, SIMD path, toolchain, thread count.
- [x] T5.3 The external driver: DuckDB and Polars as subprocesses, same file, pinned threads,
      version recorded, load and query timed separately. (Revised — see T8.5.)
- [x] T5.4 The machine-readable JSON result format.
- [x] T5.5 The comparison report, with the "kernel X on query Y" framing rule enforced.

## T6 — The interface (`columnar-cli`)

- [x] T6.1 `columnar dataset` — generate a dataset.
- [x] T6.2 `columnar query` — run a plan and print the plan output.
- [x] T6.3 `columnar bench` — the kernel A/B.
- [x] T6.4 `columnar compare` — the external comparison.

## T7 — The evidence

- [x] T7.1 `PERF.md` — the measured numbers, with their conditions.
- [x] T7.2 `README.md` — the headline, the reproduction command, and the scope.
- [x] T7.3 The CI workflows: fmt, clippy, test, differential, and the two benchmark runners.
- [x] T7.4 The ADR set.
- [ ] T7.5 The x86-64 measurement. **Blocked**: there is no x86-64 runner yet. The AVX2 path is
      code-complete and CI-wired, but its numbers do not exist and must not be estimated.

## T8 — The follow-on work, driven by the first measurement

The first published kernel number was wrong: the "scalar" baseline had been auto-vectorised by
the compiler, so the ratio was comparing two vectorised implementations. Fixing that turned up a
second problem — the hand-written kernel was losing to the compiler — and fixing *that* turned up
a third: the comparison harness was measuring the wrong thing. See ADR-0003, ADR-0006 and
ADR-0007.

- [x] T8.1 Disassemble the compiled kernel and check that the "scalar" baseline really is scalar.
      It was not: 296 vector instructions inside the supposed baseline.
- [x] T8.2 Compile the scalar reference twice — once with `-fno-vectorize`, once with ordinary
      flags — and publish both ratios.
- [x] T8.3 Add the `CppAuto` flavor and the `col_compare_*_auto` ABI entry points.
- [x] T8.4 Rewrite the NEON mask accumulation: keep the mask in a 128-bit register and extract
      once per 64-row block, instead of extracting per pair of rows. The same rewrite in the Rust
      intrinsics path, so the two remain comparable implementations of one algorithm.
- [x] T8.5 Rebuild the external comparison: cold and resident modes, load and query timed
      separately, the answers cross-checked against the engine's own.
- [x] T8.6 Fix the report's dataset provenance: read the manifest instead of reporting zeros.
- [x] T8.7 Fix the Polars SQL generation: `Polars`' `SQLContext` has no file-path relation, so the
      table name is registered rather than quoted.
- [x] T8.8 Add the parser regression tests for the incumbent output formats.
- [x] T8.9 The global aggregate: a global query has exactly one group, so the key vector and the
      hash lookup per row were pure overhead. `scan` went from 48.9 ms to 0.0 ms.
- [x] T8.10 The dictionary map: a mispredicted branch per row while setting the mask bit. The Q6
      filter stage went from 41.2 ms to 6.6 ms.
- [x] T8.11 The global and grouped aggregate paths are pinned to agree by a test.
- [x] T8.12 Correct the published claim about *why* the engine is slow cold: it is the eager
      decode of all sixteen columns (39.3× of the 62× gap), not a slow reader (1.58×). Recorded
      in ADR-0008.
