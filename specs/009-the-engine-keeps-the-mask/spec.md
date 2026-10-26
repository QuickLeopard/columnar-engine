# Spec 009 — The engine keeps the mask

**Status**: accepted
**Date**: 2026-10-25
**Related**: [ADR-0015](../../docs/adr/0015-the-engine-keeps-the-mask.md), [ADR-0010](../../docs/adr/0010-pushdown-equivalence.md), [spec 002](002-lazy-columnar-scan/spec.md), [spec 003](003-kernel-pushdown/spec.md)

## 1. The problem, measured

The engine's lazy scan of the two columns TPC-H Q6 needs took **82.4 ms** on the
published median, against DuckDB's **32.0 ms** on the same file at the same thread
count. That 2.6× was the largest published gap in the query path, and it had never
been split: the plan output said the scan stage was 84.3 ms and that the pushdown
was 6.0 ms of it, and said nothing about the other 78.

So it was split, the way spec 008 split the eager decode. A probe
(`crates/columnar-engine/examples/lazy_scan_decomposition.rs`) measured the scan's
parts on the real file, in one process, warm:

| | |
|---|---|
| Arrow's own decode of the two columns, no filter, no conversion | **11.9 ms** |
| the engine's kernel — the comparison itself | **6.0 ms** |
| the output phase — the rows that survived, handed over | **1.4 ms** |
| **the rest** | **~60 ms** |

Sixty milliseconds of nothing. The rest of the scan was **the mask, on its way to
the reader**.

## 2. What the mask became

The pushdown's mask is one bit per row, and it is fine-grained: `l_discount` is a
random float, so `0.04 <= l_discount <= 0.07` keeps a scattered 36% of the rows.
The engine's mask for that predicate covers 6,000,000 rows and keeps 2,178,861 of
them.

The pushdown handed that mask to the Parquet reader as a **row filter**, and a row
filter is not a mask. parquet-rs turns the mask into a **list of row ranges** and
applies the list **one range at a time**, through one `read_records` call per
range:

| | |
|---|---|
| rows the mask keeps | 2,178,861 |
| **ranges in the list** | **2,775,138** |
| the list is longer than the number of rows it selects | |
| `RowSelection::from_filters` — building the list | 5.7 ms |
| reading `l_shipdate` **through** the list | **50.0 ms** |
| reading `l_shipdate` **without** it | **7.8 ms** |

Six and a half times the cost of decoding the column, to select a third of it.

The engine was paying that price on every scan with a pushed-down predicate over
scattered data — which is every scan over a random column, which is most of them.

## 3. What this increment does

**The engine evaluates the pushed-down predicates itself, over the reader's
arrays, and keeps the mask.**

The reader is given **no** row filter. The scan:

1. projects the columns the query materialises **and** the columns the predicates
   read — the predicates' columns are read so the engine can evaluate the
   predicates over them, and dropped afterwards, because the query did not ask
   for them;
2. evaluates each predicate over its column, with the engine's own kernel, and
   `AND`s the masks together — they are aligned, because every column of a record
   batch holds the same rows;
3. applies the mask to the batch — one bit per row, in the engine's own layout,
   with no range list anywhere.

The predicates' semantics do not change. They are the same function they have
always been: the engine's own comparison kernel, with the engine's NULL rule and
the engine's NaN rule, over Arrow's raw value slice. What changes is **who calls
it** — the engine, rather than the reader — and therefore what the reader does
with the answer.

## 4. Requirements

**R1 — The engine applies the predicates, and the reader applies none.**
The Parquet reader is built with no row filter. The predicates are evaluated by
the engine over the reader's arrays.

**R2 — The mask is one bit per row, and it stays that way.**
The mask is never turned into a row-range list, and no row-range list is ever
built from it. The engine's mask layout — LSB-first, one bit per row, in 64-bit
words — is the layout Arrow's `BooleanBuffer` uses, so the mask reaches Arrow
without a conversion.

**R3 — The predicates' semantics are unchanged.**
The same kernel, the same NULL rule, the same NaN rule, the same dictionary path
(compared once per distinct value, mapped back through the indices). The
differential suite in `crates/columnar-engine/tests/lazy_scan.rs` must still hold,
answer for answer.

**R4 — The predicates' columns are read, and not materialised.**
The scan reads the predicates' columns so the predicates can be evaluated over
them. The dataset holds the materialised columns and nothing else: a dataset that
held the predicates' columns would hold every row of the file for a column nobody
asked for, and the memory estimate would be wrong by exactly that much.

**R5 — The materialised columns keep their own values.**
The projection holds the materialised columns *and* the predicates' columns, in
the file's order, so a materialised column's position in the projected batch is
not its position in the file. The scan reads each column at its own position.

**R6 — A predicate on a column the query materialises is still declined.**
The engine's filter stage applies the predicates the scan did not, over the rows
that survived the scan. Pushing such a predicate into the scan would evaluate it
over every row of the file *and* leave the filter to evaluate it again over the
survivors. The rule does not change; its reason does.

**R7 — The order of the rows is the file's.**
The mask is applied to a batch in place, so the surviving rows keep their order.
The parallel path splits the file's row groups and joins the parts in block
order, exactly as it did before.

**R8 — The measurement is reproducible.**
The decomposition lives in
`crates/columnar-engine/examples/lazy_scan_decomposition.rs`, in the tree, and
prints the numbers this spec is based on.

## 5. Out of scope, stated rather than deferred

- **Page-level pruning.** The reader still fetches the pages of the columns it
  reads. The engine has no page index to prune with, and the manifest says so.
- **The morsel driver.** Overlapping the read with the operators is still the
  next architectural step, and it is still worth less than it looks: the
  pushdown leaves the cold path's operators with nothing to do.
- **The x86-64 measurement.** Blocked until there is an x86-64 machine.
- **A cheaper mask.** The engine's mask is already one bit per row, and the
  kernel already writes it a word at a time. There is nothing to make cheaper
  here; the cost was never the mask.

## 6. What would make this wrong

- **The predicates' columns may be expensive to read.** The old design read the
  *second* predicate's column only for the rows the first predicate kept. The new
  design reads every column of the projection in full. The measurement says the
  trade is worth it — 78.4 ms against 26.6 ms on Q6 — but the trade exists, and a
  file whose predicate columns are enormous and whose selectivity is very high
  could go the other way. The measurement is published either way.
- **The predicates are evaluated over more rows.** Q6's pushdown now evaluates
  12,000,000 row-comparisons where it used to evaluate 8,178,861, because the
  second predicate's column is no longer narrowed by the first. The extra work
  is real and it is counted: the pushdown's own metrics report it.
- **Arrow's `filter_record_batch` is not the engine's kernel.** It is Arrow's
  selection, and it is used only to *apply* a mask the engine computed. The
  engine's semantics are in the mask, not in Arrow's filter.
