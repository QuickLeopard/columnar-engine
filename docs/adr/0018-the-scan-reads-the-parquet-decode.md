# ADR-0018: The scan's read is the Parquet decode, and owning it is not worth it

**Status**: accepted
**Date**: 2026-10-26
**Related**: [ADR-0017](0017-the-scan-filters-only-what-it-keeps.md), [spec 012](../../specs/012-the-scan-does-not-expand-the-predicates-column/spec.md)

## Context

The cold path's one remaining loss is `filter-range`, by **1.1 ms**: the engine's lazy path takes 17.5 ms against
Polars' 16.4 ms. The scan that holds it is 15.7 ms of the query, and it decomposes like this:

| | |
|---|---|
| **the Parquet decode** of the two columns the query reads | **10.4 ms** |
| the engine's own work on top of it — the compaction 2.5 ms, the predicates 1.4 ms, the rest the scan's own overhead | **5.4 ms** |

The obvious next move is the one every columnar engine makes eventually: **stop going through Arrow and read the
Parquet file into the engine's own column format.** Arrow's reader is a thin wrapper over the parquet crate's own
reader — it decodes a column chunk into a buffer and *moves* that buffer into an Arrow array — so the engine could
use the parquet crate directly, apply its own mask to the decoded values, and write the survivors straight into its
own column. One less layer, one less copy, the engine's own format end to end.

It looked like the remaining gap. **It is not.**

## The measurement

Before anything was built, the shape was measured, the way every other shape of work in this repository has been. The
probe is in the tree, as `crates/columnar-engine/examples/own_decode_probe.rs`, and it measures four things on the
same file, in the same process, warm, each the median of five runs:

| | |
|---|---|
| **A** | the parquet crate's own decode of the two columns, into plain Rust buffers | **10.30 ms** |
| **B** | Arrow's decode of the same two columns | **9.86 ms** |
| **C** | A, plus the engine's predicate over the decoded buffer, plus the compaction of the survivors into the engine's own column | **15.73 ms** |
| **D** | the engine's own scan, as it is today | **15.74 ms** |

**C is D.** The shape without Arrow is equal to the engine's scan, to a tenth of a millisecond, and the difference is
inside the noise. And **A is B**: Arrow's reader is not slower than the parquet crate's raw reader, because it *is*
the parquet crate's reader with an array wrapper on top, and the wrapper costs nothing measurable.

Three things fall out of that, and they decide the work:

- **Arrow is not the cost.** The whole layer between the parquet crate and the engine's values — the `ArrayData`, the
  `Arc`, the null bitmap — is worth **0.3 ms** on a 15.7 ms scan. There is nothing there to win.
- **The decode is the parquet crate's, and it is the floor.** 10.4 ms of a 15.7 ms scan is the decompression and the
  expansion of six million rows × two columns, and that is the *same code* with or without Arrow.
- **So the engine's own work is the only part the engine can move**, and it is 5.4 ms: the compaction (2.5 ms), the
  predicates (1.4 ms) and the scan's own overhead (the rest).

## Decision

**Rejected. The engine keeps Arrow.**

The rewrite was prototyped as a full read path — the parquet crate's own typed column reader, the engine's kernel
over the decoded buffer, the engine's own compaction into its own column — and it came out **equal** to what the
engine already does. Paying a large rewrite, in the read path, for a measured zero is not a trade this repository
makes.

There is one more reason, and it is the repo's own: **a measurement that says "no" is worth more than the increment
it refuses.** A future contributor does not have to spend a week discovering that the Arrow layer is worth 0.3 ms.
This ADR is the receipt.

## What the measurement says to do instead

The probe did not just say no. It said where the time is, and the file's own metadata says what the time is doing.

The file's columns are **dictionary-encoded**. Per the Parquet footer:

| column | encodings | uncompressed, per row group of 1,000,000 rows |
|---|---|---|
| `l_quantity` | PLAIN, RLE, **RLE_DICTIONARY** | 1,667,405 bytes — **1.67 bytes a row** |
| `l_discount` | PLAIN, RLE, **RLE_DICTIONARY** | 503,674 bytes — **0.50 bytes a row** |

A `float64` column with 1.67 bytes a row is not storing floats. It is storing a **dictionary of a few dozen values**
and six million **RLE indices** into it, and the decode is the work of **expanding** those indices back into six
million values.

**And the engine does not need most of that expansion.** The predicate column — `l_discount`, whose eleven distinct
values the engine compares *once each* through the dictionary path — is expanded into six million floats, compared
row by row, and then thrown away. If the engine read the dictionary and the indices, and compared the **dictionary**,
the expansion of the predicate's column would never happen: the mask would be eleven bits wide, and the rows would be
selected by the indices.

That is the next increment, and it is **spec 012**: *the scan does not expand the predicate's column*. It is a
hypothesis, and it is measured before anything is built — the way this one was.

## What this does not do

- **It does not make the read faster.** The scan is 15.7 ms before and 15.7 ms after; nothing in the engine changed.
- **It does not close the 1.1 ms gap.** It explains where the 1.1 ms is: in the decode, and specifically in an
  expansion the engine may not need.
- **It does not touch the eager path.** The eager path reads all sixteen columns and materialises them, and the
  expansion is the data it wants.
- **It does not say the parquet crate is fast.** It says the parquet crate is the same speed with or without Arrow,
  which is a different claim.
