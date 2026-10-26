# ADR-0016: The global aggregate reads the source in place

**Status**: accepted
**Date**: 2026-10-26
**Related**: [ADR-0013](0013-parallel-operators.md) (the projection is a separate stage), [ADR-0015](0015-the-engine-keeps-the-mask.md), [spec 010](../../specs/010-fuse-the-projection-into-the-aggregate/spec.md)

## Context

`filter-range` — `SELECT sum(l_quantity) FROM lineitem WHERE l_discount >= 0.04 AND l_discount <= 0.07` — was the
**only** query the engine lost. On the published median it took 14.6 ms resident against Polars' 11.5 ms and 26.6 ms
cold against Polars' 14.9 ms. Every other query in the table was won.

It was not a filter problem, and the plan output said so. The query's stages were:

| stage | |
|---|---|
| filter | 1.5 ms |
| **project** | **7.0 ms** |
| aggregate | 2.4 ms |

**The projection was more than half the query.** And it was materialising a column that nobody would ever see:
the aggregate sums it and throws it away. `PERF.md` §2a named the cause — "the projection and the aggregate are two
separate stages rather than one fused operator, and the gap is the materialisation" — and the engine's own plan
said the same thing in a different way: a plan *with* an aggregation ignores the projection, because the aggregate
result is the output.

So the gathered column existed for exactly one reason: the aggregate operator had no way to read the source.

## The measurement

The fusion was **prototyped and measured before it was built**, the way every other shape of work in this
repository has been. A probe — now `crates/columnar-engine/examples/fused_aggregate_probe.rs`, in the tree — took
the real file, the real filter and the real selection, and measured four ways of getting the same answer:

| | |
|---|---|
| gather the survivors, then fold the gathered column | **9.06 ms** |
| **fold the source at the selection's rows** | **1.83 ms** |
| fold the source, testing the selection for every row | 17.68 ms |
| fold the source a word at a time | 1.99 ms |
| the whole column, with no selection at all | 3.73 ms |

All four folds produced **bit-identical** sums. The spread between them is the decision:

- **The fold must follow the selection.** The selection is a bitmap over 6,000,000 rows that keeps 2,178,861 of
  them — 36.3%. Walking the source and asking the selection about every row costs 17.68 ms, nearly ten times the
  fold that follows the selection, because the bitmap's bits have to be tested one row at a time and the branch
  mispredicts.
- **The fold must not copy.** The gathered column is 17.4 MB of survivors. Copying them out and reading them
  back is most of the 9.06 ms.
- **The fold is cheaper than reading the whole column.** 1.83 ms against 3.73 ms for the whole column: the fold
  reads 2.18 million values, not 6 million.

## Decision

**A global aggregate reads the column where it lies, over the filter's selection. The projection materialises
nothing for it.**

A global aggregate — one with no `GROUP BY` — has exactly one group, and every surviving row belongs to it. It
folds the survivors one after another, and it never needs their values **aligned** with anything: not with each
other, not with a group key, not with an expression's evaluation. So the engine does not build a column of the
survivors. It hands the aggregate the **source column** and the **selection**, and the aggregate reads the
survivors' values out of the source where they are.

The change is where the values are read from. Everything about them is the same:

- **The same values.** The source column's encoding is read directly — a dictionary-encoded column is read
  through its dictionary, exactly as the gather read it.
- **The same order.** The survivors are visited in ascending row order, which is the order the gathered column's
  fold visited them in. A `SUM` over `float64` is order-dependent, so this is what makes the answer **bit for bit**
  the same sum, not merely the same number.
- **The same NULL rules.** A NULL row is skipped, and a row beyond the validity mask's length is a NULL row —
  exactly as the gather treated it.
- **The same saturation.** An integer `SUM` saturates at the `i64` bounds rather than wrapping.
- **The same NaN rule.** A float `MIN`/`MAX` compares with `total_cmp`, so a NaN is ordered rather than refused.

### What is fused, and what is not

A column is folded in place when **all three** of these hold:

1. The aggregation is **global** — one group.
2. The column is read by a **plain** aggregate — a sum over the column, a count, a min or a max — **and by
   nothing else**. A column an *expression* reads still has to be gathered, because the expression is evaluated
   over the survivors and needs its operands aligned with them.
3. The column's **encoding** can be read one row at a time. A run-length-encoded column holds runs, not values,
   so reading one of its rows means finding the run the row falls in; such a column is gathered, as it always was.

A **grouped** aggregate still gathers. Its group keys have to line up with its values, and the hash table wants
both. There is no measurement behind fusing that, so it is not done — and the grouped path is unchanged, which
means the global path's answer is still pinned to the grouped path's by the differential test that compares them.

### One global implementation

There is one global fold, not two. `aggregate_global` takes the value sources and folds them; the grouped path's
`aggregate` converts its gathered columns into the same sources and calls the same fold. A second implementation
would be a second answer to disagree with this one.

### The fold is specialised, not dispatched

The first implementation dispatched on the column's kind and on the accumulator's kind **inside** the loop, and
cost 6.2 ms — three times the prototype's. Resolving both **before** the loop, so that the loop is a tight one
over a resolved column, took the same fold to **2.0 ms**. The accumulator's state stays in a register for the
same reason.

That is a 4 ms difference on one query, and it is why the fold is written as a table of specialised loops rather
than as one loop with a match in it.

## The result

`filter-range`, on the real file, at one thread:

| | before | after |
|---|---|---|
| resident | 14.6 ms | **6.9 ms** |
| cold (lazy) | 26.6 ms | **19.9 ms** |

The stages tell the story. The two builds were measured **back to back, in the same session**, and
these are the medians of three runs of each:

| stage | before | after |
|---|---|---|
| filter | 1.5 ms | 1.4 ms |
| **project** | **7.0 ms** | **0.0 ms** |
| aggregate | 2.4 ms | **1.9 ms** |

And the CLI's own median wall clock over five runs, for the same two builds: **10.9 ms → 3.3 ms**
resident, **26.4 ms → 19.9 ms** lazy. The lazy figure is the number the comparison harness publishes
for the cold path; the resident figure is lower than the harness's 6.9 ms because the harness
measures all six queries in one process, after a 231 MiB decode has churned the allocator. Both are
published, and the harness's is the one the table uses, because the harness measures the incumbents
the same way.

**The projection now materialises nothing.** The plan output still names `l_quantity` as a column the query read,
because it was read — it is simply read where it lies, rather than copied into a column of the survivors.

`filter-range` is no longer the query the engine loses. See [PERF.md §2a](../../PERF.md).

## What this does not do

- **It does not parallelise the aggregate.** A `SUM` over `float64` is not associative, so the aggregate stays
  serial, exactly as constitution 1.5.0 says it always would.
- **It does not fuse the filter into the aggregate.** The filter's selection is the input to the fold, and the
  filter still runs as its own stage. A filter that ran *inside* the fold would save the selection's own cost —
  and the measurement says that is small, because the pushed-down predicates are already applied by the scan.
- **It does not fuse the grouped path.** No measurement, no change.
- **It does not fuse an expression's operands.** The expression is evaluated over the survivors, and its operands
  have to be aligned with them.

## The consequence for the plan output

The plan output reports the columns that were **read**, not the columns that were copied. A column the aggregate
folded in place is named, because the query read it. The projection stage reports the time the gathers took, which
is now zero for a query whose aggregate reads in place.

That is the honest report: the projection did nothing, and it says so.
