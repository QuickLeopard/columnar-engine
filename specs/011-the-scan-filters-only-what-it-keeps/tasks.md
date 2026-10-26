# Spec 011 — Tasks

**Spec**: [011-the-scan-filters-only-what-it-keeps](spec.md)
**ADR**: [ADR-0017](../../docs/adr/0017-the-scan-filters-only-what-it-keeps.md)
**Status**: done

## T1 — The measurement, before anything was written

- [x] `filter-range`'s stages were read off the plan output: **scan 17.6 ms**, project 0.0 ms, aggregate 1.4 ms. The
      scan was 88% of the query.
- [x] The scan was **decomposed**, with a probe in the tree: Arrow's own decode of the two columns **10.7 ms**, the
      predicates **0.7–1.4 ms**, and the filter and the copy **4.7 ms**.
- [x] **The README's recommendation was wrong.** It said the predicates were worth narrowing in the engine's own
      domain. They cost 0.7–1.4 ms of a 16 ms scan, and there is nothing there to recover. The correction is
      published, not quietly dropped.
- [x] The same work in **three shapes** was measured on the same file, in the same process: **A — filter the whole
      batch, then copy — 4.7 ms**, **B — filter only what is kept — 3.3 ms**, **C — compact once, into the chunk —
      2.5 ms**.
- [x] The batch size was swept, and then the two candidates were measured **against each other**, with the order
      alternating on every round so that a drifting machine cannot bias the result.

## T2 — The scan compacts the survivors once

- [x] `ColumnBuilder::push_masked` writes the survivors **straight out of the reader's array** into the engine's own
      column. One copy, of the values that are kept.
- [x] `decode_batches_filtered` **no longer calls** `arrow::compute::filter_record_batch`. It calls `push_masked` per
      materialised column, and never touches the pushed-down columns.
- [x] A predicate's column is read by the reader and **dropped**: it is never copied into the engine's column.
- [x] The compaction is the engine's own mask, walked a word at a time — `RowMask::for_each`, one bit per row, no
      range list anywhere.

## T3 — The output is unchanged

- [x] The scan's output still holds **the survivors**, in **the file's order**.
- [x] The plan output still reports the scan's rows out as the survivors.
- [x] The answer is **bit for bit** the same: `55559443.879999444`.

## T4 — The compaction gets the details right

- [x] The survivors are visited in **ascending** row order.
- [x] A NULL row's value is the engine's convention of **zero**.
- [x] A NULL row is recorded at the position it takes **among the survivors**. (The first implementation recorded the
      row it had in the file, which is not a position in the column — it was caught by the tests, and fixed.)
- [x] A dictionary column's keys are **repointed** at the column's own dictionary, over the row group's **whole**
      dictionary.

## T5 — The batch size is measured for the lazy shape

- [x] The lazy path's constant was measured in **spec 002**, for a two-column lazy read — but the scan's shape has
      changed **three times** since, so that measurement is of a shape that no longer exists.
- [x] The sweep: 1,024 → 24.8 ms, 8,192 → 18.6 ms, 65,536 → 17.4 ms, 131,072 → 17.3 ms (medians of three sweeps).
- [x] The **interleaved A/B**: 8,192 → 16.1–16.4 ms, 65,536 → 14.3–14.6 ms. A difference of **1.6–2.0 ms (10–12%)**,
      repeatable across three runs, with the order alternating so the drift hits both.
- [x] The constant is now **65,536**, and the measurement that chose it is published with it.

## T6 — The tests

- [x] The compacted column holds **the same rows as the eager path's gather** — the same values, in the same order,
      with the same validity — for a numeric column, for a dictionary column, and across row groups.
- [x] The parallel scan compacts **the same rows** as the single-threaded one, at 2, 3, 4 and 8 threads.
- [x] **New**: the NULL-value convention is tested with an array that has a **value in the NULL slot**. A file cannot
      show it — the Parquet reader puts a zero in a NULL row's slot — so the array is built by hand.
- [x] **New**: a NULL survivor is recorded at **its own position**, not at the row it had in the file.
- [x] **New**: the masked push and the unmasked push **agree**, row for row.
- [x] **New**: the dictionary's keys are **repointed**, over a dictionary the reader cannot produce — which is why the
      test builds the arrays by hand. (A file written by Arrow gives every row group a dictionary that accumulates,
      and the remap is the identity there.)
- [x] **New**: the mask's iteration visits the kept rows in **ascending** order, and does not visit the **padding**.
- [x] **The mutations were run**: a null survivor keeping the value the reader decoded, a null survivor recorded at
      the row it had in the file, the compaction keeping every row, the compaction keeping the rows in the wrong
      order, the mask's iteration visiting the padding, and a dictionary survivor's index not being remapped — six
      injected, six caught.

## T7 — The publication

- [x] The headline table was **re-measured**: three full campaigns, medians and spreads, the raw reports committed.
- [x] The **sweeps were not re-measured**: they measure `q6`, whose aggregate is `count(*)` over no column, and
      neither change reaches it.
- [x] ADR-0017, this spec, `PERF.md`, `README.md` and the constitution were updated.
- [x] The old figures are published next to the new ones.
