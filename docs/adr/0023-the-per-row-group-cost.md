# ADR-0023: The per-row-group cost, and the partial batch

**Status**: accepted
**Date**: 2026-10-26
**Related**: [ADR-0011](0011-parallel-decode.md), [ADR-0022](0022-the-parallelism-above-the-row-group-count.md), [spec 016](../../specs/016-the-per-row-group-cost/spec.md)

## Context

Spec 015 made the row-group count a knob and measured what it does. The cap is the file's:
`min(threads, row_groups)`, exactly, and the engine's speedup at fourteen threads rose with it.

The same measurement found the other half, and it is a defect:

> **The engine's one-thread figure rises by 14.9 ms — 76% — across the three files, and the three
> incumbents' figures do not move at all.**

The engine paid about **0.83 ms a row group** that `DuckDB`, `Polars` and the `DataFusion` peer did
not pay, reading the same bytes on the same machine. The consequence was that **the more parallel
file was the slower one**: the engine's best figure was on the six-row-group file, and the
twenty-four-row-group file — the one with the most parallelism — was the slowest.

Spec 015 wrote the mechanism down as this spec's question. This is that spec, and it found the
mechanism.

## The candidates

The cost could have been in three places, and the measurement had to say which:

1. **The parquet crate's read**, one row group at a time — the crate opens a column reader per row
   group, and reads and decodes that row group's pages.
2. **The engine's pushdown**, which compares a dictionary-encoded column once per distinct value,
   per batch — and a row group is a batch boundary.
3. **The engine's materialisation**, which copies the survivors out of the batch.

## The measurement

`crates/columnar-engine/examples/row_group_cost_probe.rs` measures the **crate's read on its own**
— the same projection, no pushdown, no materialisation — and the **engine's scan**, on all three
files, and sweeps the reader's batch size over eight values on each of them. The raw reports are
`bench-results/row-group-cost-{6,12,24}-run{1,2,3}.json`, and
`python3 scripts/medians.py` derives every figure below from them.

**The crate's read is the whole of it, and within the crate's read it is one column.** The crate's
read, at the fixed 65,536-row batch the tree used:

| the crate's read, one thread | 6 row groups | 12 row groups | 24 row groups | ms a row group |
|---|---|---|---|---|
| `l_discount` alone | 5.35 ms | 6.03 ms | 5.52 ms | 0.010 |
| `l_extendedprice` alone | 5.05 ms | 5.57 ms | 5.32 ms | 0.015 |
| **`l_shipdate` alone** | **14.64 ms** | **23.96 ms** | **39.35 ms** | **1.373** |
| both predicate columns | 20.14 ms | 29.96 ms | 45.56 ms | 1.412 |
| the crate's read, with the pushdown on top | 30.67 ms | 39.14 ms | 54.61 ms | 1.330 |
| **the engine's own scan, at the derived batch** | **23.25 ms** | **24.27 ms** | **24.97 ms** | **0.096** |
| the engine's own scan, and the materialised column | 30.71 ms | 31.27 ms | 32.16 ms | 0.081 |

`l_discount` and `l_extendedprice` are **flat** across the three files. `l_shipdate` carries the
entire per-row-group cost: (39.35 − 14.64) / 18 = **1.373 ms a row group**, and the engine's own
work above the crate is a **constant** — the pushdown and the materialisation do not scale with the
row-group count.

**The dictionary's bytes are not it.** The two predicate columns' dictionary pages total 66,239
bytes over six row groups and 264,572 over twenty-four — eleven kilobytes a row group, against a
cost of 1.373 ms a row group. And the *size* of a dictionary is not it either:
`l_extendedprice`'s dictionary holds **131,411** values and its read is flat at 0.015 ms a row
group.

**Which of `l_shipdate`'s properties makes it the one that pays** — its string encoding, its
dictionary, its page count, or its size — is **not separated by this measurement**, and it is
written down as the next question rather than guessed at.

## The mechanism: the partial batch

The crate's reader reads a row group at a time, and the reader's **batch size** decides how many
rows a batch holds. When the batch size does not **divide** a row group's row count, the row
group's last batch is **partial**.

