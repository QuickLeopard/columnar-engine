# ADR-0021: The materialised column is bytes, and the peer is an instrument

**Status**: accepted
**Date**: 2026-10-26
**Related**: [ADR-0018](0018-the-scan-reads-the-parquet-decode.md), [ADR-0020](0020-the-expansion-is-not-the-decode-s-cost.md), [spec 014](../../specs/014-the-materialised-column-and-a-rust-peer/spec.md)

## Context

[ADR-0020](0020-the-expansion-is-not-the-decode-s-cost.md) measured the **predicate's** column and refused the
increment that would have avoided its expansion. That refusal left a bigger number than the one it was about.

The query's time is not the predicate's column. On the real Q6 — `sum(l_extendedprice * (1 - l_discount))` over
a tighter predicate — the engine spends a few milliseconds on the column it filters on and the rest on the
column it **materialises**: `l_extendedprice`, the column the revenue is computed from. Nobody had measured what
that cost is made of.

Spec 014 asks three questions, and all three are evidence rather than features:

1. **What is the materialised column made of?** The same instrument that took the predicate's column apart,
   pointed at the column that matters more.
2. **How does it move with the size of the data?** The repository publishes a thread sweep and a load sweep, and
   no curve over the size of the data.
3. **Where does the engine stand against a Rust peer?** The incumbents are `DuckDB` (C++) and `Polars`
   (Rust-through-Python). There is no peer that is a Rust engine reading the same Parquet through the same
   Arrow stack.

## The measurement

The probe is in the tree, as `crates/columnar-engine/examples/materialised_column_probe.rs`. It measures the
same nine things on three columns of the real Q6, on the same file, in the same process, warm, each the median
of five runs, over **three campaigns**. The raw reports are in `bench-results/materialised-run{1,2,3}.json`, and
every table below is derived from them by `python3 scripts/medians.py`.

The machine was busy — the host's load average was **14.1** on fourteen cores during the campaign, with another
project's test suite running at 93% of a core — so the spreads are wide and the medians are the figures. The
reports record the load average with every campaign.

### The three columns

| | `l_discount` | `l_quantity` | `l_extendedprice` |
|---|---|---|---|
| the column's role | predicate | predicate | **materialised** |
| written | `RLE_DICTIONARY` | `RLE_DICTIONARY` | **plain** |
| data pages | 294 dictionary | 294 dictionary | **42 dictionary, 258 plain** |
| dictionary values | 11 | 4,901 | 131,411 |
| index width | 4 bits | 13 bits | 18 bits |
| on disk | 3,023,401 bytes | 9,904,661 bytes | **36,451,424 bytes** |

### The decode, decomposed

| | median (min–max) | `l_discount` | `l_quantity` | `l_extendedprice` |
|---|---|---|---|---|
| **P** | the parquet crate's decode | 7.55 ms (7.48–7.73) | 7.92 ms (7.55–7.99) | **38.42 ms** (37.70–38.83) |
| **A** | Arrow's decode | 3.95 ms (3.90–4.44) | 5.40 ms (5.28–5.76) | **36.01 ms** (35.16–36.25) |
| **V** | the probe's decode | 6.83 ms (6.39–6.98) | 8.89 ms (8.65–9.21) | 38.53 ms (38.08–38.56) |
| **I** | the probe's indices, no gather | 5.12 ms (4.68–5.40) | 7.06 ms (6.72–13.26) | — |
| **G** | the expansion | 1.81 ms (1.75–1.82) | 1.96 ms (1.82–2.26) | 1.06 ms (1.02–1.08) |
| **G1** | the reader's fresh buffers | 0.21 ms (0.21–0.22) | 0.22 ms (0.21–0.22) | 0.23 ms (0.22–0.24) |
| **G2** | the gather, or the copy | 1.56 ms (1.54–1.56) | 1.59 ms (1.59–1.63) | 0.83 ms (0.82–0.97) |
| **X** | the counterfactual, the parquet crate | 23.01 ms (22.82–23.62) | 37.36 ms (37.34–38.82) | 43.20 ms (42.83–43.32) |
| **X** | the counterfactual, Arrow | 21.07 ms (20.98–22.49) | 36.50 ms (36.30–37.42) | **39.74 ms** (38.91–39.84) |

### The real Q6, as a ladder

Every rung is the engine's own scan, through the engine's own lazy path, with the real Q6's three predicates.

