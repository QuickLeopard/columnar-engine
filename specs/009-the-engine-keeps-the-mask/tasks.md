# Spec 009 — Tasks

**Spec**: [009-the-engine-keeps-the-mask](spec.md)
**ADR**: [ADR-0015](../../docs/adr/0015-the-engine-keeps-the-mask.md)
**Status**: done

## T1 — The measurement, before anything was written

- [x] The scan was split the way the eager decode was split: Arrow's own decode, the
      engine's kernel, the output phase, and the rest. On the real file, in one
      process, warm: **11.9 ms + 6.0 ms + 1.4 ms, and ~60 ms of nothing.**
- [x] The sixty milliseconds were the mask on its way to the reader: parquet-rs turns a
      mask into a **row-range list** and applies it one range at a time.
- [x] The list was measured: **2,775,138 ranges for 2,178,861 surviving rows**, and
      reading `l_shipdate` through it costs **50.0 ms** against **7.8 ms** without it.
- [x] The alternative was then **prototyped and measured** before it was built: the
      engine evaluating the predicates itself, over the reader's arrays, took
      **25.9 ms** against the current **78.3 ms**, with the same 315,203 survivors.

## T2 — The engine keeps the mask

- [x] The reader is built with **no row filter**. The predicates are the engine's, and
      the engine evaluates them.
- [x] The reader projects the **materialised columns and the predicates' columns**:
      the predicates' columns are read so the predicates can be evaluated over them,
      and dropped afterwards.
- [x] The masks of the individual predicates are **`AND`ed** in the engine's own
      layout — one bit per row, LSB-first, in 64-bit words. They are aligned, because
      every column of a record batch holds the same rows.
- [x] The mask is applied with Arrow's `filter_record_batch`, which selects rows; the
      engine's semantics are in the mask, and Arrow's filter only *applies* it.
- [x] When the query materialises **no** column, the engine counts the surviving rows
      and filters nothing: the rows themselves are work nobody asked for.

## T3 — The semantics did not change

- [x] The predicates still run the **engine's own kernel**, over Arrow's raw value
      slice, with the engine's NULL rule and the engine's NaN rule.
- [x] The dictionary path is unchanged: compared once per distinct value, mapped back
      through the indices.
- [x] ADR-0010 is **amended**, not replaced: its decision — the engine's semantics, not
      Arrow's — stands, and the mechanism underneath it changed.

## T4 — The rules that did not change, and why their reason did

- [x] A predicate on a column the query materialises is still **declined**. The
      engine's filter applies the predicates the scan did not, over the rows that
      survived the scan; pushing it into the scan would evaluate it over every row of
      the file *and* leave the filter to evaluate it again over the survivors.
- [x] A predicate the pushdown cannot evaluate **exactly** is still declined.
- [x] A declined predicate's column is still read, because the filter stage needs it.

## T5 — The tests

- [x] The differential suite still holds: the lazy path and the eager path agree, row
      for row, over every operator, NULLs, NaNs, a NaN threshold, a dictionary column,
      a plain UTF-8 column, and the 64-row block boundaries.
- [x] **New**: the predicates' columns are read but **not** materialised — the dataset
      holds the materialised columns and nothing else.
- [x] **New**: the materialised columns keep **their own values** — the projection puts
      the predicates' columns between them, so a column read at the wrong position
      would hold another column's values.
- [x] The tests are **mutation-verified**: not `AND`ing the masks fails 3 of them,
      materialising the predicates' columns fails 13, taking the batch's position
      instead of the column's own fails 3, counting every row instead of the
      surviving ones fails 9, and never applying the mask fails 2.

## T6 — The measurement

- [x] The headline table was **re-measured**, because the committed reports carry the
      figures.
- [x] The cold sweep and the resident sweep were **re-measured**, at every thread
      count, with both incumbents measured beside the engine at the same thread
      count.
- [x] The **load** sweep was not re-measured: this increment does not touch the eager
      path, and the load is a measurement of the eager path.
- [x] The decomposition is **in the tree**, as
      `crates/columnar-engine/examples/lazy_scan_decomposition.rs`, so a reader can
      run it.

## T7 — The follow-ons, not done

- [ ] **The morsel driver** — overlapping the read with the operators — is not here.
- [ ] **The predicates are evaluated over more rows.** Q6's pushdown evaluates
      12,000,000 row-comparisons where it used to evaluate 8,178,861, because the
      second predicate's column is no longer narrowed by the first. The extra work is
      real, and it is counted.
- [ ] **The predicates' columns are read in full.** The old design read the second
      predicate's column only for the rows the first kept. The trade is worth it on
      this file, and the measurement is published either way.
- [ ] **Page-level pruning** is still absent, and the manifest still says so.
- [ ] **The x86-64 measurement** stays blocked: there is no x86-64 machine.
