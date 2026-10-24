# Spec 001 — Vectorized engine core: storage to aggregate

**Status**: implemented
**Created**: 2026-10-23
**Scope**: the first shippable increment of the columnar engine

## What this spec covers

The first milestone of `columnar-engine`: read a Parquet file into the engine's own column
chunks, filter it with a SIMD comparison kernel, materialise only the columns that are needed,
and aggregate the survivors — with a plan output that shows what each stage cost.

This is deliberately the **TPC-H Q6 path** and not "a query engine". Q6 is a single scan, a
filter on one numeric column, one projection and one aggregate. It is the shortest path that
exercises every mechanism the project claims to have, and it is the path on which the blueprint
expects the engine to be competitive.

## Documents

| File | What it holds |
|---|---|
| `spec.md` | the requirements, with acceptance criteria |
| `plan.md` | the architecture and the crate layout |
| `tasks.md` | the work breakdown, with status |

## Verdict

Every requirement in `spec.md` is implemented and machine-checked. The verification method for
this spec is the **differential test suite** in `columnar-kernels/tests/differential.rs`, which
asserts that all five implementations of the comparison predicate agree with the oracle bit for
bit, plus the per-operator unit tests in each crate.

See `README.md` and `PERF.md` for the measured numbers and their conditions.
