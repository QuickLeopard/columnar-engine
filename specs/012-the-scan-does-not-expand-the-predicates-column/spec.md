# Spec 012 — The scan does not expand the predicate's column

**Status**: in progress
**Started**: 2026-10-26
**ADR**: [ADR-0018](../../docs/adr/0018-the-scan-reads-the-parquet-decode.md)
**Predecessor**: [011-the-scan-filters-only-what-it-keeps](../011-the-scan-filters-only-what-it-keeps/spec.md)

## The problem

The cold path's one remaining loss is `filter-range`, by **1.1 ms**: the engine's lazy path takes 17.5 ms against
Polars' 16.4 ms. The scan that holds it is 15.7 ms, and it decomposes like this:

| | |
|---|---|
| **the Parquet decode** of the two columns the query reads | **10.4 ms** |
| the engine's own work on top of it | **5.4 ms** |

ADR-0018 measured what the engine could do about that decode — read the file without Arrow, into its own column
format — and the answer was **nothing**: the shape without Arrow came out equal to the engine's scan, to a tenth of a
millisecond. Arrow's layer is worth 0.3 ms. The decode is the parquet crate's, and it is the floor.

But the *same* measurement pointed at where the decode's time is.

## The measurement that points at it

The file's columns are **dictionary-encoded**. Per the Parquet footer's own metadata, one row group of 1,000,000 rows:

| column | encodings | uncompressed |
|---|---|---|
| `l_quantity` | PLAIN, RLE, **RLE_DICTIONARY** | 1,667,405 bytes — **1.67 bytes a row** |
| `l_discount` | PLAIN, RLE, **RLE_DICTIONARY** | 503,674 bytes — **0.50 bytes a row** |

A `float64` column at 1.67 bytes a row is not storing floats. It is storing a **dictionary of a few dozen values** and
a million **RLE indices** into it, and the decode is the work of **expanding** those indices back into a million
values.

**The engine throws most of that expansion away.** The predicate column is expanded into six million floats,
compared row by row, and then dropped — because the query does not materialise it. And it does not have to be
compared row by row at all: the engine's dictionary path already compares **one distinct value at a time**, which is
why the predicates cost 1.4 ms rather than 14.

So the expansion is the candidate, and it is a specific one:

> **the predicate's column is expanded into six million values, compared, and thrown away — and the engine already
> knows how to compare a column without expanding it.**

## The requirements

### R1 — The measurement comes first

- [ ] The share of the decode that is the **expansion** is measured, not argued: the decode of a dictionary-encoded
      column against the decode of the same column without one.
- [ ] The measurement says what the expansion is worth **in the scan**, with the engine's own read path and the real
      file, and not in a microbenchmark of the expansion alone.
- [ ] If the measurement says the expansion is not the cost, **the increment is refused and the measurement is
      published**.

### R2 — The predicate's column is not expanded

- [ ] The scan reads the predicate column's **dictionary and indices**, and compares the dictionary.
- [ ] The mask that comes out of the dictionary is applied to the **indices**, not to expanded values.
- [ ] The expanded values of the predicate's column **do not exist** at any point.

### R3 — The semantics do not change

- [ ] The rows the scan keeps are the rows it kept before, **bit for bit** — including the NULL rows, the NaN
      thresholds and the dictionary's own semantics.
- [ ] The engine's comparison semantics stay the engine's: a NULL never matches, a NaN never matches, for every
      operator including `!=`. Arrow's `totalOrder` is not substituted for them.
- [ ] A column the query **materialises** is still expanded, because the materialised column is the output.

### R4 — The tests

- [ ] The scan's rows are compared against the scan's rows **before** this change, over the real predicates, over
      every operator, and over the NULL and NaN edges.
- [ ] A dictionary whose values are **not sorted**, and whose indices are **not monotonic**, is tested — because an
      implementation that assumes the dictionary is sorted would pass on this file and fail on another.
- [ ] **The mutations were run**, and each one was caught.

### R5 — The publication

- [ ] The headline table is **re-measured**: three full campaigns, medians and spreads, the raw reports committed.
- [ ] ADR-0018 and this spec are updated with what was measured, whichever way it went.

## The non-goals

- **The engine does not own its Parquet reading.** ADR-0018 measured that as worth nothing.
- **The materialised column is not changed.** It is the output, and the output is expanded values.
- **The eager path is not changed.** It materialises all sixteen columns, and the expansion is the data it wants.
