# Spec 002 — Lazy, column-selective scan with predicate pushdown

**Status**: accepted
**Date**: 2026-10-24
**Supersedes**: nothing. Extends spec 001 (the core and the operators).
**Related**: ADR-0008 (the eager decode this spec removes), ADR-0005 (the engine owns its chunks)

---

## 1. The problem, measured

Spec 001 built the engine and measured it against DuckDB 2.0.0 and Polars 1.36.1. The engine
wins the *query* and loses the *file*, by a lot:

| | engine | duckdb | polars |
|---|---|---|---|
| `q6`, resident | **10.3 ms** | 46.0 ms | 69.7 ms |
| `q6`, cold | 1972 ms | **31 ms** | 194 ms |

The 63.6× cold gap decomposes into two independent terms, and only one of them is about speed:

| | engine | duckdb | ratio |
|---|---|---|---|
| Reader speed, same job: decode all 16 columns | 1921 ms | 1241 ms | 1.55× |
| Architecture: materialise everything, or stream and push the predicate down | materialises everything | streams, reads 2 columns | 39.3× |

**1.55 × 39.3 ≈ 61.** So about 2.4% of the gap is how fast the reader is. The other 97.6% is a
design decision: `Dataset::read_parquet` decodes the *entire file* — all sixteen columns, all
six million rows — into the engine's own column chunks before any query starts.

The file does not need all of that. Per the Parquet footer, Q6 needs `l_discount` and `l_shipdate`:
11.5 MiB compressed out of 231.5 MiB, and 11.7 MiB raw out of 404.5 MiB. **2.9% of the raw bytes.**
`l_comment` alone is 203 MiB raw — half of everything the engine decodes — and Q6 never reads it.

ADR-0008 recorded the eager decode as a deliberate choice for the first increment, with a clause
to revisit it once the cost was measured. This spec is that revisit.

## 2. What this spec changes

The engine gains a **second way to read a file**, and the benchmark measures both:

- **Eager** (spec 001, unchanged): `Dataset::read_parquet(path)` decodes everything;
  `execute(plan, &dataset)` answers the query over it.
- **Lazy** (this spec): `ParquetSource::open(path)` reads the file's *footer* and returns a
  handle. `execute_on(plan, &source)` then reads only the columns the plan needs, with the
  plan's predicates pushed into the reader.

The eager path is not removed. It is the reference implementation: the differential test in
§6 holds the two to the same answer, and the eager path is what a caller who wants every column
still uses.

## 3. Requirements

### R1 — A file can be opened without being decoded

`ParquetSource::open` reads the file's metadata and returns a handle. No row is decoded, and no
column's data is fetched.

### R2 — The plan resolves against the schema before any data is read

A plan that names a column the file does not have, or that compares an `int64` column against a
`float64` threshold, is an error **before** any column data is read. The resolution works against
the file's schema, not against decoded chunks, so it costs a footer read.

### R3 — Only the columns the plan needs are read

The reader is handed a projection over the file's root columns. The bytes of the columns outside
it are never fetched from the file — not read, not decompressed, not decoded.

A column is "needed" when the plan filters on it, groups by it, aggregates it, or projects it.
For the Q6 plan that is `l_discount` and `l_shipdate`: two of sixteen.

### R4 — A predicate may be pushed into the reader, and only when it is exactly equivalent

This is the requirement that decides whether the whole thing is safe.

The engine's filter has two rules that are **stricter than SQL**, and both are load-bearing:

- A row whose validity bit is clear is NULL and **never matches**, for every operator,
  **including `!=`**.
- A comparison involving a NaN value or a NaN threshold **never matches**, for every operator,
  **including `!=`**.

Arrow's own comparison semantics differ on both counts. Arrow's kernels return `null` for a
comparison against a NULL, and `RowFilter` keeps a row whose predicate is `null`; and in IEEE-754
`NaN != 5.0` is **true**, where the engine says it never matches.

So the engine does not use Arrow's comparisons. It implements its own `ArrowPredicate` whose
`evaluate` returns exactly the engine's answer, and the equivalence is argued in
[ADR-0010](../../docs/adr/0010-pushdown-equivalence.md) and checked by R6.

### R5 — A predicate that cannot be pushed down exactly is declined, not approximated

Two cases:

- **The column's type is not one the pushdown implements.** The predicate stays with the
  engine's filter. It is never approximated.
- **The column is already being read** for the aggregation or the projection. Pushing it down
  would make the reader decode it twice — once to evaluate the predicate and once to produce the
  output — which is slower than not pushing it. The predicate stays with the engine's filter.

A declined predicate's column is added to the read set, and the engine's filter stage runs over
it.

### R6 — The lazy path and the eager path must agree, row for row

For every query and every dataset — including datasets with NULLs, with NaNs, with
dictionary-encoded columns, and with every row count around the 64-row block boundary — the two
paths must produce the same groups and the same values. This is a differential test, and it is
the guarantee the whole spec rests on.

### R7 — The plan output says what was pushed down and what was read

A claim about what the reader did has to be checkable rather than asserted. The plan output
gains two lines: the predicates that were pushed into the scan, and the columns that were read.

### R8 — The benchmark measures both paths, the same way

