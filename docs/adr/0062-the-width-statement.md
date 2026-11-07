# ADR-0062: The width statement — one number for what a row costs, and the batch is charged

**Status**: accepted
**Date**: 2026-11-07
**Related**: [ADR-0061](0061-the-batch-sizes-price.md), [ADR-0017](0017-the-scan-filters-only-what-it-keeps.md)

## Context

[ADR-0061](0061-the-batch-sizes-price.md) priced the reader's batch size and decided the rule's shape: **a byte
budget**. **And a byte budget has to know what a row costs before the read**, and there was no honest statement of
that to divide it by.

**The engine charged a `utf8` column 24 bytes a row** — a `String` header — **and a dictionary-encoded column's real
cost is its index's four.** The estimate's own document said so: *"the dictionary is shared by the whole column, and
the per-row cost is the index."* **Both paths did it**:

- The **eager** path's estimate had the chunk's encoding in hand and charged the header anyway.
- The **lazy** path's estimate could not see the encoding at all, and charged the same.

**And the file's own footer is not the statement either.** A column chunk's `total_uncompressed_size` over its
`num_values` is the **file's** bytes a row, and it is **the file's pages and not the working set**: the sixteen
columns come to **70.70** bytes a row that way, against the **173.88** the allocator measured, and the file stores
`l_discount` in **0.50** bytes a row where the engine holds it in **eight**.

**So three candidates were rejected** — the file's pages, the schema's bare type, and the estimate — and **the
signal was the hard part**:

**The footer's `encodings` list names `RLE_DICTIONARY` for a column that is not dictionary-encoded at all.** The
published file's `l_orderkey` lists `('PLAIN', 'RLE', 'RLE_DICTIONARY')`, and the engine reads it as a plain
`int64`. **The chunk's own dictionary page** is the honest signal for whether the *file* stored a dictionary — and
**it is not the signal the engine needs**, because the engine's cost depends on the layout the **reader** produces,
and the reader follows the **Arrow schema**.

**And the Arrow schema does say**: a `Dictionary(_, Utf8)` field is a column of **indices** and a `Utf8` field is a
column of **headers** — and it is the same signal the engine's own builders answer to, because the engine's
`Utf8Dict` chunk is built from a `DictionaryArray` and its `Utf8` chunk from a `StringArray`.

## The measurement

**The statement, checked against a counting global allocator**, at the three widths of
[§3af](../../PERF.md#3af-the-batch-sizes-price). Three campaigns of three repetitions, on one host, load average
between **6.46** and **6.91**. **All in bytes a row:**

| the shape | the statement | the file's own footer | the measurement | the difference |
|---|---|---|---|---|
| the two predicate columns | **13.79** | **2.04** | **13.39** | **−0.40** |
| the two materialised columns | **16.25** | **16.53** | **16.03** | **−0.22** |
| the file's sixteen columns | **159.02** | **70.70** | **173.88** | **+14.86** |

**The statement is within 3% on the two narrow shapes and 9% low on the wide one, and the file's own footer is
2.46× low on the wide one.**

**And the wide shape's difference is the plain strings.** The **+14.86** is the statement being low because **the
plain string's bytes are held twice in the batch** — once in Arrow's buffer and once in the engine's own copy — and
the statement counts them once. **The dictionary path borrows the batch's values; the plain path copies every string
out of it, one allocation a row.** That is a defect the measurement found, and **this increment does not fix it**.

**And the estimate, which now charges the batch:**

| the shape | 62,500 | 125,000 | 250,000 | 1,000,000 |
|---|---|---|---|---|
| the two predicate columns | 2,361,802 | **3,223,586** | 4,947,156 | **15,288,577** |
| the two materialised columns | 100,015,642 | **101,031,266** | 103,062,516 | **115,250,016** |
| the file's sixteen columns | 965,574,092 | **975,512,992** | 995,390,808 | **1,114,657,705** |

**Before**, the estimate was **one number a shape** — **1,500,016**, **99,000,016** and **1,549,500,016** — and it
**did not depend on the batch size at all**, so the budget could not see the knob's price. **Now the narrow shape's
estimate is 1.11× high at a whole row group — **15,288,577** against the read's **13,719,522**, where it was
9.15× low (**1,500,016** against **13,719,490**) — and the wide one's is 16% low (**975,512,992** against
**1,157,291,503**) where it was 25% high (**1,549,500,016** against **1,157,291,375**).** **And the reads moved
too**: the campaign was taken again after the engine changed, and the read's peak moved by **32** bytes on the
narrow shape and **128** on the wide one, so the reports hold the new figures and the old ones stand beside them.

## Decision

**One statement says what a row costs, and it lives in one place** — `crates/columnar-engine/src/width.rs`:

| the shape the row takes | the row holds | the bytes a row |
|---|---|---|
| a fixed-width value — an `int64` or a `float64` | the value itself | **8** |
| a dictionary's **index** | an `i32` into the column's dictionary | **4** |
| a plain string's `String` header | a `String` | **24** |

**And the value's own bytes ride on top**: a plain string's heap holds the string, a dictionary holds its distinct
values once for the whole column, and a number's eight bytes **are** its value.

**The estimate and the reader's batch rule read the same statement**, so they cannot disagree about what a row
costs.

**The estimate charges the batch.** A read holds the **file's rows** for the columns the dataset holds, and the
**batch's rows** for the columns the reader fetches — and a parallel read holds one batch per worker. **So the
budget's verdict on a plan now depends on the reader's batch size**, which it did not before.

**And the value's own bytes come from the file's own accounting on the lazy path** — the chunk's
`total_uncompressed_size` over its `num_values` — **for the columns whose values the engine copies**, which are the
strings. **A number's eight bytes already are its value**, so nothing is added for it: the file's pages for
`l_orderkey` are its dictionary, and the engine holds neither.

**The signal is the schema's own Arrow type**, and not the chunk's encodings.

## Consequences

- **The public surface does not break.** The trait's new method has a default body, the new functions are
  additions, and `cargo semver-checks` says **202 pass** and **58 skip** against the baseline. **The semver baseline
  does not move.**
- **And the spec said it moves.** The requirement was written before the change was made, and it said **"the semver
  baseline moves, and the move is recorded"**. **It does not move** -- a new method with a default body is not a
  break -- **and the sentence is corrected where it was published**, in the spec's R3 and in its tasks. **This ADR
  records the correction.**
- **The estimate is close, and it is not an upper bound.** The statement is what a row costs in the engine's own
  storage, and the engine's own containers hold **spare capacity** and each string is **a separate allocation** — so
  the estimate runs **16% low** against the wide shape's read. **The measurement is published beside it**, and the
  guard checks the figures inside the section that publishes them.
- **The wide shape is still refused by the default budget**, at every batch size — and now for a reason that is not
  an accident: **the plan does not fit**.
- **The default does not move.** Moving it moves **every** published figure the reader's batch size touches — the
  headline table, the thread sweeps, the ladder, the join's parts — and re-measuring those is **spec 051's** work.
- **The plain string's copy is left open.** The conversion copies every string out of the Arrow batch, one
  allocation a row, and the dictionary path does not. **That is the wide shape's gap**, and the fix wants its own
  measurement.
