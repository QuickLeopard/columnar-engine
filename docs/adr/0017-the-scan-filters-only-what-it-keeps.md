# ADR-0017: The scan filters only what it keeps

**Status**: accepted
**Date**: 2026-10-26
**Related**: [ADR-0015](0015-the-engine-keeps-the-mask.md), [ADR-0016](0016-the-aggregate-reads-in-place.md), [spec 011](../../specs/011-the-scan-filters-only-what-it-keeps/spec.md)

## Context

`filter-range` — `SELECT sum(l_quantity) FROM lineitem WHERE l_discount >= 0.04 AND l_discount <= 0.07` — was the
**only** query the engine still lost on the cold path. On the published median at the time it took 19.9 ms against
Polars' 15.7 ms. Every other query in the table was won, and the **resident** table was won outright.

This increment has **two** parts, and both are about the scan's shape: **what it copies**, and **how big the batches
it copies in are**. Neither was a guess: both were measured before either was built, and the measurements are in the
tree.

The plan output said what was left. The lazy path's stages were:

| stage | |
|---|---|
| **scan** | **17.6 ms** |
| project | 0.0 ms |
| aggregate | 1.4 ms |

**The scan was 88% of the query.** And it was a scan, not an operator: the filter had already run inside the reader,
and the aggregate that used to cost 7.8 ms of materialisation now costs 1.4 ms of folding. What was left was the read
itself, and the shape of the read.

The repository's own README proposed where to go next: *"narrowing the predicates in the engine's own domain, the
morsel driver, and the x86-64 measurement"*. The first of those was a guess, and the guess was **wrong**. The
measurement below says the predicates cost 0.7–1.4 ms of a 16 ms scan, and that there is nothing there to recover.

## The measurement

The scan was decomposed **before** anything was built, the way every other shape of work in this repository has been.
A probe — now `crates/columnar-engine/examples/filter_range_scan_decomposition.rs`, in the tree — took the real file,
the real predicates and the real batches, and measured the scan's parts:

| | |
|---|---|
| **Arrow's own decode** of `l_quantity` and `l_discount`, with no filter and no conversion | **10.7 ms** |
| **the predicates**, over the whole column | **0.7–1.4 ms** |
| the filter and the copy, in the shape the engine used | **4.7 ms** |

Three things fall out of that, and they decide the work:

- **The predicates are not the gap.** 0.7–1.4 ms of a 16 ms scan is 4–9%. "Narrowing the predicates in the engine's own
  domain" would have recovered nothing, and the README's recommendation was wrong.
- **Arrow's decode is 10.7 ms and it is the floor.** That is the cost of the bytes, and no arrangement of the
  engine's own code can go below it. The engine's own work on top of the decode is 5.4 ms, and that is what was
  available to win.
- **The filter and the copy cost 4.7 ms, and most of it is work nobody asked for.**

That last point is the defect. The lazy scan **filtered the whole batch** with Arrow's `filter_record_batch`, and
then copied the survivors out of the filtered batch into the engine's own column. So:

1. the filter copied **every column in the batch** — including `l_discount`, the column the predicates read and the
   query does not materialise, and which was about to be thrown away;
2. and then the survivors were copied **a second time**, out of the filtered batch, into the engine's column.

Three shapes of doing the same work were measured, on the same file, in the same process:

| | |
|---|---|
| **A — filter the whole batch, then copy** (the shape in the tree before this increment) | **4.7 ms** |
| **B — filter only what is kept** | **3.3 ms** |
| **C — compact once, into the chunk** | **2.5 ms** |

**Shape C is half of shape A.** The engine's own bit loop walks the mask and writes the survivors straight out of the
reader's array into the engine's column: one copy, of the values that are kept, in the order they appear in the file.

## Decision

**The scan writes the survivors into its output once, and copies nothing it does not keep.**

The scan's mask is the engine's own — one bit per row, no range list anywhere ([ADR-0015](0015-the-engine-keeps-the-mask.md)).
The compaction is that mask, walked a word at a time: for each word, the set bits are the survivors, and each
survivor's value is copied straight out of the reader's array into the engine's column.

The scan's output is **unchanged**. It still holds the survivors, in the file's order, and the plan output still
reports them as the scan's rows out. What changed is how many times they were copied on the way there.

### What the compaction has to get right

- **The values are the same values.** The survivors' values are read out of the reader's array, and a NULL row's
  value is the engine's convention of zero, exactly as the unmasked push sets it.
- **The order is the same order.** The survivors are visited in **ascending** row order, which is the order they
  appear in the file. A `SUM` over `float64` is order-dependent, so this is what makes the answer the same sum and
  not merely the same number.
- **The NULLs are at the same positions.** A NULL row is recorded at the position it takes **among the survivors**,
  not at the row it had in the file: the column holds the survivors only, so the file's row numbers are not positions
  in it.
- **The dictionary is merged the same way.** A row group's dictionary is merged into the column's dictionary and its
  keys are repointed at the column's slots, over the row group's **whole** dictionary — because Arrow's own filter
  keeps a dictionary column's dictionary unchanged and filters only its keys, so the dictionary the column ends up
  with is the same either way.

### The batch size was measured, and it moved

