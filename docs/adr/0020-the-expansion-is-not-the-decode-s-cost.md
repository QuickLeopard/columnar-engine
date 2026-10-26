# ADR-0020: The expansion of the RLE indices is not the decode's cost

**Status**: accepted
**Date**: 2026-10-26
**Related**: [ADR-0018](0018-the-scan-reads-the-parquet-decode.md), [spec 012](../../specs/012-the-scan-does-not-expand-the-predicates-column/spec.md)

## Context

[ADR-0018](0018-the-scan-reads-the-parquet-decode.md) measured that the engine's read path is the parquet crate's,
and that owning it would buy **0.3 ms**. That was a refusal, and it left one hypothesis standing, because the same
measurement pointed at where the decode's time is.

The file's columns are **dictionary-encoded**. The predicate's column, `l_discount`, is a `float64` column stored as
**eleven distinct values** and a million **RLE indices** into them — four bits a row, 502,051 bytes of indices for a
row group of 1,000,000 rows, against eight bytes a row for the floats themselves. The decode of that column is
therefore two things: decoding the indices, and **expanding** them back into six million values.

**And the engine throws the expansion away.** The predicate's column is expanded into six million floats, compared row
by row, and then dropped, because the query does not materialise it. The engine already knows how to compare a column
without expanding it — the dictionary path compares **one distinct value at a time** — so the hypothesis was specific:

> **the predicate's column is expanded into six million values, compared, and thrown away, and the engine could
> compare the dictionary instead and apply the mask to the indices.**

Spec 012's T3 is the test of that hypothesis, and R1 says the test comes first.

## The measurement

The probe is in the tree, as `crates/columnar-engine/examples/rle_expansion_probe.rs`. It measures eleven things on
the same file, in the same process, warm, each the median of five runs, over **three campaigns**. The raw reports are
in `bench-results/rle-expansion-run{1,2,3}.json`, and the table below is derived from them by
`python3 scripts/medians.py`.

| | | median (min–max) |
|---|---|---|
| **P** | the parquet crate's own decode of the column | **7.19 ms** (7.17–7.39) |
| **A** | Arrow's decode of the column — what the engine's reader does | **3.94 ms** (3.77–3.95) |
| **PLAIN** | the decode of the **same column written without a dictionary**, through Arrow | **4.63 ms** (4.58–4.95) |
| **V** | the probe's decode: the indices *and* the gather | **6.20 ms** (6.08–6.29) |
| **I** | the probe's decode of the indices **only**: no gather, no values | **4.38 ms** (4.32–4.44) |
| **G** | **the expansion**: the reader's fresh buffers, and the gather | **1.83 ms** (1.78–1.85) |
| **G1** | the reader's fresh buffers, and their zeroing | **0.21 ms** (0.20–0.21) |
| **G2** | the gather alone | **1.57 ms** (1.56–1.58) |
| **M** | **the mask the hypothesis would have to build** from the indices | **1.80 ms** (1.73–1.83) |
| **C** | the hypothesis's shape, end to end | **7.79 ms** (7.58–7.91) |
| **S1** | the engine's scan of the predicate's column | **5.42 ms** (5.37–5.58) |
| | of which the predicate itself | 1.36 ms |
| **S2** | the whole query | **43.95 ms** (43.84–44.71) |

**The probe owns the RLE**, because the parquet crate will not give it up: `encodings/rle.rs` is a `pub(crate)`
module, the `RleDecoder` it holds is unreachable even with the `experimental` feature on, `DictIndexDecoder` is in a
private module, and the only public way to read a dictionary-encoded column is the whole expansion —
`DictDecoder::get` decodes the indices *and* gathers the dictionary through them, in one call, with no seam between
the two. The probe's decoder is the format's RLE/bit-packed hybrid in about a hundred lines, and it is **checked
against the parquet crate's own reader**: it expands all six million values of every row group with its own decoder
and compares them, bit for bit, with what the crate produced. The probe stops if they disagree.

Three things come out of the measurement, and they decide the work.

### The expansion is real, and it is not the decode's cost

**G is 1.83 ms**, and the engine's decode of that column is 4.06 ms (S1 less the predicate). So the expansion is
**45% of the decode of the predicate's column** — and that decode is **9.2% of the query**, which makes the expansion
**4.2% of the 43.95 ms query**.

The decode's cost is the **bytes**. The same column, written without a dictionary, holds 48,025,815 bytes against the
dictionary's 3,012,834, and Arrow decodes it in **4.63 ms** against the dictionary's **3.94 ms**: the dictionary
encoding wins by 0.69 ms *despite* paying the 1.83 ms expansion. The expansion costs less than the extra forty-five
megabytes it saves, which is the whole reason the file is dictionary-encoded in the first place.