The engine's batch size was **65,536 rows**, and 65,536 divides none of the three files' row-group
sizes: 1,000,000, 500,000 and 250,000. So the engine left a partial batch at the end of every row
group.

The engine's own scan, by reader batch size, on all three files:

| reader batch size | divides the row count? | 6 row groups | 12 row groups | 24 row groups | ms a row group |
|---|---|---|---|---|---|
| 12,500 | yes | 34.66 ms | 35.08 ms | 36.36 ms | 0.095 |
| 25,000 | yes | 27.78 ms | 28.24 ms | 29.42 ms | 0.091 |
| 31,250 | yes | 27.15 ms | 27.69 ms | 28.93 ms | 0.099 |
| 50,000 | yes | 24.85 ms | 25.35 ms | 26.63 ms | 0.099 |
| 62,500 | yes | 24.49 ms | 24.63 ms | 25.57 ms | 0.060 |
| **65,536** | **no** | **30.79 ms** | **38.53 ms** | **54.48 ms** | **1.32** |
| 125,000 | yes | 23.22 ms | 23.31 ms | 24.01 ms | 0.04 |
| 250,000 | yes | 22.83 ms | 23.18 ms | 23.91 ms | 0.060 |

**Seven of the eight batch sizes divide the row group's row count, and every one of them costs
0.06–0.10 ms a row group. The eighth — 65,536, which divides none of the three row counts — costs
1.32.** The mechanism is a property of the *shape* of the number and not of its size.

**Why the incumbents do not pay it** is not measured, and it is not measured because it is not the
engine's to measure: `DuckDB` and `Polars` do not use this reader, and the `DataFusion` peer does
not leave a partial batch behind in the same way. What *is* measured is that they read the same
bytes in the same time.

## The fix

**The reader's batch size is the largest divisor of the row groups' row count that fits under the
limit.** The limit stays a limit — it bounds the transient batch — and the batch size is derived
from the file.

The rule is the **greatest common divisor** of the row groups' row counts, rounded down to a
divisor that fits under the limit:

- A file with **uniform** row groups gets **one batch a row group**, and no partial batch anywhere.
- A file whose **last** row group is short gets the greatest common divisor of its row counts, which
  divides every one of them — so the short row group does not force a small batch on the rest.
- A row group whose row count has **no usable divisor** — a prime, say — is read in batches of the
  limit, and ends with a partial batch. That is the cost of an awkward row count, and it is paid
  once per row group.

A divisor under **4,096 rows** is not used: a batch of a handful of rows costs more in per-batch
overhead than it saves.

The limit moves from 65,536 to **131,072** rows, and it is **measured**, because the limit is not
the batch size any more: a bigger limit finds a bigger divisor. At the old limit the published
file's million-row row group divided by 62,500; at the new one it divides by 125,000.

Which limit is right is not obvious, because the two shapes the lazy path serves pull in opposite
directions. `crates/columnar-engine/examples/lazy_batch_size.rs` measures both — one materialised
column with one predicate, and the real Q6's predicate set — and A/Bs the batch the tree used
before against the batch it derives now, with the order alternating on every round so that a
drifting machine cannot bias the result. The raw reports are
`bench-results/batch-size-run{1,2,3}.json`.

| the limit | the batch it derives | one column, one predicate | the real Q6's predicate set |
|---|---|---|---|
| 1,024 | 1,024 | 30.2 ms | 171.3 ms |
| 4,096 | 4,096 | 24.3 ms | 77.6 ms |
| 16,384 | 15,625 | 24.2 ms | 31.8 ms |
| 65,536 | 62,500 | 21.9 ms | 24.4 ms |
| **131,072** | **125,000** | **21.88 ms** | **20.87 ms** |
| 262,144 | 250,000 | 22.8 ms | 20.5 ms |

The one-column shape is best at 125,000 and the Q6 shape at 250,000, so the limit is the one that
is best on one and within 2% of the best on the other: **131,072**, which derives 125,000 rows for
the published file. The limit 262,144 was measured and **rejected**: it derives 250,000 rows,
which is 3.7–8.5% off the one-column shape's best.

**The interleaved A/B is the figure that decides it**, because it holds everything else constant
and alternates the order:

