# ADR-0008: The Parquet reader decodes the whole file, eagerly — and that costs 39×

**Status**: accepted
**Date**: 2026-10-24
**Reviewers**: TBD

## Context

The engine reads a Parquet file into a `Dataset` and then answers queries against it. There are
two decisions here, and they are easy to confuse:

1. **What the internal representation is.** That is ADR-0005: the engine's own column chunks, not
   Arrow arrays.
2. **How much of the file gets decoded, and when.** That is this ADR.

For the first increment, the reader decodes **everything**: every column, every row group, every
row, into the engine's own chunk format, before any query runs.

```rust
// columnar-engine/src/dataset.rs
pub fn read_parquet(path: &Path) -> Result<Self, DatasetError> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)?;
    let schema = builder.schema().clone();
    let mut builders = builders_for(&schema)?;   // one builder per column
    let reader = builder.build()?;
    // ... decode every record batch, every column ...
}
```

The reason is straightforward: the first increment's queries are known, the dataset is one file,
and decoding everything makes the engine's behaviour easy to reason about. A query then costs
exactly the time its stages report, with no I/O folded in.

## Decision

**Keep the eager decode for this increment, and measure its cost rather than assume it.**

The engine's plan output already distinguishes *materialised columns* (the columns a query
gathered) from the columns the file was decoded into. The decision is about the second.

## Consequences

**Easier**: every query runs against memory that is already there. The query's measured time is
the query's time, with no I/O hidden inside it. The engine's resident measurements are
therefore directly comparable with an incumbent's resident measurements, which is the
like-for-like comparison in `PERF.md`.

**Harder**: the engine pays for the whole file on every run, whether the query needs one column
or sixteen. For the one-shot case -- one query against a file, which is exactly what the `DuckDB`
and `Polars` cold measurements do -- this is the engine's worst showing.

## Measurement

Apple M4 Max, one thread. The Q6 predicate set against the 231.6 MiB, 6,000,000-row file.

| | engine | duckdb |
|---|---|---|
| **Cold**: read the file and answer in one go | 1931 ms | **31 ms** |
| **Resident**: the data is already in memory | **10.1 ms** | 45 ms |

The engine is **62× slower cold and 4.4× faster resident.** Every one of those 62 times is the
file, and none of it is the kernel, the filter or the aggregate.

The 62× decomposes into two independent parts, because they have different fixes:

| | engine | duckdb | ratio |
|---|---|---|---|
| **Reader speed**, same job: decode all 16 columns, materialise them | 1920 ms | 1224 ms | 1.58× |
| **Architecture**: materialise everything first, vs stream and push the predicate down | materialises everything | reads 2 columns, discards 4.3M rows during the scan | 39.3× |

1.58 × 39.3 ≈ 62.

> These are the figures this decision rested on. A later measurement on the same machine gave
> 1921 ms / 1241 ms / 1972 ms against DuckDB's 31 ms — a 1.55× reader-speed ratio, the same 39.3×
> architectural factor, and a 63.6× cold gap. The ratio and the conclusion do not change; the
> current figures are in [`PERF.md`](../../PERF.md).

So **97.5% of the gap is the eager decode and only 2.5% is how fast `arrow-rs` is.** Reading
every column is the cost; the reader's speed is almost beside the point. `DuckDB`'s plan for the
same query shows the difference:
```
╭─ Parquet Scan ──────────────╮
│ Function: PARQUET_SCAN      │
│ Filters: l_discount >= 0.04 │
│ ~1,200,000 rows             │
╰─────────────────────────────╯
```

The filter is pushed into the Parquet reader, so the 4.3 million non-matching rows never
materialise at all.

The engine's own query time is not the problem: the Q6 filter stage is 6.6 ms for four
predicates over 6,000,000 rows, which is the SIMD kernel running at 8.8 G rows/s.

## Confidence

**high** on the measurement, **medium** on the decision. The cost is quantified and it is large,
but the eager decode is the right trade for the workload this increment targets -- many queries
over one file, where the decode is paid once and amortised. It is the wrong trade for a one-shot
query, and that is a real limitation rather than a rounding error.

## Revisit when

- The engine grows a second query against the same file, which is exactly when the eager decode
  starts paying for itself.
- **The next increment should supersede this ADR**: decode lazily, and only the columns a
  resolved plan asks for, with the predicate pushed into the Parquet reader. That is the 39.3×
  item, and it is worth about 38× on a one-shot query. It does not change ADR-0005 -- the engine
  still owns its own column chunks -- it changes *when* they are built.
- A faster decode path. That is the 1.58× item, and it is worth much less than the 39.3× one.

## Superseded

**Spec 002 did exactly this**, and this ADR's revisit clause has been taken up. The engine now
has two ways to read a file:

- the **eager** path this ADR describes, which is unchanged and still the default for a caller
  that wants every column; and
- the **lazy** path, which resolves the plan against the file's schema, reads only the columns
  the plan needs, and pushes the plan's predicates into the Parquet reader.

The measurement this ADR recorded is the measurement the lazy path was built to remove. On the
TPC-H `lineitem` file, Q6's cold query went from **1181 ms to 89 ms**, and the gap against
DuckDB's cold 32 ms from **39.6× to 2.8×**. (Medians of three full runs; the derivation is in
`PERF.md` and the raw reports are in `bench-results/`.)

The eager path got faster too, as a side effect: the two paths now share one decode function, and
that function stopped copying a dictionary column's distinct values out of every batch. The eager
path was paying for it 5,860 times — about 14.6 million string allocations — and putting the copy
back and re-measuring puts it at **1890 ms against 1181 ms**, a 1.60× difference, measured rather
than argued. The lazy path does not move, because it never materialised the column.

The eager path is kept because the trade this ADR recorded is real: the eager decode is the
right one for many queries over one file, where the decode is paid once and amortised. The lazy
path is the right one for a one-shot query. Both are measured, side by side, in `PERF.md`.

See [spec 002](../../specs/002-lazy-columnar-scan/spec.md) and
[ADR-0010](0010-pushdown-equivalence.md).
