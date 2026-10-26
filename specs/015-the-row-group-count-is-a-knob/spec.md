# Spec 015 — The row-group count is a knob

**Status**: in progress
**Started**: 2026-10-26
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

- [ ] The generator takes the **row-group size**, and the manifest records it and the number of row groups
      actually written.
- [ ] The command line exposes it, and its default is the current value — a million rows — so every published
      figure stays reproducible by the command that made it.
- [ ] The file's **row-group count** is in the benchmark report, so a sweep says how many row groups the file
      it swept had.

### R2 — The measurement, with the incumbents on the same file

- [ ] A file with **more row groups than the machine has cores** is generated: six million rows in 250,000-row
      groups is **24 row groups**, on a fourteen-core machine.
- [ ] A **third** file, in 500,000-row groups — **12 row groups** — is generated beside the other two, so the
      row-group count has three points and the effect has a **rate** rather than only a difference.
- [ ] The engine and **every incumbent** are swept over the thread count on those files, at the same thread
      counts, in the same harness, with the answers cross-checked — the same symmetry rule as every other
      sweep.
- [ ] The report records the **workers that ran** against the **threads asked for**, so the cap is visible in the
      table and not only in the prose.

### R3 — The confound, measured rather than assumed

- [ ] The row-group size changes the **file's encoding**, because the writer's one-megabyte dictionary page limit
      is **per row group**: a smaller group has a smaller dictionary, and the high-cardinality column's
      dictionary may fit where it did not before. The **same probe** from spec 012 and 014 is run on all three
      files and the three files' shapes are published side by side.
- [ ] The sweep's two effects — more parallelism, and a different file — are **separated** before either is
      attributed: the parallelism by the shape of the curve, the encoding by the file's bytes and the probe's
      decomposition.
- [ ] The incumbents are the **control**: they read the same bytes, so their numbers say whether the file got
      slower or the engine's read did.

### R4 — The publication

- [ ] Every figure is the median of **three full campaigns**, with the spread, the raw reports committed, and
      the table derived by `scripts/medians.py`.
- [ ] `scripts/check-published-figures.py` guards the figures that are quoted.
- [ ] [`PERF.md` §5](../PERF.md#5-not-measured)'s row moves from *"not measured"* to *"measured"*, and the
      **wrong sentence in §2e** is corrected.
- [ ] ADR-0022 records what was measured, whichever way it went.

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