The lazy path's reader batch size **was** 8,192 rows, and it **was** measured — in spec 002, for a two-column lazy read,
where it took the read of Q6's two columns from 52.6 ms to 43.4 ms. But the scan's shape has changed **three times**
since that measurement: the pushdown became the engine's own kernel (spec 003), the mask stopped going to the reader as
a list of 2,775,138 row ranges (spec 009), and the scan stopped filtering the whole batch and copying the survivors
twice (this increment). A tuning constant that depends on the shape of the work must be measured for that shape
(constitution), and a constant measured for a shape that no longer exists is not a measured constant for the shape that
does — so the constant was swept again, with the engine's own lazy read end to end and nothing else changed:
`crates/columnar-engine/examples/lazy_batch_size.rs`, using the `read_with_batch_rows` seam that exists for exactly
this purpose.

| batch rows | the scan |
|---|---|
| 1,024 | 24.8 ms |
| 4,096 | 20.4 ms |
| **8,192** (the constant before this increment) | **18.6 ms** |
| **65,536** (the constant now) | **17.4 ms** |
| 131,072 | 17.3 ms |

A sweep runs its sizes **in order**, and a machine that gets busier as the sweep goes on shows up as a trend in the
curve — and the trend is indistinguishable from a real effect. So the two candidates were measured **against each
other**, interleaved, with the order alternating on every round so that the drift hits both:

| | the scan |
|---|---|
| 8,192 | 16.2 ms |
| **65,536** | **14.5 ms** |

Three runs of that A/B: 8,192 → 16.18, 16.42 and 16.08 ms; 65,536 → 14.55, 14.47 and 14.34 ms. **A difference of
1.6–2.0 ms, 10–12%**, repeatable, and blind to the order. **The constant becomes 65,536**, and the measurement that
chose it is published with it.

### A rejected alternative: do not compact at all

The obvious next step is to compact **nothing**: keep the whole column and let the aggregate fold over the mask
instead. It would save the 2.5 ms of compaction as well. It was rejected, for three reasons that do not need a
benchmark:

- the dataset would hold 6,000,000 rows instead of 2,178,861, so the working set would grow by 2.75×;
- the scan's published behaviour — *the scan reports the surviving rows* — would be undone, and the memory estimate
  with it;
- and the aggregate would have to test the mask per row, which the spec 010 probe measured at **17.68 ms** against
  1.83 ms for the fold that follows the selection.

The first two are properties of the engine's published contract. The third is the measurement that says the same
thing from the other side.

## The result

The two builds were measured **back to back**, on this machine, in the same session, because the machine is shared
and two campaigns taken on different days cannot be compared. The CLI's own median wall clock over five runs:

| | before | after |
|---|---|---|
| **lazy (cold)** | 19.9 ms | **17.3 ms** |
| resident (the control) | 3.3 ms | 3.3 ms |

The resident row is the **control**: the resident path does not go through the lazy scan, so it did not move, and the
change is the engine's.

The published table, which is the median of three full campaigns and measures every query in one process:

| | before | after |
|---|---|---|
| `filter-numeric` cold, lazy | 5.6 ms | **5.2 ms** |
| `filter-string` cold, lazy | 18.1 ms | **13.6 ms** |
| `filter-range` cold, lazy | 19.9 ms | **17.5 ms** |
| `q6` cold, lazy | 26.0 ms | **20.7 ms** |
| `q6-revenue` cold, lazy | 70.7 ms | **63.1 ms** |

The stages tell the story, and the scan is the only thing that moved:

| stage | before | after |
|---|---|---|
| **scan** | **18.5 ms** | **15.1 ms** |
| project | 0.0 ms | 0.0 ms |
| aggregate | 2.0 ms | 2.0 ms |

And the answer is **bit for bit the same**: `55559443.879999444`.

**The gap to Polars is now 1.1 ms, and it was 4.2 ms.** The scan's floor — Arrow's own decode of the two columns —
is 10.7 ms, and Polars does not pay it: it reads the file with its own reader, in its own format, and answers a
`SUM` over a filtered column in 16.4 ms in total. The engine's own work on top of that decode is **5.4 ms**, and
it is not one thing: the compaction is 2.5 ms of it, the predicates 1.4 ms, and the scan's own overhead the rest.

That is the honest state of this query, and it is published as what it is: **the engine is 1.1 ms behind Polars on
the cold `filter-range`, and the scan's own decode is 10.7 ms of the 16.1.**

## What this does not do

- **It does not narrow the predicates.** The measurement says the predicates are 0.7–1.4 ms, and the README's earlier
  recommendation to narrow them was wrong. That correction is published in the README, not quietly dropped.
- **It does not change the scan's output.** The dataset still holds the survivors, in the file's order, and the plan
  output still reports them.
- **It does not change the eager path.** The eager path reads every column and materialises it, and the compaction is
  a property of a scan that applies a mask.
- **It does not change the resident path.** The resident path never goes through the lazy scan, and its numbers are
  published as they were measured, with the machine's drift in them.
- **It does not make the aggregate read the mask.** The aggregate folds a column of the survivors, which is what
  spec 010 made it do.

## The consequence for the plan output

None. The plan output reports the columns the query **read** and the rows the scan **kept**, and both are unchanged.
The scan's elapsed time is smaller, and that is the whole of the difference.
