# Spec 010 — Fuse the projection into the aggregate

**Status**: accepted
**Date**: 2026-10-26
**Related**: [ADR-0016](../../docs/adr/0016-the-aggregate-reads-in-place.md), [ADR-0013](../../docs/adr/0013-parallel-operators.md), [spec 006](../006-arithmetic-aggregate/spec.md), [spec 007](../007-parallel-operators/spec.md)

## 1. The problem, measured

`filter-range` — `SELECT sum(l_quantity) FROM lineitem WHERE l_discount >= 0.04 AND l_discount <= 0.07` — was the
**only** query the engine lost. On the published median it took 14.6 ms resident against Polars' 11.5 ms, and
26.6 ms cold against Polars' 14.9 ms. Every other query in the table was won.

The plan output named the cause. The query's stages were:

| stage | |
|---|---|
| filter | 1.5 ms |
| **project** | **7.0 ms** |
| aggregate | 2.4 ms |

**The projection was more than half the query, and it was materialising a column nobody would ever see.** The
aggregate sums it and throws it away. A plan *with* an aggregation ignores the projection — the aggregate result
is the output — so the gathered column existed for exactly one reason: the aggregate operator had no way to read
the source.

So the fusion was **prototyped and measured before it was built**. A probe
(`crates/columnar-engine/examples/fused_aggregate_probe.rs`) took the real file, the real filter and the real
selection, and measured four ways of getting the same answer:

| | |
|---|---|
| gather the survivors, then fold the gathered column | **9.06 ms** |
| **fold the source at the selection's rows** | **1.83 ms** |
| fold the source, testing the selection for every row | 17.68 ms |
| fold the source a word at a time | 1.99 ms |
| the whole column, with no selection at all | 3.73 ms |

All four folds produced **bit-identical** sums. The spread between them decided the shape of the work:

- the fold must **follow** the selection — walking the source and asking the selection about every row costs
  nearly ten times as much;
- the fold must **not copy** — the gathered column is 17.4 MB, and copying it out and reading it back is most of
  the 9.06 ms;
- the fold is **cheaper than reading the whole column**, because it reads 2.18 million values rather than 6 million.

## 2. What this increment does

**A global aggregate reads the column where it lies, over the filter's selection. The projection materialises
nothing for it.**

A global aggregate — one with no `GROUP BY` — has exactly one group, and every surviving row belongs to it. It
folds the survivors one after another and never needs their values **aligned** with anything. So the engine does
not build a column of the survivors: it hands the aggregate the **source column** and the **selection**.

### R1. The values are the same values

The source column's encoding is read directly. A dictionary-encoded column is read through its dictionary, exactly
as the gather read it. A run-length-encoded column holds runs rather than values, so it cannot be read a row at a
time, and it is gathered as it always was.

### R2. The order is the same order

The survivors are visited in **ascending row order**, which is the order the gathered column's fold visited them
in. A `SUM` over `float64` is order-dependent — floating-point addition is not associative — so this is what makes
the answer **bit for bit** the same sum and not merely the same number.

### R3. The NULL rules are the same rules

A NULL row is skipped, exactly as the gather's mask skipped it. A row beyond the validity mask's length is a NULL
row, exactly as it was in the gather: a row the engine cannot vouch for is a NULL row.

### R4. The saturation and the NaN rule are the same

An integer `SUM` saturates at the `i64` bounds rather than wrapping. A float `MIN`/`MAX` compares with `total_cmp`,
so a NaN is ordered rather than refused.

### R5. A column is folded in place only when all three hold

1. The aggregation is **global** — one group.
2. The column is read by a **plain** aggregate — a sum over the column, a count, a min or a max — **and by nothing
   else**. A column an *expression* reads is still gathered, because the expression is evaluated over the survivors
   and needs its operands aligned with them.
3. The column's **encoding** can be read one row at a time.

### R6. The grouped path is unchanged

A grouped aggregate needs its keys aligned with its values, and the hash table wants both. There is no measurement
behind fusing that, so it is not done — and because the grouped path is unchanged, the differential test that
compares the global answer against the one-group answer still pins the two together.

### R7. There is one global implementation

`aggregate_global` takes the value sources and folds them; the grouped path's `aggregate` converts its gathered
columns into the same sources and calls the same fold. A second implementation would be a second answer to
disagree with this one.

### R8. The fold is specialised before the loop, not inside it

The first implementation dispatched on the column's kind and on the accumulator's kind inside the loop, and cost
6.2 ms — three times the prototype's. Resolving both before the loop took the same fold to **2.0 ms**.

### R9. The plan output reports the columns that were read

A column the aggregate folded in place is named in the plan output, because the query read it. The projection
stage reports the time the gathers took, which is now zero for a query whose aggregate reads in place.

## 3. The result

`filter-range`, on the real file, at one thread:

| | before | after |
|---|---|---|
| resident | 14.6 ms | **6.9 ms** |
| cold (lazy) | 26.6 ms | **19.9 ms** |

The stages, as the medians of three runs of each build, measured back to back on this machine:

| stage | before | after |
|---|---|---|
| filter | 1.5 ms | 1.4 ms |
| **project** | **7.0 ms** | **0.0 ms** |
| aggregate | 2.4 ms | **1.9 ms** |

**`filter-range` is no longer the query the engine loses.** See [PERF.md](../../PERF.md) §2a.

## 4. What this does not do

- The aggregate stays **serial**. A `SUM` over `float64` is not associative, so the aggregate is never
  parallelised — constitution 1.5.0, unchanged.
- The **filter is not fused** into the aggregate. The filter's selection is the fold's input, and the filter still
  runs as its own stage.
- The **grouped path is not fused**. No measurement, no change.
- An **expression's operands are not fused**. The expression is evaluated over the survivors, and its operands
  have to be aligned with them.

## 5. How this was verified

- **The answers do not change.** The fused sum is compared against the gathered sum — the same sum computed
  through an expression, which gathers — bit for bit, and against an oracle that walks the rows independently.
- **The data is order-sensitive on purpose.** The float column repeats `0.1, 0.2, 0.3`, which do not add up to
  the same number in every order, and the test asserts that the ascending and the descending sums differ — so a
  fold that visited the survivors in the wrong order cannot pass.
- **The NULL rows hold a value no correct sum would hold**, so a fold that counted one would be caught.
- **The mutations were run.** A fold in the wrong order, a fold that did not skip the NULL rows, a count that
  counted every survivor, a dictionary read in the wrong order, a column an expression reads being fused, a
  grouped aggregate being fused, an integer sum that wrapped instead of saturating, and a minimum and a maximum
  swapped: each was injected, and each was caught.