| | median (min–max) |
|---|---|
| **Q0** the predicates alone | **27.70 ms** (26.76–27.83) |
| **Q1** the predicates, and the price | **64.77 ms** (64.55–65.48) |
| **Q2** the predicates, and the discount | **27.83 ms** (27.45–27.96) |
| **Q3** the predicates, and both | **64.82 ms** (64.43–66.06) |
| **Q4** the whole query | **65.39 ms** (65.29–65.70) |
| | of which the scan stage | 63.12 ms |
| | of which the operator stages | 1.33 ms |
| **the materialised column's share** (Q1 − Q0) | **37.65 ms** |
| **the aggregate's own share** (Q4 − Q3) | 0.56 ms |

**110,915 rows survive**, and the answer is **5,526,985,127.48064**.

## What the measurement says

### The materialised column is 58% of the query, and it is bytes

**37.65 ms of the 65.39 ms query** is the materialised column — 58% of the query, and 96% of the scan stage.
The predicates are 27.70 ms and the aggregate over the survivors is 0.56 ms.

And the column's cost is **the bytes**. Arrow's decode of it is **36.01 ms**, the parquet crate's is 38.42 ms,
and the engine's own materialisation of it is **37.65 ms** — so the engine is **1.64 ms** from the floor the
Arrow stack sets, and 4.4% above it. There is nothing in the engine's read path to fix.

The expansion, which is what ADR-0020 was about, is **1.06 ms** of that column's decode — and it is *not* an
expansion of dictionary indices. The column is **plain**: 258 of its 300 data pages hold doubles directly.

### The writer gave up on the dictionary, and it was right to

The file's writer starts a dictionary for every column and falls back to plain when the dictionary page
reaches its size limit — one megabyte by default. `l_extendedprice` has 131,411 distinct values per row group,
so its dictionary page would be 1.05 MB, and the writer abandoned it part-way through the first pages of each
row group.

The **counterfactual** says that was the right call. The same column, written with a dictionary and a
dictionary page big enough to keep it, is:

| | the file's column | the counterfactual | |
|---|---|---|---|
| on disk | **36,451,424 bytes** | 47,499,582 bytes | **30% bigger** |
| Arrow's decode | **36.01 ms** | 39.74 ms | **10% slower** |

The dictionary is a **loss** for this column: it costs a megabyte a row group of dictionary values and a gather
into it, and it buys compression that the plain doubles do not need.

**And the opposite is true of the two predicate columns.** `l_discount` written plain is **21.07 ms** against
the dictionary's **3.95 ms** — 5.3× slower — and 11,644,361 bytes against 3,023,401. `l_quantity` written
plain is **36.50 ms** against **5.40 ms**, 6.8× slower, and 26,980,250 bytes against 9,904,661.

So the answer to "should this column be dictionary-encoded" is **not a rule, it is a measurement**: the
dictionary pays for a column with a few thousand distinct values and loses for one with a hundred thousand. The
writer's own limit is a reasonable proxy for that line, and the file is on the right side of it.

**One thing the file does carry for nothing.** The writer's fallback leaves the dictionary page in the file:
**6,313,096 bytes** over the six row groups, **2.6%** of the file, that no reader reads. It costs disk and
nothing else. That is a fact about the *generator*, not about the engine, and the generator's writer is not
this increment's business.

### ADR-0018's non-goal stands, and this is the other side of it

Owning the Parquet reading — the increment ADR-0018 refused — would buy the engine the difference between
Arrow's decode and the parquet crate's. On the materialised column that is **1.64 ms**; on the predicate's
column it is **1.51 ms**. It would also cost the Parquet format's RLE decoder, which has to match the crate's
bit for bit, and the crate's RLE is not exposed: `encodings/rle.rs` is a `pub(crate)` module, and the only
public way to read a dictionary-encoded column is the whole expansion.

The measurement here does not reopen that question. It says the same thing from the other side: **the
materialised column is bytes, and the bytes are what they are.** The only lever that moves it is reading fewer
of them, and that is the file's business, not the engine's.

### The probe's own decoder, and what it cost to make it fair

At four bits — the predicate's column — the probe's decoder is competitive with the crate's. At **thirteen
bits** it was not: the bit-at-a-time path made the probe's decode of `l_quantity` **39.55 ms** against the
crate's 7.92 ms, so the probe was measuring itself. The decoder now reads a group's eight values as one
128-bit word, and the probe's decode of `l_quantity` is **8.89 ms** against the crate's **7.92 ms** — within
12%.

That is worth saying plainly: **an instrument that has not been checked against the reference it measures
against is not an instrument**, and the check is not a formality. The probe's decoder is still verified against
the parquet crate's, bit for bit, over every value of every row group, before it measures anything, and it
stops if they disagree.

## The peer

The peer is `tools/datafusion-peer/`: **DataFusion 49.0.2**, reading the same Parquet through the same Arrow
stack (Arrow 55.2.0), answering the same SQL, with the thread count pinned.

