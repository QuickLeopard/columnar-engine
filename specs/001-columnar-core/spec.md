# Spec 001 — Vectorized engine core: storage to aggregate

**Status**: implemented
**Date**: 2026-10-23
**Constitution**: `constitution.md` v1.0.0

## Problem

A reviewer should be able to run one command on their own machine and see the engine's kernel
number, next to a number for DuckDB, measured by the same harness on the same file. That requires
five things: a storage format the engine owns, a comparison kernel that is actually fast, an
operator pipeline that only touches the columns it needs, a dataset both engines can read, and a
harness that measures all of it the same way.

This spec defines those five things.

## Requirements

### R1 — The engine owns its storage representation

**R1.1** The engine MUST represent a column as a `ColumnChunk`: a name, a typed encoding, a
validity mask and a row count.

**R1.2** The encodings MUST include a plain form for each physical type and a dictionary form
for each physical type. A columnar engine without dictionary-encoded strings is a numeric toy,
and the design review is explicit about this (finding 2.3).

**R1.3** Arrow MUST NOT appear inside the engine. It is an I/O-boundary type only.
(Principle: ADR-0005.)

**R1.4** The validity mask MUST be bit-packed, LSB-first, with padding bits zeroed so that two
masks describing the same rows are byte-for-byte equal.

**Acceptance**: `columnar-core`'s unit tests cover canonicalisation, null counting, and
dictionary decoding.

### R2 — The comparison kernel is the project

**R2.1** The engine MUST implement `(column OP threshold) → bit mask` for all six comparison
operators, over `Int64` and `Float64` columns.

**R2.2** There MUST be five implementations of that predicate: a Rust oracle, a C++ scalar
reference, a C++ SIMD path, a Rust intrinsics path, and the C++ SIMD path's internal scalar
fallback. All five MUST agree bit for bit, always. (Principle I.)

**R2.3** The C++ scalar reference MUST be branchless and MUST compute the same predicate as the
SIMD paths, so that the published ratio measures vectorisation rather than branch prediction.
(Principle II.)

**R2.4** The SIMD path MUST be selected at runtime, with a scalar fallback on a machine that has
no SIMD path. The kernel MUST run correctly on a machine with neither NEON nor AVX2.
(Principle: review finding 2.4.)

**R2.5** A row whose validity bit is clear MUST NOT match, for every operator, including `!=`.
(Principle III.)

**R2.6** A comparison involving a NaN MUST NOT match, for every operator, including `!=`.
(Principle III.)

**R2.7** The kernel MUST expose three flavors — `Scalar`, `CppSimd`, `RustSimd` — so that the
cost of the FFI seam can be measured rather than argued about. (Principle: review finding 2.12.)

**Acceptance**: `columnar-kernels/tests/differential.rs` — a property test over random inputs and
all six operators, asserting agreement with the oracle for every flavor, plus explicit tests for
block boundaries, NULL rows, NaN values, NaN thresholds, empty input, and bits set beyond the
end of the input.

### R3 — The pipeline touches only what it needs

**R3.1** The optimization order MUST be pinned and documented as: **predicate pushdown →
projection pushdown → late materialization → aggregate**. (Review finding 2.6.)

**R3.2** A column that is neither filtered, nor projected, nor aggregated MUST NOT be decoded.
This MUST be observable in the plan output. (Review finding 2.6, late materialization.)

**R3.3** The selection vector MUST have two representations — an index list and a bitmap — and
MUST dispatch between them on density. The threshold MUST be a named constant in one place.
(Review finding 2.2.)

**R3.4** `COUNT(*)` MUST have an in-place path that materialises no column. (Review finding 2.7.)

**R3.5** The engine MUST declare a per-query memory budget and MUST refuse a query whose
estimated working set exceeds it, before doing any work. (Review finding 2.8.)

**R3.6** The plan output MUST report, per stage: rows in, rows out and elapsed time; the list of
materialised columns; the estimated memory and the budget; and the kernel flavor in use.

**Acceptance**: `columnar-engine`'s unit tests cover the density dispatch, the late
materialization observation, the `COUNT(*)` fast path, and the memory-budget refusal.

### R4 — The dataset is reproducible and honestly named

**R4.1** The dataset MUST be a pure function of `(scale_factor, seed, null_rate)`. The PRNG MUST
be implemented in this repository; no crate in this workspace may depend on `rand`. (Constitution
Principle VI.) The rule is enforced by
`crates/columnar-core/tests/dependency_policy.rs`, and `deny.toml` records why it is a test and
not a `cargo deny` graph rule.

**R4.2** The dataset MUST be written as Parquet, so that DuckDB and Polars read the same bytes
this engine reads.

**R4.3** The seven low-cardinality string columns MUST be written dictionary-encoded, and the
Q6 path MUST filter a dictionary-encoded string column. (Review finding 2.3.)

**R4.4** The dataset MUST be described as **TPC-H-shaped**, never as "TPC-H SF-1". The manifest
MUST record the seed, the scale factor, the row count and the null rate.

**Acceptance**: `columnar-testdata`'s tests cover determinism, the schema, the dictionary
encoding, the null rate and the manifest round-trip.

### R5 — The comparison is reproducible by a stranger

**R5.1** The engine MUST build and run from one command, with no manual steps.

**R5.2** The external comparison MUST run DuckDB and Polars in the **same harness**, against the
**same Parquet file**, with the thread count pinned and the plan cache disabled. (Review
finding 2.1.)

**R5.3** Every published number MUST carry: the commit, the hardware, the SIMD path, the
optimization level, the warm/cold state and the thread count. (Principle IV.)

**R5.4** The benchmark MUST emit machine-readable JSON. Prose numbers are hand-copied and drift.
(Principle IV.)

**R5.5** The results MUST be published as "kernel X on query Y", and MUST publish the queries
where DuckDB wins. (Review finding 2.13.)

**Acceptance**: `columnar-bench`'s harness, the JSON artifacts, and `PERF.md`.

## Out of scope for this spec

Stated explicitly, with the reason (Principle VII):

| Excluded | Reason |
|---|---|
| A SQL parser | A month of work that produces no evidence. The query surface is a typed Rust struct. (Review finding 2.1's "SQL parser is a time trap".) |
| Morsel-driven parallelism | Single-threaded execution for this increment, so that the external comparison can pin both engines to one thread. Morsel scheduling is a separate spec. |
| Hash join and sort | The Q6 path needs neither. They are in the "full engine" increment. |
| A decimal type | TPC-H decimals are materialised as `Float64` and the manifest says so. Claiming a richer type system than is implemented would be a claim, not evidence. |
| A date type | Dates are `Utf8` in `YYYY-MM-DD` form. |
| GPU acceleration | The project is CPU-only by design. |
| Nested types | Not present in the TPC-H schema. |

## References

| Requirement | Principle or review finding |
|---|---|
| R1.4 | Principle III (NULL is an absent bit) |
| R2.2 | Principle I |
| R2.3 | Principle II |
| R2.4 | Review finding 2.4 |
| R2.5, R2.6 | Principle III |
| R2.7 | Review finding 2.12 |
| R3.1, R3.2 | Review finding 2.6 |
| R3.3 | Review finding 2.2 |
| R3.4 | Review finding 2.7 |
| R3.5 | Review finding 2.8 |
| R4.1, R4.4 | Principle VI |
| R4.3 | Review finding 2.3 |
| R5.2 | Review finding 2.1 |
| R5.5 | Review finding 2.13 |
