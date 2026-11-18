# ADR-0067: The compaction reads a file at a time, and the optionality is decided

**Status**: accepted
**Date**: 2026-11-18
**Related**: [ADR-0066](0066-the-databases-own-price.md), [spec 056](../../specs/056-the-streaming-compaction/spec.md)

## Context

**The compaction had a ceiling, and the measurement found it.** The probe bisected for it and published it in
[PERF.md §3ak](../../PERF.md#3ak-the-databases-own-price): the compaction read the table's live rows in one piece, the
engine's estimate of that one plan was the table's rows, and the plan was refused above **6,000,000** rows —
**5,375,000** compacted and **6,000,000** refused, with the reason *"the engine refused: the estimated working set exceeds
the query memory budget"*. **And a table past that size could never be compacted**, and
[the compaction is the only thing that pays the tombstone tax back](../../crates/columnar-db/src/versions.rs) — so such a
table's reads went on paying the tax, at **0.115 ns** a retired version a row, for as long as the table lived.

**And the ceiling was not the only one.** The compaction materialised the table's live rows in the database's own memory
and built every Arrow batch at once, so **two ceilings shared one shape: the compaction read the table in one piece.**

**And the output file's schema was derived from the data**, which a streaming write cannot use: the writer's schema is
fixed before the first row, and the data's shape is known only after the last one. The store wrote a column required when
the rows held no NULL and optional when they held one, **so the file's optionality was the rows' happenstance**, and two
files of one table could disagree about the same column.

**And the engine does not care.** The engine's file-set check compares the column count, the names, the types and the
storage, and not the nullability — **and it does not have to**: the engine's vocabulary has no nullability in it, and a
file set whose files disagree about a column's optionality **reads correctly**
([`crates/columnar-engine/tests/nullability.rs`](../../crates/columnar-engine/tests/nullability.rs)). **So the disagreement
was a fact about the bytes and not about the answer** — and it was the database's business and not the engine's.

## Decision

**A1 — The compaction reads a file at a time, and it writes one file.**

One plan a file, so the engine's estimate is a fraction of the table's and the budget's verdict is about a file's rows; the
rows are written as they arrive, so the rows in flight are a file's rows; and the dead set is read a file at a time too,
because it was a plan over the whole table as well. **And the output is one file**, because the manifest is what makes the
retired versions stop existing, and a streaming read that wrote several files would be a compaction that does not compact.
**And the batches are on the store's grid**, because the file's bytes are a function of the rows and the grid and not of
the files the rows came from.

**A2 — The optionality is decided, and the decision is the database's row model's.**

The store's row-writing API takes the optionality as an explicit input, and the database says what it is: **the
application's columns are optional in every file, and the hidden columns are required.** **And the tombstone is the reason
for both**: a tombstone holds no value of the application's, so the application's columns carry an absent value in every
file that holds one — and a column that may hold an absent value cannot be written required. **So the file's optionality
cannot be the table's nullability**, and the schema's promise cannot reach the file; it stays where it belongs, at the
boundary and in the catalog. **And the rows' own optionality is still available**, and is named: it is
[`rows::nullability`](../../crates/columnar-store/src/rows.rs), and it is a choice the caller takes rather than a rule the
store applies.

**A3 — The disagreement is repaired, not refused.**

The compaction's output is decided, so it agrees with the database's rule whatever the files said, and **a file written
before the rule is fixed the next time the table is compacted**. **And the database does not learn to read a file's
nullability**, because it does not need to: the engine's vocabulary has no nullability, and the check's silence is the
engine saying that a column's optionality is not its business.

## Consequences

- **The ceiling is gone.** **6,000,000** rows compacted and **0** refused, where **5,375,000** compacted and **6,000,000**
  were refused. The full tables are in [PERF.md §3al](../../PERF.md#3al-the-compaction-reads-a-file-at-a-time).
- **And the compaction costs what it cost.** The clean compaction is **327** ns a row where it was **293**, and the two
  sets' ranges overlap; the taxed one is **14,328** ns a row where it was **13,088**, and the difference is the machine's —
  the campaigns were taken at a load average of **14.52**, **17.17** and **17.27** against **10.63**, **11.73** and
  **15.47**, and the write's throughput fell by eight per cent. **And the prediction holds: the tax's total work did not
  change and only its peak did.**
- **And the read path pays a little for the bounded dead read**, and the two sets' ranges overlap there too: the
  database's overhead on the read is **8.997 ms** where it was **6.777 ms**. **It is not distinguishable from the machine's
  movement**, and it is published so that a reader who thinks otherwise can see the spread.
- **And the bytes move by almost nothing.** The optional column's definition levels are a few bytes a file — the changed
  table grew by **2,091** bytes and **the space amplification did not move at all**, at **1.339**.
- **And the store's surface grew before the publish.** `rows::schema` and `rows::batches` take the optionality, and
  `Store::append_rows` does too, and the rows' own optionality is a named function. **The store publishes at 0.1.0**, and
  a surface published and then changed is a break in every embedder's build — so this was settled before the publish and
  not after it.
- **And the sentence that said otherwise is corrected where it was published**: the store's own module and the database's
  schema said the file's optionality was *"what the rows in it happened to be"*, and it is the database's row model now.
- **And the figures that moved are corrected where they were published.** The tombstone tax's own price moved from
  **0.115 ns** to **0.119 ns** a retired version a row, and from **0.120 ns** to **0.132 ns** in the small table, and the
  read's times with it — **13,942.178 ms** where it was **13,515.470 ms**, and **44.401 ms** where it was **44.637 ms**.
  **And the difference is the machine's**, the campaigns being taken at a load average of **14.52**, **17.17** and **17.27**
  against **10.63**, **11.73** and **15.47**. **And the crate's README and the status file quoted the old figure as the
  current one, and they are corrected where they were published** — the old figure beside the new one, with the reason
  named.
- **And the tombstone tax's shape is still open.** Streaming does not change what a read pays — **it only makes the
  compaction able to run at all**. The three ways out are three architectures, and the measurement does not pick between
  them.
- **And the index is still not built.** Its shape is decided and its write cost is still unmeasured, because there is no
  index to measure.
