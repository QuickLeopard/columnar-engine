# ADR-0022: The parallelism above the row-group count, and what it costs

**Status**: accepted
**Date**: 2026-10-26
**Related**: [ADR-0011](0011-parallel-decode.md), [ADR-0021](0021-the-materialised-column-and-a-rust-peer.md), [spec 015](../../specs/015-the-row-group-count-is-a-knob/spec.md)

## Context

[`PERF.md` §2e](../../PERF.md#2e-the-thread-count-sweep) published a curve that flattens at six threads, and
explained it with the file's:

> **The engine's parallelism is capped by the file, not by the machine.** The read splits the file's **row groups**
> across the workers, and this file has **six** of them — so six workers is the most that can run, and the curve is
> flat from six threads on. … A file written with larger row groups would go further, and that is a property of the
> file rather than of the engine.

Two things were wrong with that paragraph, and one of them was a published error.

**The error.** *Larger* row groups is backwards. The read splits the **row groups** across the workers, so the number
of workers is bounded by the **number** of row groups: `min(threads, row_groups)`. A file written with **smaller**
row groups has more of them and goes further; a file written with larger ones has fewer and goes less far. The
sentence said the opposite of the truth, and it is the kind of sentence a reader copies.

**The gap.** The row-group count was not a knob. The generator hard-coded a million rows per row group, so every
file this repository had ever produced had six row groups per six million rows, and the claim *"the parallelism above
the file's row-group count"* had never been measured. [`PERF.md` §5](../../PERF.md#5-not-measured) said so, in a
table row, with the reason: *"there is not one in this repository."*

So the increment has three parts: make the knob, measure the thing the knob opens, and correct the sentence.

## The measurement

The knob is `columnar-cli dataset --row-group-rows N`. It writes the file with N rows per row group, the manifest
records both N and the number of row groups written, and the benchmark report carries the row-group count, so a sweep
says how many row groups the file it swept had. **The default does not move** — a million rows, six row groups — so
every figure published before this increment is still reproducible by the command that made it.

Three files were generated from the same seed, the same six million rows and the same schema. They differ in one
thing: the row-group size.

| | `data/lineitem.parquet` | `data/lineitem-rg12.parquet` | `data/lineitem-rg24.parquet` |
|---|---|---|---|
| rows a row group | 1,000,000 | 500,000 | 250,000 |
| **row groups** | **6** | **12** | **24** |
| bytes | 242,857,841 | 248,251,848 | 259,042,338 |

All three were swept and probed in **one session**, interleaved, three campaigns each. A comparison between two
campaigns taken days apart on a shared machine measures the machine as much as the engines; interleaving the files
is what makes the difference between them a difference between the files. The raw reports are in
`bench-results/scaling-run{1,2,3}.json`, `scaling-rg12-run*.json`, `scaling-rg24-run*.json` and
`row-groups-{6,12,24}-run*.json`, and every table below is derived from them by `python3 scripts/medians.py`.

### The cap

The sweep is `q6`, cold, at 1, 2, 4, 6, 8 and 14 threads, with **every incumbent at the same thread count** and
every answer cross-checked.

| threads | 6 groups: workers | 6 groups: engine | 12 groups: workers | 12 groups: engine | 24 groups: workers | 24 groups: engine |
|---|---|---|---|---|---|---|
| 1 | 1 | 19.7 ms | 1 | 24.8 ms | 1 | 34.6 ms |
| 2 | 2 | 9.9 ms | 2 | 12.8 ms | 2 | 17.3 ms |
| 4 | 4 | 7.1 ms | 4 | 6.6 ms | 4 | 9.3 ms |
| 6 | 6 | 4.0 ms | 6 | 5.5 ms | 6 | 6.5 ms |
| 8 | **6** | 4.2 ms | **8** | 5.1 ms | **8** | 5.8 ms |
| 14 | **6** | 4.0 ms | **12** | 4.3 ms | **14** | 5.4 ms |

**The worker count is `min(threads, row_groups)` at every point of every sweep.** The engine's speedup at fourteen
threads rises with it — **4.95×**, **5.76×**, **6.44×** — so the cap was real and is now lifted. The claim in §2e is
**confirmed**, and it is confirmed by three files rather than inferred from one.

### The confound, and the control

The row-group size changes the file as well as the parallelism, because the writer's one-megabyte dictionary page
limit is **per row group**. The incumbents are the control: they read the same bytes, so their numbers say whether
the *file* got slower or the *engine's read* did.

| at one thread | 6 groups | 12 groups | 24 groups |
|---|---|---|---|
| **the engine** | **19.7 ms** | **24.8 ms** | **34.6 ms** |
| duckdb | 32.0 ms | 29.0 ms | 30.0 ms |
| polars | 189.4 ms | 186.6 ms | 190.3 ms |
| datafusion | 35.2 ms | 36.6 ms | 39.0 ms |

**The engine's one-thread figure rises by 14.9 ms — 76% — and the three incumbents' figures do not move at all.**
The file did not get slower for anybody else. The engine's read got slower, by a cost the other readers do not pay.

### What the cost is made of

The same probe that took the materialised column apart in [ADR-0021](0021-the-materialised-column-and-a-rust-peer.md),
run on all three files:

| `l_extendedprice` | 6 groups | 12 groups | 24 groups |
|---|---|---|---|
| pages | 42 dictionary, 258 plain | 84 dictionary, 216 plain | 168 dictionary, 144 plain |
| dictionary-page bytes — **read by nobody** | 6,313,096 | 12,627,464 | **25,259,152** |
| Arrow's decode | 33.68 ms | 33.86 ms | 36.00 ms |
| **the materialised column's share of the query** | **35.65 ms** | **35.75 ms** | **36.91 ms** |

**The encoding is a real effect and it is a small one.** The smaller row groups do make the high-cardinality column
*more* dictionary-encoded — 168 dictionary pages against 42 — and they do leave four times as many dead dictionary
pages in the file. But the column's decode moves by 7% and its share of the query by 3%, while the predicates' cost
moves by 56%. The encoding is separated from the parallelism by being measured on its own.

**The per-row-group cost is the large one, and it is linear.** The engine's one-thread figure over the three files
is 19.7, 24.8 and 34.6 ms: **+5.1 ms** for six more row groups and **+9.8 ms** for twelve more, which is **0.85 ms
and 0.82 ms a row group**. The probe's `q6-revenue` ladder measures the same rate on the same files — 26.30, 30.26
and 40.90 ms for the predicates alone, **0.81 ms** a row group — and it is the probe's *three-predicate* query
against the sweep's *two-predicate* one, so the rate does not scale with the number of columns the query reads.

## What the measurement says

### The claim holds, and the sentence was backwards

**The engine's parallelism is capped by the file, not by the machine**, and the cap is `min(threads, row_groups)`
exactly. The sentence that said *larger* row groups would go further was wrong, and it is corrected in §2e: a file
with **more, smaller** row groups goes further.

### But the cap is not the only thing the row-group count does

**Unlocking the parallelism costs more than the parallelism is worth**, on this machine and for this query. The
engine's *best* figure is on the **six**-row-group file — 4.0 ms — and not on the file with the most parallelism:
4.3 ms on twelve row groups and 5.4 ms on twenty-four. A file with twenty-four row groups is more parallel and
**slower**.

That is not a paradox, it is two effects with opposite signs: the cap lifts — the speedup at fourteen
threads goes from **4.95×** to **6.44×** — and the per-row-group cost rises, at **0.8 ms** a group.
On this machine the second wins by a wide margin.

### The per-row-group cost is a defect, and this increment does not fix it

The engine's read path pays about **0.8 ms a row group** that DuckDB, Polars and DataFusion do not pay, reading the
same bytes. **A defect gets its own spec**, so this one is not fixed here. What is not measured is written down as
out of scope, with the reason: this increment is the *evidence* that the cost exists, and the mechanism is the next
spec's question. It is not known yet whether the cost is

- the parquet crate's **per-chunk setup** — the reader is opened per row group and the column chunks are set up
  with it,
- the **dictionary read per column chunk**, which the engine's pushdown does once per row group per column, or
- something the **engine** does itself per row group.

The measurement says the cost is **per row group** and does **not** scale with the number of columns the query
reads. It does not say which of the three it is.

## The decision

1. **The knob exists and the default does not move.** A million rows a row group, so every published figure stays
   reproducible by the command that made it.
2. **The claim in §2e stands, and its sentence is corrected.** The cap is the file's, `min(threads, row_groups)`,
   and it is now measured on three files.
3. **The per-row-group cost is published as a measured finding**, with the incumbents as the control, and it is
   **not** fixed in this increment. It is the next spec's question.
4. **The `DataFusion` peer joined the sweep.** It was already an incumbent in §2; a sweep with only two of the three
   would have been a comparison with a hole in it, and the peer is the control that says the *engine* got slower
   rather than the file.

## What was not done

- **The engine's read path was not changed.** See the decision above.
- **The generator's writer was not changed.** The dead dictionary pages are a fact about the file, not about the
  engine, and the file's writer is not this increment's business — the same call ADR-0021 made.
- **The older sweeps were not re-measured.** The `q6-revenue` sweep, the resident sweep and the load sweep were
  measured before the peer joined the harness, and they are published as they were measured rather than re-run for
  the sake of a column.
- **The mechanism of the per-row-group cost was not measured.** That is the next spec.
