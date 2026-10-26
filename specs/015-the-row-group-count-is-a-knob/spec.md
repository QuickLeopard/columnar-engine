# Spec 015 — The row-group count is a knob

**Status**: done
**Started**: 2026-10-26
**Finished**: 2026-10-26
**ADR**: [ADR-0022](../../docs/adr/0022-the-parallelism-above-the-row-group-count.md)
**Predecessor**: [014-the-materialised-column-and-a-rust-peer](../014-the-materialised-column-and-a-rust-peer/spec.md)

## The problem

[`PERF.md` §2e](../PERF.md#2e-the-thread-count-sweep) publishes a curve that **flattens at six threads**, and the
explanation is the file's:

> **The engine's parallelism is capped by the file, not by the machine.** The read splits the file's **row groups**
> across the workers, and this file has **six** of them — so six workers is the most that can run, and the curve
> is flat from six threads on. … A file written with larger row groups would go further, and that is a property of
> the file rather than of the engine.

Two things are wrong with that paragraph, and one of them is a published error.

**The error.** *Larger* row groups is backwards. The read splits the **row groups** across the workers, so the
number of workers is bounded by the **number** of row groups: `min(threads, row_groups)`. A file written with
**smaller** row groups has **more** of them and goes further; a file written with larger ones has fewer and goes
less far. The sentence says the opposite of the truth, and it is the kind of sentence a reader copies.

**The gap.** The row-group count is not a knob. The generator hard-codes a million rows per row group, so every
file this repository has ever produced has **six** row groups per six million rows, and the claim *"the
parallelism above the file's row-group count"* has never been measured — [`PERF.md` §5](../PERF.md#5-not-measured)
says so, in a table row, with the reason: *"there is not one in this repository"*.

So the increment has three parts: make the knob, measure the thing the knob opens, and correct the sentence.

## The requirements

### R1 — The knob exists, and the default does not move

- [x] The generator takes the **row-group size**, and the manifest records it and the number of row groups
      actually written.
- [x] The command line exposes it, and its default is the current value — a million rows — so every published
      figure stays reproducible by the command that made it.
- [x] The file's **row-group count** is in the benchmark report, so a sweep says how many row groups the file
      it swept had.

### R2 — The measurement, with the incumbents on the same file

- [x] A file with **more row groups than the machine has cores** is generated: six million rows in 250,000-row
      groups is **24 row groups**, on a fourteen-core machine.
- [x] A **third** file, in 500,000-row groups — **12 row groups** — is generated beside the other two, so the
      row-group count has three points and the effect has a **rate** rather than only a difference.
- [x] The engine and **every incumbent** are swept over the thread count on those files, at the same thread
      counts, in the same harness, with the answers cross-checked — the same symmetry rule as every other
      sweep.
- [x] The report records the **workers that ran** against the **threads asked for**, so the cap is visible in the
      table and not only in the prose.

### R3 — The confound, measured rather than assumed

- [x] The row-group size changes the **file's encoding**, because the writer's one-megabyte dictionary page limit
      is **per row group**: a smaller group has a smaller dictionary, and the high-cardinality column's
      dictionary may fit where it did not before. The **same probe** from spec 012 and 014 is run on all three
      files and the three files' shapes are published side by side.
- [x] The sweep's two effects — more parallelism, and a different file — are **separated** before either is
      attributed: the parallelism by the shape of the curve, the encoding by the file's bytes and the probe's
      decomposition.
- [x] The incumbents are the **control**: they read the same bytes, so their numbers say whether the file got
      slower or the engine's read did.

### R4 — The publication

- [x] Every figure is the median of **three full campaigns**, with the spread, the raw reports committed, and
      the table derived by `scripts/medians.py`.
- [x] `scripts/check-published-figures.py` guards the figures that are quoted.
- [x] [`PERF.md` §5](../PERF.md#5-not-measured)'s row moves from *"not measured"* to *"measured"*, and the
      **wrong sentence in §2e** is corrected.
- [x] ADR-0022 records what was measured, whichever way it went.

## The non-goals

- **The engine's read path does not change.** This increment is evidence, like every other one since spec 012.
- **The generator's per-group shape does not change.** The row-group *size* is the knob; the schema, the
  compression, the statistics and the dictionary policy stay as they are, so the two files differ in the one
  thing the increment is about — and anything else that difference causes is **measured and published** rather
  than designed away.
- **No new query.** The sweep is the same `q6` the published sweep uses, so the two curves are the same
  measurement on two files.
- **The default file does not change.** `data/lineitem.parquet` is still six million rows in a million-row
  groups; the new file is a second file, for a second measurement.

## What the measurement found

- **The cap is `min(threads, row_groups)`, exactly**, at every point of every sweep: six workers at
  fourteen threads on the six-row-group file, twelve on the twelve-row-group one, fourteen on the
  twenty-four-row-group one. The engine's speedup at fourteen threads rises with it — 4.95×, 5.76×,
  6.44× — so the claim §2e made is confirmed by three files rather than inferred from one.
- **The more parallel file is the slower one.** The engine's one-thread figure rises from 19.7 ms to
  34.6 ms across the three files, and the three incumbents' figures do not move at all. The engine
  pays about **0.8 ms a row group** that DuckDB, Polars and DataFusion do not pay, reading the same
  bytes.
- **The encoding is a small effect and the per-row-group cost is the large one**: the materialised
  column's decode moves by 7% and its share of the query by 3%, while the predicates' cost moves by
  56%.
- **The cost is a defect, and it gets its own spec.** The mechanism is not measured here: it is known
  to be per row group and not per column, and it is not known whether it is the parquet crate's
  per-chunk setup, the dictionary read per column chunk, or something the engine does itself.

See [ADR-0022](../../docs/adr/0022-the-parallelism-above-the-row-group-count.md) and
[`PERF.md` §3j](../PERF.md#3j-the-row-group-count-and-what-it-costs).
