# Spec 011 — The scan filters only what it keeps

**Status**: done
**Started**: 2026-10-26
**ADR**: [ADR-0017](../../docs/adr/0017-the-scan-filters-only-what-it-keeps.md)
**Predecessor**: [010-fuse-the-projection-into-the-aggregate](../010-fuse-the-projection-into-the-aggregate/spec.md)

## The problem

`filter-range` was the last query the engine lost on the cold path: 19.9 ms against Polars' 15.7 ms on the
published median. Every other query in the table was won, and the resident table was won outright.

The plan output said what was left, and it said it was the **scan**:

| stage | |
|---|---|
| **scan** | **17.6 ms** |
| project | 0.0 ms |
| aggregate | 1.4 ms |

The scan was 88% of the query. It was not an operator gap: the filter had already run inside the reader, and the
aggregate that used to cost 7.8 ms of materialisation now cost 1.4 ms of folding.

So this spec is about **the scan's shape**, and there are two shapes to fix in it: what the scan copies, and how big
the batches it copies in are.

## The measurement, first

Both of the decisions below came from measurements taken **before** the code was written, and both probes are in the
tree so the numbers can be reproduced.

### What the scan spends its time on

`crates/columnar-engine/examples/filter_range_scan_decomposition.rs` takes the real file, the real predicates and the
real batches, and measures the scan's parts:

- Arrow's own decode of the two columns, with no filter and no conversion: **10.7 ms**
- the predicates, over the whole column: **0.7–1.4 ms**
- the filter and the copy, in the shape the engine used: **4.7 ms**

And the same work in three shapes, on the same file, in the same process:

| | |
|---|---|
| **A — filter the whole batch, then copy** (the shape in the tree) | **4.7 ms** |
| **B — filter only what is kept** | **3.3 ms** |
| **C — compact once, into the chunk** | **2.5 ms** |

### How big the batches should be

`crates/columnar-engine/examples/lazy_batch_size.rs` sweeps the lazy path's batch size and then measures the two
candidates **against each other**, with the order alternating on every round. The A/B is there because a sweep runs
its sizes in order, and a machine that drifts through the run shows up as a trend that is indistinguishable from a
real effect.

## The requirements

### R1 — The scan compacts the survivors once

- [x] The lazy scan writes the survivors **straight out of the reader's array** into the engine's own column: one
      copy, of the values that are kept.
- [x] It does **not** filter the batch and copy the survivors out of the filtered batch.
- [x] It does **not** copy a column the query does not materialise. A predicate's column is read by the reader and
      then dropped; it is never copied into the engine's column.
- [x] A column the query does materialise is copied **once**, and only its surviving rows.

### R2 — The output is unchanged

- [x] The scan's output still holds **the survivors**, in **the file's order**.
- [x] The plan output still reports the scan's rows out as the survivors.
- [x] The answer is **bit for bit** the answer the previous code gave. A `SUM` over `float64` is order-dependent, so
      "the same number" is not enough.

### R3 — The compaction gets the details right

- [x] The survivors are visited in **ascending** row order.
- [x] A NULL row's value is the engine's convention of **zero**, exactly as the unmasked push sets it.
- [x] A NULL row is recorded at the position it takes **among the survivors** — the column holds the survivors only,
      so the file's row numbers are not positions in it.
- [x] A dictionary column's keys are **repointed** at the column's own dictionary, over the row group's **whole**
      dictionary, because Arrow's own filter keeps a dictionary column's dictionary unchanged and filters only its
      keys.

### R4 — The batch size is measured for the lazy shape

- [x] The lazy path's batch size is measured **for the lazy path's shape**. It was measured in spec 002, but the
      scan's shape has changed three times since, so that measurement is of a shape that no longer exists.
- [x] The measurement is an **interleaved A/B**, not only a sweep.
- [x] The constant is changed **only** if the measurement says so, and the new value is published with the
      measurement that chose it.

### R5 — The tests

- [x] The compacted column holds **the same rows as the eager path's gather** — the same values, in the same order,
      with the same validity.
- [x] The NULL-value convention is tested with an array that has a **value in the NULL slot**, because a file cannot
      show it: the Parquet reader puts a zero in a NULL row's slot, so a push that left the value alone would look
      exactly the same.
- [x] A NULL survivor is recorded at **its own position**, not at the row it had in the file.
- [x] The dictionary's keys are repointed, over a dictionary the reader cannot produce — which is why the test builds
      the arrays by hand.
- [x] The mask's iteration visits the kept rows in ascending order, and does not visit the padding.
- [x] **The mutations were run**, and each one was caught.

### R6 — The publication

- [x] The headline table is **re-measured**: three full campaigns, medians and spreads, the raw reports committed.
- [x] ADR-0017, this spec, `PERF.md`, `README.md` and the constitution are updated.
- [x] The old figures are published next to the new ones, and the README's earlier **wrong** recommendation — that
      the predicates were worth narrowing — is corrected in the open.

## The non-goals

- **The predicates are not narrowed.** The measurement says they cost 0.7–1.4 ms of a 16 ms scan. There is nothing
  there to recover, and the README's earlier recommendation was wrong.
- **The scan does not stop compacting.** Holding all 6,000,000 rows and folding over the mask would save the
  compaction as well, but it would undo the published "the scan reports the surviving rows" behaviour and multiply
  the working set by 2.75×.
- **The aggregate does not read the mask.** It folds a column of the survivors, which is what spec 010 made it do.
- **The eager path does not change.** The eager path reads every column and materialises it; a compaction is a
  property of a scan that applies a mask.