The comparison harness measures the lazy path in the same resident and cold modes as the eager
one, with the same cross-check of the answer against the incumbents'. The two paths are published
side by side.

## 4. Design

### 4.1 The schema abstraction

Plan resolution currently happens against a `Dataset`. A lazy query has no `Dataset` yet — it has
a *schema*. So resolution moves behind a trait:

```rust
pub trait ColumnSchema {
    fn column_count(&self) -> usize;
    fn column_name(&self, index: usize) -> &str;
    fn column_type(&self, index: usize) -> DataType;
}
```

`Dataset` implements it (from its chunks) and so does the file's schema. The eager path is
unchanged in behaviour; it just resolves through the same code.

### 4.2 The read set

Given a resolved plan and the file's schema:

```
read set := the plan's materialised columns            (group-by, aggregate, projection)
for each predicate:
    if its column is in the read set          -> declined (R5: it would be decoded twice)
    else if the pushdown implements it        -> pushed
    else                                      -> declined, and its column joins the read set
```

A predicate that is declined has its column added to the read set, because the engine's filter
stage will need it.

### 4.3 The filter stage after a pushdown

A pushed predicate has already been applied by the reader, so the engine's filter stage must not
apply it again — its column is not even in the dataset. The filter stage therefore runs with the
**declined** predicates only.

That is correct because the conjunction is separable: `(A ∧ B)` applied by the reader, then `B`
applied by the engine, is `A ∧ B`. And when every predicate is pushed, the filter stage does not
run at all, and the plan output says so.

### 4.4 A dataset with no columns

A `count(*)` over a fully pushed-down filter needs no column at all: the reader returns
zero-column batches that still carry the surviving row count. The `Dataset` therefore carries its
row count explicitly rather than deriving it from the first chunk.

## 5. The shape of the code

| module | what it holds |
|---|---|
| `columnar-engine/src/source.rs` | `ParquetSource`: open a file, read a column subset with a pushdown |
| `columnar-engine/src/pushdown.rs` | the `ArrowPredicate` that reproduces the engine's filter semantics exactly |
| `columnar-engine/src/plan.rs` | `ColumnSchema`, resolution against a schema, the read set, the remap |
| `columnar-engine/src/lib.rs` | `execute_on`: the lazy execution path |
| `columnar-engine/src/explain.rs` | the plan output's new lines |

## 6. The differential test

`crates/columnar-engine/tests/lazy_scan.rs` holds the two paths to the same answer:

1. **Every builtin query**, over the generated TPC-H-shaped dataset, eager and lazy: the same
   groups, the same values.
2. **Every operator**, over datasets built to hit the edges: NULLs at every position, NaN values,
   NaN thresholds, empty inputs, and row counts of 0, 1, 63, 64, 65, 127, 128, 129.
3. **Every comparison operator**, against the oracle in `columnar-kernels`, so the pushdown is
   checked against the same reference the kernels are checked against.
4. **Dictionary-encoded string columns**, where the pushdown must map a per-distinct-value
   comparison back through the indices and agree with the engine's own dictionary path.

## 7. Out of scope

Stated here rather than deferred silently:

- **Row-group pruning from statistics.** The reader does not skip row groups whose statistics
  exclude every row. It reads the row groups it needs and filters them.
- **Page-level skipping.** `RowFilter` can refine a `RowSelection` at page granularity; this spec
  does not use it.
- **Parallel decode.** The reader is single-threaded, like the rest of the engine in this
  increment.
- **Spilling.** Unchanged from spec 001: the memory budget is a pre-flight check, not a
  mechanism.
- **A SQL parser.** Unchanged.

## 8. What this spec is expected to move

Not a prediction — a target to be measured and published either way:

- The cold `q6` number, which is 1972 ms today, should fall by roughly the ratio of the bytes
  read: the file's Q6 columns are 2.9% of its raw bytes.
- The cold gap against DuckDB should fall from 63.6× to somewhere in the low tens.
- The resident numbers should not move much, because the resident path does not read the file.

If the cold numbers do not fall, that is a finding, and it gets published as one.

## 9. What actually moved

The spec's targets were set against the spec-001 baseline above. Measured, as the median of
three full runs (`PERF.md` §2, and the raw reports in `bench-results/`):

| target | predicted | measured |
|---|---|---|
| The cold `q6` number falls by roughly the ratio of the bytes read | 1972 ms, falling | **1181 ms → 121 ms**, a 9.7× fall |
| The cold gap against DuckDB falls from 63.6× into the low tens | low tens | **36.9× → 3.8×** — better than the target |
| The resident numbers should not move much | unchanged | unchanged: 9.6 ms against DuckDB's 44.0 ms |

Two things the spec did not anticipate, and both are findings rather than surprises:

- **The gain was bigger than the byte ratio.** Q6's columns are 2.9% of the file's raw bytes, and
  the cold `q6` query fell by 90%, not 97%. The engine also stops *materialising* the 5.7 million
  rows that fail the predicate, which the byte ratio does not count.
- **The reader became the bound, and the remaining gap is page pruning.** The lazy path is now
  limited by the Parquet reader fetching pages DuckDB would have skipped, not by the engine's
  operators. That gap is measured and attributed in `PERF.md`, and it is the next increment's work.