| the batch | one column, one predicate | the real Q6's predicate set |
|---|---|---|
| 65,536 — the batch the tree used | 22.0 ms | 27.5 ms |
| **125,000 — the batch the tree derives** | **21.9 ms** | **20.89 ms** |
| the difference | −0.07 ms — **−0.3%** | −6.63 ms — **−24.1%** |

The **eager** read gets the same rule, because it has the same defect: it reads the whole file, so
it had a partial batch at the end of every row group too. Its limit is smaller — 32,768 — because it
reads all sixteen columns, and the per-batch cost scales with the number of columns.

## The result

The engine's own scan, at one thread, **in one session, with the two batch sizes alternating round
by round** — the same binary, the same file, the same process, one row broken and one row fixed:

| the engine's scan | 6 row groups | 12 row groups | 24 row groups |
|---|---|---|---|
| the fixed 65,536 rows — the defect | 30.79 ms | 38.53 ms | 54.48 ms |
| **the derived 125,000 rows — the fix** | **23.22 ms** | **23.31 ms** | **24.01 ms** |

**The per-row-group cost is gone.** The defect's cost is 7.6, 15.2 and 30.5 ms; the fix's is flat
within 3.4%. The cost of one more row group fell from **1.32 ms to 0.04 ms**, a 97% reduction,
measured on the probe's own back-to-back ladder. The engine's best figure is now on the file with
the **most** parallelism — the claim spec 015 could not make.

**The engine's answer does not change**: the same rows, in the same order, and the same value. That
is checked by the test suite, not asserted.

**The figures in this ADR are this campaign's, and the absolute numbers are the host's.** The
campaign ran on a machine under another tenant's memory pressure, so every absolute figure here is
larger than the one the earlier probe run produced on the same measurement — the `l_shipdate` read
was 10.58 / 15.37 / 25.66 ms there, against 14.64 / 23.96 / 39.35 ms here, and the cost of a row
group was 0.84 ms there against **1.373 ms** here. The *ratio* is the claim, and the ratio is
larger: the defect is worth more on this host than on a quiet one. The **old and the new figure**
are both given, and the reason is named, as
[`PERF.md` §3j](../../PERF.md#3j-the-row-group-count-and-what-it-costs) requires.

## The decision

1. **The reader's batch size is derived from the file's row groups**, not fixed. The limit bounds
   the transient batch; the row groups decide the batch.
2. **The eager read gets the same rule.** It had the same defect.
3. **The limit is measured, over both shapes the lazy path serves.** It moves from 65,536 to
   **131,072** rows, which derives 125,000 rows for the published file: the best measured batch on
   the one-column shape, and within 2% of the best on the Q6 shape.
4. **The crate is not changed.** The cost is in a dependency the engine does not own, and the fix
   is in how the engine calls it.
5. **The measurement is published**: the mechanism, the fix, the before and the after.

## What was not done

- **Why the crate charges for a partial batch was not established.** The cost is in the crate's
  arrow reader's batch assembly, and the crate is a dependency. What is measured is the behaviour —
  a partial batch costs 1.32 ms on this shape — and that is enough to design around it.
- **Which of `l_shipdate`'s properties makes it the one column that pays was not separated.** The
  read of the other two columns is flat, and the string column's read is not; whether that is its
  encoding, its dictionary, its page count or its size is not settled by this measurement, and it is
  written down as the next question rather than guessed at.
- **The residual per-row-group cost on the three-predicate shape was not separated.** The real Q6's
  **three**-predicate shape still rises by about **0.14 ms a row group** — the predicates alone go from
  21.31 ms on the six-row-group file to 23.75 ms on the twenty-four-row-group one, over the eighteen
  row groups between them — and the two shapes differ by exactly one thing: the third predicate's
  column, `l_quantity`, which is dictionary-encoded with 4,901 values a row group. Whether that is the
  crate building that dictionary per row group, or the engine comparing its distinct values per row
  group, is not separated by this measurement.
- **The generator's writer does not change.** The dead dictionary pages are a fact about the file,
  not about the engine — the same call ADR-0021 and ADR-0022 made.
- **The plan does not change.** No new predicate, no new stage, no new plan shape.
- **The parallelism cap does not move.** `min(threads, row_groups)` is the file's and it is
  published as such.
