# ADR-0015: The engine keeps the mask

**Status**: accepted
**Date**: 2026-10-25
**Related**: [ADR-0010](0010-pushdown-equivalence.md) (the pushdown's semantics, amended here), [ADR-0014](0014-parallel-eager-decode.md), [spec 009](../../specs/009-the-engine-keeps-the-mask/spec.md)

## Context

The engine's lazy scan pushes the plan's predicates into the Parquet reader, as
ADR-0010 describes: the reader is given a `RowFilter`, the reader evaluates the
predicates as it decodes, and the reader hands back only the rows that survived.

That worked, and it was correct, and it was slow in a way nobody had measured.
The engine's lazy scan of the two columns TPC-H Q6 needs took **82.4 ms** on the
published median, against DuckDB's **32.0 ms** on the same file at the same thread
count — and the plan output said the scan stage was 84.3 ms with the pushdown 6.0
ms of it, and said nothing about the other 78.

So it was measured, the way spec 008 measured the eager decode.

## The measurement

A probe — now `crates/columnar-engine/examples/lazy_scan_decomposition.rs`, in the
tree — split the scan on the real file, in one process, warm:

| | |
|---|---|
| Arrow's own decode of the two columns, no filter, no conversion | **11.9 ms** |
| the engine's kernel — the comparison itself | **6.0 ms** |
| the output phase — the surviving rows, handed over | **1.4 ms** |
| **the rest** | **~60 ms** |

Sixty milliseconds of nothing, and the "nothing" had a shape. The pushdown's mask
is one bit per row, and for `l_discount` — a random float — it is fine-grained:
`0.04 <= l_discount <= 0.07` keeps a scattered 36% of the rows.

A **row filter is not a mask**. parquet-rs turns the mask into a **list of row
ranges** and applies the list **one range at a time**, through one `read_records`
call per range:

| | |
|---|---|
| rows the mask keeps | 2,178,861 |
| **ranges in the list** | **2,775,138** |
| `RowSelection::from_filters` — building the list | 5.7 ms |
| reading `l_shipdate` **through** the list | **50.0 ms** |
| reading `l_shipdate` **without** it | **7.8 ms** |

The list is longer than the number of rows it selects, and applying it costs six
and a half times the decode of the column it selects.

## Decision

**The engine evaluates the pushed-down predicates itself, over the reader's arrays, and keeps the mask. The reader is given no row filter.**

The scan:

1. projects the columns the query materialises **and** the columns the predicates
   read — the predicates' columns are read so the engine can evaluate the
   predicates over them, and dropped afterwards, because the query did not ask for
   them;
2. evaluates each predicate over its column, with the engine's own kernel, and
   `AND`s the masks together — they are aligned, because every column of a record
   batch holds the same rows;
3. applies the mask to the batch — one bit per row, in the engine's own layout,
   with no range list anywhere.

**The predicates' semantics do not change.** They are the same function they have
always been: the engine's own comparison kernel, with the engine's NULL rule and
the engine's NaN rule, over Arrow's raw value slice. What changes is **who calls
it** — the engine, rather than the reader — and therefore what the reader does
with the answer.

## The result

The scan, on the real file, at one thread:

| | before | after |
|---|---|---|
| Q6's two columns | **78.4 ms** | **26.6 ms** |
| the real Q6's four columns | **140.2 ms** | **68.4 ms** |
| `filter-range`'s two columns | **87.3 ms** | **17.6 ms** |

The published query figures are in [`PERF.md` §2a](../../PERF.md) and §2e, and they
are the medians of three committed reports.

## The tests, and what they have teeth

The differential suite in `crates/columnar-engine/tests/lazy_scan.rs` answers every
operator, against NULLs, NaNs, a NaN threshold, a dictionary column and a plain
UTF-8 column, over row counts of 0, 1, 2, 63, 64, 65, 127, 128, 129, 191, 192 and
193 — the boundaries of the 64-row block the kernel works in — and requires the
lazy path and the eager path to agree, row for row, every time. It is unchanged,
and it still passes.

Two tests are new, and they cover what this change could have broken:

- **The predicates' columns are read but not materialised.** The dataset holds the
  materialised columns and nothing else.
- **The materialised columns keep their own values.** The projection holds the
  materialised columns *and* the predicates' columns, in the file's order, so a
  materialised column's position in the projected batch is not its position in the
  file — and a column read at the wrong position would hold another column's
  values, which a row count would not catch.

The tests are mutation-verified. Not `AND`ing the masks fails 3 of them;
materialising the predicates' columns fails 13; taking the batch's position instead
of the column's own fails 3; counting every row instead of the surviving ones fails
9; never applying the mask fails 2.

## Consequences

**Good.**

- The lazy scan of Q6's two columns is **2.9× faster**, and every published lazy
  query got faster with it.
- The engine's mask is now the *only* representation of a row selection in the
  engine. There is no range list, and no second representation that could drift
  from the first.
- ADR-0010's argument gets **stronger**, not weaker: the pushdown and the filter
  are not two implementations kept in agreement by a test, they are one function
  called from two places.

**Bad, and stated.**

- **The predicates are evaluated over more rows.** Q6's pushdown now evaluates
  12,000,000 row-comparisons where it used to evaluate 8,178,861, because the
  second predicate's column is no longer narrowed by the first predicate's
  selection. The extra work is real and the engine counts it: the pushdown's
  metrics report the rows evaluated and the time per row.
- **The predicates' columns are read in full.** The old design read the second
  predicate's column only for the rows the first predicate kept. The trade is worth
  it on this file — 78.4 ms against 26.6 ms — but it is a trade, and a file whose
  predicate columns are enormous and whose selectivity is very high could go the
  other way.
- **The reader no longer skips the rejected rows.** The old design's reader
  returned only the surviving rows; the new one returns every row of the projected
  columns and the engine drops the rest. The pages are still fetched either way —
  the engine has no page index — but the rejected rows are now *decoded* where
  before some of them were skipped. On this file that is a net win, because the
  range list cost more than the decoding did.

**Neutral.**

- The rule that declines a predicate on a column the query materialises does not
  change; its **reason** does. It used to be "the reader would decode that column
  twice". The reader decodes it once now. The reason is that the filter stage
  applies the predicates the scan did not, over the rows that survived the scan,
  and pushing it into the scan would evaluate it over every row of the file *and*
  leave the filter to evaluate it again over the survivors.

## Alternatives considered

**Keep the row filter, and make the list cheaper.** The list's length is a
function of the data: a random float column has a scattered mask, and a scattered
mask has a range per surviving run. No re-encoding of the list makes a
2.78-million-entry list cheap to walk. Rejected.

**Push the predicate down at a coarser granularity** — a block-level or
page-level predicate, with the engine applying the exact predicate afterwards.
The engine does not know the file's page boundaries, and a coarse selection at the
reader's batch granularity skips nothing: the reader would decode the whole batch
anyway. Rejected, and it would have made the reader's answer approximate.

**Read the predicate columns twice**: once for the mask, once narrowed by the
first predicate. That is what the old design did, and the narrowing is what
required the range list. Rejected: the narrowing is the thing that costs.

**Give the reader a bitmap selection.** parquet-rs has no such API; the selection
is a range list by construction. Rejected.

## Revisit when

- **The engine gains a page index.** Then the reader could skip pages for the
  predicates' columns, and the trade above would have to be measured again.
- **The predicates' columns get expensive.** If a file's predicate columns are
  much larger than their selectivity suggests, reading them in full may cost more
  than the range list did. The engine would need to measure that and choose, and
  the choice would have to be published.
- **The morsel driver arrives.** Overlapping the read with the operators is the
  next architectural step, and it is still worth less than it looks: the pushdown
  leaves the cold path's operators with nothing to do.
