# Spec 048 — The probe, twice

**Status**: done
**Started**: 2026-11-07
**Finished**: 2026-11-07
**Predecessor**: [spec 047 — the build side's keys as a filter](../047-the-build-side-s-keys-as-a-filter/spec.md)

**CI green on `378cf06`** -- run 42334500041, sixteen of sixteen jobs.

## The problem

**The engine's own probe reads two different numbers, and the engine does not know which one it pays.**

- **14.625 ms** is the join part spec 046 published: the `probe_elapsed` the reader charges the join's filter,
  over **6,000,000** rows, in **48** batches ([§3ac](../../PERF.md#3ac-the-join-orders-query-on-both-sides-taken-apart)).
- **7.622 ms** is the engine's own probe measured by spec 047's ladder, over the **same 6,000,000** rows, against
  the **same** table the engine built ([§3ad](../../PERF.md#3ad-the-build-side-s-keys-as-a-filter)).

**The same `probe_rows`, the same table, the same rows, and 7 ms apart.** And 7 ms is **more than the whole gap
to `DuckDB`**: the engine's cold `join-orders` is **96.241 ms** against `DuckDB`'s **92.735 ms**, a difference of
**+3.290 ms**.

**And the two numbers were measured in two different campaigns**, on a shared host, days apart. **So the first
question is not "what causes the gap" — it is "is there a gap at all".** Either:

- **the gap is real**, and six milliseconds of the engine's join is something the engine does not know it is
  paying — in which case closing it puts the engine **ahead** of `DuckDB` on the published query for the first
  time; or
- **the gap is not real** — two campaigns, one host, two answers — in which case **spec 046's published
  +6.898 ms join gap is wrong**, and it has to be corrected where it was published.

**Neither answer is a reason to build anything yet.** The measurement decides.

### What the two paths are

**Path A — the ladder's path.** The probe side's column is materialised, and the engine's own `probe_rows` runs
over it, in one pass, over the engine's own table.

**Path B — the query's path.** The reader decodes the file in batches, and the join's filter probes each batch's
key column **before the copy**, so the rows that do not match are never copied
([§3o](../../PERF.md#3o-the-joins-stage-taken-apart)). The reader's clock covers the probe and nothing else:
`dataset.rs` times exactly `filter.keep`, and the join's stage is that clock plus the table's construction.

**The two paths are the same function.** The difference is what happens around it: in path B the reader decodes
the value column and copies the matched rows between the probes, and the probes arrive in batches.

### Why the batch size is the first thing to turn

**The reader's batch size is a constant**: `MAX_READER_BATCH_ROWS` is 131,072, and the batch a row group is read
in is the **largest divisor** of its row count under that limit — **125,000** rows for this file, so **48**
batches over six million rows.

**And the batch size is not a knob.** `BatchPlan::Fixed` exists — the house calls it "the measurement seam" — but
nothing above it can turn it: the query's read asks for `Derived(MAX_READER_BATCH_ROWS)` and gets what it gets.
So the first thing this increment does is **make the batch size a knob**, and then measure what it does to the
probe.

**And the knob is a measurement seam, so it does not move the engine's default.** The default stays
`MAX_READER_BATCH_ROWS`, and every published figure stays reproducible by the command that made it.

## The requirements

### R1 — The reader's batch size is a knob, and the default does not move

- [x] The knob is the **batch size in rows**, and it lives on the thing that reads: `ParquetSource`.
- [x] **`None` is the engine's own**: the largest divisor of the row groups' row count under `MAX_READER_BATCH_ROWS`,
  so that no row group ends with a partial batch.
- [x] **A named size is exactly that size**, and a size that does not divide a row group's row count leaves a
  **partial batch** at the end of every row group. The house already knows what a partial batch costs — the
  parquet crate's reader pays about **0.85 ms** for it, once per row group — and the knob's doc says so.
- [x] The knob reaches the **query**, not only the reader: the command line exposes it on the command that runs a
  query, so a reader can turn it and see the query's own parts move.
- [x] **The default does not move**, so every published figure stays reproducible.

### R2 — The two paths, measured **in one session**, back to back

- [x] The two paths are measured **in the same iteration**, **back to back**, with the **order alternated**, over the
  **same rows** and the **same table**.
- [x] **The difference is the paired one**, not the difference of two medians from two campaigns.
- [x] The candidates are **the engine's own `probe_rows`**, both of them — the ladder's path and the reader's path
  are the same function, and the instrument calls it twice.

### R3 — The batch size is swept, and the sweep's points are named

- [x] The reader's path is measured at **several batch sizes**, including the engine's own, and the points are **rows**,
  not "small" and "large".
- [x] Every point is the **same rows and the same table**, so the sweep is the engine's probe and nothing else.

### R4 — The rows are the rows

- [x] Every path's rows are compared with the engine's own probe's rows, **row for row, in order**, in every
  iteration.
- [x] **The query's answer does not move**: the knob changes when the rows arrive, not which rows arrive.

### R5 — The answer is published either way

- [x] **A real gap** is published with its parts and its mechanism, and the open question is rewritten.
- [x] **No gap** is published as the correction it is: spec 046's published join figure is corrected **where it was
  published**, and the correction is recorded.
- [x] The figures are derived from the committed reports and checked by the guard **inside the section that publishes
  them**.

## What the measurement found

**The gap is real, and it is the reader's context — and the reader's batch size is what sets it.**

The engine's own probe costs **14.675 ms** inside the reader and **10.033 ms** outside it, measured **in one
session, back to back, with the order alternated**: **+4.273 ms**, positive in **24 of 24** paired samples.
**The reader's batch size moves it**: a batch of a row group — **1,000,000** rows — is **−4.926 ms**
against the engine's own **125,000**, faster in **24 of 24** paired samples, and the curve is monotone —
**−1.438 ms** at **250,000** and **+0.397 ms** at **62,500**. **And the query pays it**: the engine's join
part is **15.823 ms** at the engine's own batch size and **9.641 ms** at a row group's worth, and the paired
difference against `DuckDB`'s join falls from **+8.083 ms** to **+1.898 ms** — **with the scan flat**, so the
reader does the same work at every batch size.

**And the knob is a knob**: the default does not move, and no engine behaviour changed. **What is not yet
explained** — and is the next question — is why a shorter run of the probe costs more per row, when the
build table is equally cold either way and the reader's work is the same. See
[ADR-0060](../../docs/adr/0060-the-probe-measured-twice.md) and `PERF.md` §3ae.

## The scope

- **In**: the reader's batch size, the join's probe, and the two paths it runs in.
- **Out**: the **hashed** table. The published file's join uses the direct table.
- **Out**: `DuckDB`. Its join is measured as `DuckDB` measures it.
- **Out**: any change to the engine's **default** batch size. **The knob is a knob**: it is set by the caller, and
  the default does not move. If the measurement says a different default is worth having, that is the next
  increment.
- **Out**: any change to the plan's shape. **The surface does not move**: a method on `ParquetSource` and a flag on
  the command line are additions, and nothing is removed or renamed.

## The instrument

**`crates/columnar-engine/examples/probe_paths_probe.rs`** — the increment's own probe, beside spec 047's ladder.

**And `crates/columnar-bench/src/join_parts.rs`**, spec 046's probe, for the query's own parts — because the
question is not only "what does the probe cost" but "what does the query pay".

## The references

- [PERF.md §3ac](../../PERF.md#3ac-the-join-orders-query-on-both-sides-taken-apart) — the parts, on both sides,
  and the join part this increment re-measures.
- [PERF.md §3ad](../../PERF.md#3ad-the-build-side-s-keys-as-a-filter) — the ladder, the control, and the **7.622
  ms** the engine's own probe reads over the same rows.
- [PERF.md §3o](../../PERF.md#3o-the-joins-stage-taken-apart) — the join's stage, and the scan that feeds the join.
- [ADR-0060](../../docs/adr/0060-the-probe-measured-twice.md) — the decision.
- [constitution.md](../../constitution.md), principle **IV — Evidence Over Claims**: a number is a claim about a
  measurement, and a claim without its evidence is not published.
- [constitution.md](../../constitution.md), principle **II — A Negative Result Is a Result**: a correction is a
  result, and it is published where the wrong figure was published.

## The amendments

### A1 — The knob is on `ParquetSource`, not on `ExecOptions`

**Why**: the knob has to reach the **query**, and the query is `execute_on(plan, &source, &options)` — the source
is in the call. And `ExecOptions` is a struct of **public fields**, so a new field on it **breaks** every external
construction of it: `cargo-semver-checks` calls that a major break, and the house's rule is that a breaking
change on the public surface is made deliberately, while the crates are unpublished, and recorded in
`scripts/semver-baseline.txt`.

**`ParquetSource` has no public fields** — it is built by `open()` — so a field on it is not a break, and a
method on it is an addition. **The knob therefore breaks nothing**, and the baseline does not move.

### A2 — The knob is the batch size, and not a limit

**Why**: the house's vocabulary has a "limit" (`batch_rows_under`) and a "measurement seam" (`BatchPlan::Fixed`),
and both are derived numbers. A knob a reader turns wants to say **125,000 rows** and get 125,000 rows — and the
sweep's points have to be *rows*, not "small" and "large". **The partial batch a named size may leave is the
caller's choice, and the knob's doc says what it costs** — the house already measured it.
