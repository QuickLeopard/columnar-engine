# ADR-0068: The index's shape, and its price

**Status**: accepted
**Date**: 2026-11-18
**Related**: [ADR-0066](0066-the-databases-own-price.md), [spec 057](../../specs/057-the-index/spec.md)

## Context

**The rule is the database's own, and it is written down**: *a point lookup that a scan already answers more cheaply is
not given an index, and the database says so*
([ADR-0066](0066-the-databases-own-price.md)). **And the figure that decided the shape is published**: the point lookup
costs **43.319 ms** as a scan in the engine's own read, the same lookup over **one file** — the floor an index would reach
— costs **2.657 ms**, so the scan is **17.20×** more expensive and **the most an index could save is 40.828 ms**.

**And what is missing is the index's own price.** ADR-0066 says it in its own words: *"the index's own cost — its build,
its storage and its lookup — is unmeasured, because there is no index to measure"*. **So the shape is decided and the price
is not** — and **the price is the only thing that decides whether the index is worth having at all.**

**And the price that was missing is the price that decides the encoding.** ADR-0066 chose *"a map from the key to the
file that holds it"*, and it chose it for the **granularity** — the file, because that is where the floor was measured —
**and not for the encoding**. **And there are three candidates, and they are not the same size**: the **file's own
statistics**, which the store already writes (`EnabledStatistics::Chunk`) and which cost nothing to keep; the **per-file
key range the database keeps**, two numbers a file and a write's worth of work; and **the per-key map**, an entry a key.

**And the map that is free is the map that narrows least — on this data.** The published lineitem file's rows are in the
file's order and not in the key's order, so **every one of the table's sixteen files holds a range that spans nearly the
whole key space**, and **a range that spans the key space excludes nothing**.

## The measurement

**The instrument is `crates/columnar-db/examples/index_probe.rs`**, and the table is the one the database's own probe
builds: **a million rows of the published lineitem file**, on the store's grid, so **sixteen files of 65,536 rows**. The
probe measures **the key's order**, **each file's own range** — the file's statistics, which is what a range map would
use — **the distinct keys**, and **the number of files each of the maps would name**, over sixty-four sampled keys.

**And the answer refuses an arm.** The key falls **499,701** times over the table's million rows, and the range map's
reach runs from **13** to **5,999,999** — so **it names all sixteen files, for every one of the sampled keys**: the mean
is **16.000**, and the smallest and the largest are **16**. **And the per-key map names 1.156 files on average**, **1** at
the smallest and **2** at the largest.

## Decision

**A1 — The index is the per-key map, and ADR-0066's shape is confirmed by a measurement.**

**Why**: the map that narrows is the map that is built. **And the per-key map narrows**: **1.156** files on average
against the range map's **16.000**, so it reads **a fourteenth** of the files a scan reads. **And the shape is the one
ADR-0066 decided** — a map from the key to the file that holds it, many-to-one, over the grid — **so the sentence that
decided it stands, and it stands on a figure and not on an assumption.**

**And the map's own size is published with it**: **921,129** distinct keys over a million rows, and **78,871** rows that
repeat a key — so **the index holds an entry a key**, and a key's entry can name **two** files.

**A2 — The range map is refused, and the refusal is on this data and not on the shape.**

**Why**: a map that names every file saves nothing, and **a map that saves nothing is not built**. **And the reason is the
data's order**: the published lineitem file's rows are in the file's order and not in the key's order, so **a range cannot
exclude a file on this data**.

**And that is published as a condition, because it is one**: **on ordered data the range map would narrow, and it would be
free** — the file's own statistics are already written, and the engine reads them when it opens the file. **So the range
map is not refused as a shape, and it is refused on this data, and the figure that says so — the key's fall, the reach,
and the sixteen files — is published beside the refusal.** **And the two range maps are one shape**: the file's own
statistics and the database's own range name the same files, because the file's statistics *are* that range.

**A3 — The granularity is the file, and the bound is published.**

**Why**: the engine reads whole files and has no row-group skipping, so **the floor an index can reach is the engine's own
read of one file** — **2.657 ms**, measured. **So the index names a file and not a row**, and **that bound is published with
the answer**, so that a reader who expects a row-level index knows exactly what was and was not built.

## Consequences

- **The shape is settled, and the price is the next measurement.** The index's **write cost**, its **storage**, its **peak
  memory**, the **rebuild's cost at an open**, and the **lookup's own time** are measured next, beside the scan and the
  one-file floor, back to back, on the same table, at the same moment. **And the break-even — the number of reads after
  which the index has paid for itself — is the figure the rule turns on**, and it is published whether it is small or
  large.
- **And the index's memory grows with the table's keys**, which is **the same shape of defect as the compaction's
  ceiling** — where the database's own memory was the table's size — **so the row count at which the index's own memory
  exceeds the engine's 256 MiB budget is measured**, if it does.
- **And the engine's surface does not grow**: the index is the database's own, and the file's statistics are already
  reachable through the engine's own `ParquetSource`.
- **And the store's surface does not grow**: the store already writes the statistics, and the grid is the grid. **And the
  four crates publish at 0.1.0**, so a change to a published surface is a break in every embedder's build.
- **And the tombstone tax's shape is still open.** An indexed lookup reads one file, and that file's retired versions still
  cost a predicate each — **the index does not change the tax**.
