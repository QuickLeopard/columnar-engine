# Plan 001 — Vectorized engine core

**Date**: 2026-10-23
**Spec**: `spec.md`

## Crate layout

```
crates/
  columnar-core/      vocabulary: types, encodings, null bitmaps, selection vectors
  columnar-kernels/   the comparison kernel: C++ (NEON/AVX2) + Rust intrinsics + oracle
  columnar-engine/    Parquet reader, operators, plan, memory budget, plan output
  columnar-testdata/  the deterministic TPC-H-shaped dataset generator
  columnar-bench/     the criterion harness and the DuckDB/Polars comparison driver
  columnar-cli/       the command line surface
```

Dependencies point one way only:

```
columnar-core  <-  columnar-kernels
      ^                  ^
      |                  |
      +---- columnar-engine ----+
      |                  |      |
      +-- columnar-testdata     |
                 ^              |
                 +-- columnar-bench
                              ^
                              |
                        columnar-cli
```

`columnar-core` has no dependencies at all, on purpose: it is the vocabulary, and a vocabulary
with a version history is a vocabulary that can drift.

## The kernel, in three layers

**Layer 1 — the oracle.** A plain Rust loop, one row at a time, the predicate spelled out. It is
the correctness reference and it is written to be obviously right rather than fast.

**Layer 2 — the C++ implementation.** One translation unit with three code paths:

- a branchless scalar reference, which is the A/B baseline;
- a NEON path for AArch64;
- an AVX2 path for x86-64, reached through `__attribute__((target("avx2")))` after a runtime
  probe. The translation unit is never compiled with a global `-mavx2` (ADR-0004).

The SIMD paths compute, per 64-row block, a 64-bit mask of matching rows. The validity mask is
ANDed in once per block rather than per lane, which keeps the inner loop free of a second memory
stream and turns null handling into one scalar operation per 64 rows. A partial final block is
computed with the scalar path, because the SIMD loop always covers 64 rows and would otherwise
read past the end of the array.

**Layer 3 — the Rust intrinsics path.** The same predicate through `std::arch`, with no FFI at
all. It exists so that the FFI seam's cost is a measurement rather than an argument
(review finding 2.12). If it and the C++ path measure the same, the seam is free; if it does not,
the C++ story has to go.

## The mask assembly, and why it is bit-packed

The kernel's output is a bit mask, LSB-first, one bit per row — the same layout as DuckDB, Arrow
and Parquet validity masks. Choosing that layout rather than a byte-per-row or a `Vec<bool>`
means the mask can be handed straight to the validity mask's format with no repack, and it means
the selection vector's bitmap representation is free.

## Selection vectors, and the density dispatch

A selection vector has two representations: an **index list** of surviving rows, and a **bitmap**
of one bit per source row. The engine picks between them at runtime on density, at a named
threshold (`BITMAP_DISPATCH_DENSITY`).

The index representation costs 4 bytes per surviving row; the bitmap costs 1 bit per source row.
They cross over at a density of 1/16 in raw bytes, but the index wins earlier than that suggests
— an index list is contiguous in memory and the gather that consumes it is a linear scan, whereas
a bitmap forces a bit test per source row. The threshold used is 30%, and it lives in one place
so that the constant, the code and the documentation cannot drift apart.

## The pipeline

```
scan  ->  filter  ->  project  ->  aggregate
```

- **scan** decodes the columns the plan actually needs, and nothing else.
- **filter** runs the comparison kernel and produces a selection vector.
- **project** gathers the surviving rows of the projected columns, using the selection vector's
  own representation — an index list is a linear gather, a bitmap is a bit test per row.
- **aggregate** hashes the group key, accumulating the aggregates.

The pinned optimization order is **predicate pushdown → projection pushdown → late materialization
→ aggregate**, and it is stated in the plan output, not only in a design document.

## The memory budget

Every plan is costed before it runs: the bytes of the columns that will actually be materialised,
plus the selection vectors, plus the aggregate hash table. If the estimate exceeds the budget,
the planner refuses the query **before** doing any work. A query that would exhaust the machine
is rejected, not attempted.

## The external comparison

`columnar-bench` drives DuckDB and Polars as CLI subprocesses against the same Parquet file the
engine reads, with the thread count pinned and the plan cache disabled, inside the same harness
that measures this engine. The point is that both incumbents read the *same bytes*: the
comparison is about the engine, not about the data.

The framing rule, enforced by the write-up: every headline reads **"kernel X on query Y"**, never
"this database versus DuckDB". The queries where DuckDB wins are published in the same table.

## What this plan deliberately does not do

No morsel scheduler, no hash join, no sort, no SQL parser. Each is a separate spec. The reason
is not that they are hard; it is that each one adds a variable, and the point of this increment
is to have exactly one variable to explain: the kernel.