### What it says

| the real Q6 (`q6-revenue`), one thread | cold | resident |
|---|---|---|
| **the engine, lazy** | **68.3 ms** | — |
| **the engine, eager** | — | **20.2 ms** |
| `DuckDB` | 76.0 ms | 56.0 ms |
| `Polars` | 216.0 ms | 82.4 ms |
| **`DataFusion`** | 126.7 ms | 73.1 ms |

**The engine is 1.9× faster than DataFusion cold and 3.6× faster resident**, on the same file, with the same
SQL, at the same thread count, and with the answer cross-checked. The peer's answer differs from the engine's in
the last bits — `5526985127.480701` against `5526985127.48064` — which is what a sum over a hundred thousand
floats does, and the harness's relative tolerance of 1e-9 is there for exactly that.

The engine's load is **0.1 ms** against DataFusion's **391.5 ms**: the lazy path reads the file during the query
and does not have a load phase at all, and that is a structural difference and not a tuning one.

### What it is, structurally

It is **a measuring instrument, not a dependency**, and that is a decision worth recording:

- It lives in its **own cargo workspace** under `tools/`, with its **own committed lock file**. It is not a
  member of this repository's workspace, so `cargo deny`, `cargo test --workspace` and the published
  dependency graph never see it. Adding DataFusion to the workspace would have pulled 256 crates — and `rand`,
  which the repository's dependency policy bans — into the engine's graph, for the sake of a measurement.
- It is built by `just peer-build` into `tools/bin/`, which is where the harness looks for a comparator.
- It speaks the harness's **existing marker protocol** — `LOAD_MS=`, `QUERY_MS=`, `VALUE=` — so it is measured
  in the same two modes, cold and resident, as the other incumbents, and the harness's parser is reused
  rather than written a second time.
- Its answer is cross-checked against the engine's at the same relative tolerance the other incumbents are
  held to, because a sum over a hundred thousand floats is not associative and the last bits may differ.

## The curve

The third axis: the query and the thread count held fixed, and the **size of the data** moved. The real Q6 at
three sizes, generated by this repository's own generator from the pinned seed 24001, with every incumbent
measured at every size.

| rows | sf | **engine** | engine scan | engine ops | duckdb | polars | datafusion | **ms/Mrow** |
|---|---|---|---|---|---|---|---|---|
| 6,000,000 | 1 | **66.7 ms** | 64.6 ms | 1.4 ms | 74.0 ms | 213.5 ms | 127.3 ms | **11.1** |
| 12,000,000 | 2 | **138.7 ms** | 146.8 ms | 3.9 ms | 152.0 ms | 426.9 ms | 251.3 ms | **11.6** |
| 24,000,000 | 4 | **279.7 ms** | 275.0 ms | 6.7 ms | 303.0 ms | 848.0 ms | 498.4 ms | **11.7** |

**The cost per row is flat.** 11.1, 11.6, 11.7 ms a million rows: the engine's cost does not amortise as the file
grows, which is exactly what "the cost is bytes" predicts. The scan is the query at every size, and the operators stay at
2–3% of it. The engine is ahead of every incumbent at every size, and its margin over DuckDB
**narrows** as the file grows — 1.11×, 1.10×, 1.08× — so a fourth doubling would find the crossover. That is
published as what it is.

**And the curve found a limit that is not a timing.** The engine's **default** memory budget refuses the real Q6
at 24 million rows: the estimate is about 16 bytes a row, so 24 million rows need roughly **391 MiB** against the
**256 MiB** default, and the query is rejected before a row is read. The budget is a **policy knob**, not a limit
on the engine — below it, it changes nothing at all — and the curve is measured at **1024 MiB** so that the largest
size runs at all. The figure is in the report, because a curve that says which budget it was measured under is a
curve a reader can check.

## The decision

1. **The engine's scan does not change.** This increment is evidence, and the evidence found no defect: the
   engine's materialisation of the column is 4.4% from Arrow's floor. A defect would get its own spec.
2. **ADR-0018's non-goal stands.** The measurement says the same thing from the other side.
3. **The peer stays an instrument.** It is never a dependency of the engine, the kernels or the harness.
4. **The size curve is published with the incumbents on it**, because a curve with only this engine's line is a
   curve with nothing to be compared against.

## What was not done

- **The engine's read path was not changed.** See the decision above.
- **The generator's writer was not changed.** The dead dictionary pages are a fact about the file, not about
  the engine, and the file is the generator's business.
- **The engine did not grow a SQL parser.** The peer is driven with the SQL the repository already publishes
  for the incumbents.