### The replacement is not free, and it is not cheaper than what it replaces

The hypothesis saves the expansion by not expanding. But the mask is still **one bit per row**, and the row's bit is
the predicate's answer for the row's index — so the engine has to build it per row, out of the indices. That is **M**:
**1.80 ms**.

The engine's predicate today costs **1.36 ms**, and it is **vectorised**: the kernel compares eight floats at a time
and writes the mask as it goes. The mask from the indices cannot be vectorised — there is no cheap "test a bit at a
variable index" instruction — so it is a scalar loop, and it costs **0.44 ms more** than the comparison it replaces.

So the increment's arithmetic is: **1.83 ms saved, 0.44 ms spent**, a net **1.39 ms** — *if* the indices can be had
for free.

### The indices cannot be had for free

The parquet crate does not expose them. Not through its column reader (the expansion is inside `DictDecoder::get`),
not through its `RowFilter` and `ArrowPredicate` (which evaluate on the *decoded* batch), not through
`DictIndexDecoder` (a private module), and not through `RleDecoder` (`pub(crate)`).

So the engine would have to **own the Parquet reading** — the page reader, the level decoding, the RLE decoder, the
dictionary decoding — which is this spec's own stated non-goal, and which ADR-0018 already measured and refused.

And the price of owning it is measurable. The probe's own decoder — the format's hybrid, checked bit-for-bit against
the crate — is **0.99 ms slower** than the crate's (P less V). That is **72% of the expansion it would save**, and it
cancels the increment:

| | |
|---|---|
| the expansion, saved | 1.83 ms |
| the mask from the indices, minus the predicate it replaces | −0.44 ms |
| **the net, with the crate's own RLE decoder** | **1.39 ms** |
| the probe's RLE decoder, against the crate's | −0.99 ms |
| **the net, with the decoder the probe actually wrote** | **0.40 ms** |

**0.40 ms on a 43.95 ms query**, for a rewrite of the read path and a new implementation of the Parquet format's
index encoding.

## Decision

**Refused. The engine keeps expanding the predicate's column.**

The hypothesis is not wrong about the file: the column is dictionary-encoded, the decode does expand six million
indices, and the expansion is 45% of that column's decode. It is wrong about the size of the prize and about where
the prize is:

- The expansion is **4.2% of the query**, and the query's time is dominated by the column the query actually
  materialises.
- The replacement costs **more than the comparison it replaces**, because a float comparison vectorises and a bit test
  at a variable index does not.
- Capturing the expansion at all requires **owning the Parquet reading**, and the probe's own decoder of the format
  is 0.99 ms slower than the crate's, which cancels most of what the increment would win.

**And the measurement is published**, the way ADR-0018's was, because a future contributor should not have to spend a
week discovering that the expansion is 45% of a decode that is 9.2% of a query.

## What would change this

The verdict is a measurement, not a principle, and two things would move it:

1. **An RLE decoder as fast as the crate's.** The crate's `unpack8` is a tuned word-at-a-time unpack; the probe's
   decoder is a straightforward one. A decoder that matched it would turn the 0.40 ms into 1.39 ms — still 3% of the
   query, but real.
2. **A decision that owning the Parquet reading is acceptable.** It is this spec's non-goal and ADR-0018's refusal,
   and both would have to be revisited together, not one at a time.

And one fact that an implementation would have to respect, which the probe found the hard way: **every row group has
its own dictionary**. The writer builds one per column chunk, in the order the values were first seen in that chunk,
so two row groups hold the same eleven values in a different order. An answer computed for one row group's dictionary
is wrong for the next, and the probe's first attempt at the mask was wrong by 3,671 rows because of it.

## What was measured, and where

| | |
|---|---|
| the probe | `crates/columnar-engine/examples/rle_expansion_probe.rs` |
| the raw reports | `bench-results/rle-expansion-run{1,2,3}.json` |
| the derivation | `python3 scripts/medians.py` |
| the guard | `python3 scripts/check-published-figures.py` |
| the finding, in prose | [PERF.md](../../PERF.md) §3g |

**The machine**: an Apple M4 Max, 14 logical cores, arm64, rustc 1.97.1, release profile (fat LTO, one codegen
unit). The host is **not isolated** — the campaigns ran at a load average of **9.4**, with another project's test
suite on the machine — so every figure above is a median with its spread. The expansion's own spread across the three
campaigns is **1.78–1.85 ms**; the widest *within*-campaign spread of any figure is the whole query's, at **75%**
(44–77 ms in one campaign), which is the machine and not the measurement.
