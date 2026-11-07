# ADR-0060: The probe, measured twice

**Status**: accepted
**Date**: 2026-11-07
**Reviewers**: TBD

## Context

Spec 046 published the engine's join part — the `probe_elapsed` the reader charges the join's filter, over
**6,000,000** rows, in **48** batches — at **14.625 ms**
([§3ac](../../PERF.md#3ac-the-join-orders-query-on-both-sides-taken-apart)). Spec 047's ladder measured the
engine's own probe over the **same** six million rows, against the **same** table, at **7.622 ms**
([§3ad](../../PERF.md#3ad-the-build-side-s-keys-as-a-filter)).

**The same `probe_rows`, the same table, the same rows, and 7 ms apart.** And the two numbers came from two
different campaigns, on a shared host, days apart. **So the first question was not "what causes the gap" —
it was "is there a gap at all".** Either the gap is real, and several milliseconds of the engine's join is
something the engine does not know it is paying; or it is not real, and spec 046's published **+6.898 ms**
is wrong and has to be corrected where it was published.

**And the first thing to turn was named.** The reader's batch size is a constant: `MAX_READER_BATCH_ROWS` is
131,072, and the batch a row group is read in is the **largest divisor** of its row count under that limit —
**125,000** rows for the published file, so **48** batches over six million rows. `BatchPlan::Fixed` exists —
the house calls it "the measurement seam" — but nothing above it could turn it: the query's read asks for
`Derived(MAX_READER_BATCH_ROWS)` and gets what it gets. **A knob that cannot be turned cannot be measured.**

## Decision

1. **The reader's batch size is a knob, and it lives on the thing that reads.** The knob is
   `ParquetSource::with_reader_batch_rows`, it names a size **in rows**, and `None` is the engine's own. It
   is on `ParquetSource` and not on `ExecOptions` because `ExecOptions` is a struct of **public fields** — a
   new field on it breaks every external construction of it — while `ParquetSource` has **no public fields**
   and is built by `open()`, so a field on it is not a break and a method on it is an addition. **The knob
   breaks nothing, and the semver baseline does not move.**

2. **The default does not move.** `MAX_READER_BATCH_ROWS` stays the default, so every published figure
   stays reproducible by the command that made it. **The knob is a measurement seam, and a seam that moved
   the default would not be a seam.**

3. **The two paths are measured in one session, back to back, with the order alternated.** The **ladder's
   path** — the probe over a materialised column, outside the reader — and the **query's path** — the probe
   inside the reader, between the reader's own work — are **the same `probe_rows`**, over the same rows,
   against the same table. **The difference is the paired one**, not the difference of two medians from two
   campaigns: a difference of two medians would let the machine's drift land on both sides.

4. **The batch size is swept, and the points are rows.** The sweep's points are **62,500**, **125,000** (the
   engine's own), **250,000** and **1,000,000**, and every one of them divides a row group's row count, so
   no row group ends with a partial batch and the sweep is the engine's probe and nothing else. The points
   run in a **rotation of four**, twice over: every point takes every position exactly twice, and every
   pair of points runs in one order twice and the other order twice.

5. **The rows are checked before anything is timed.** Every path's rows are compared with the engine's own
   probe's rows, **row for row, in order**, in every iteration, and the comparison is asserted. A path that
   returned different rows would be a different query, and its time would not be the engine's. **The answer
   does not move**: the knob changes when the rows arrive, not which rows arrive.

6. **The same probe is measured three times in the ladder's path**, because the probe's cost is the build
   table's residency and the instrument has to say so rather than assume it: **cold**, right after the
   materialisation; **resident**, at once, with nothing in between; and once more **after the reader's
   work**. The first two differ by the table's residency, and the last two by what the reader's work costs
   the table.

## Consequences

**What became knowable. The gap is real, and it is the reader's context, not the probe.**

The engine's own probe costs **14.675 ms** inside the reader and **10.033 ms** outside it — **+4.273 ms**,
positive in **24 of 24** paired samples, and **every one of the twenty-four is positive** (the least
**+1.884 ms**, the most **+9.006 ms**). The same probe with the build table resident costs **9.290 ms**, so
**the table's residency is worth 0.657 ms**.

**And the reader's own work does not make the table colder.** The same probe, after the query's path and the
sweep have run, costs **9.642 ms** — **0.357 ms** from the resident price, against a spread of **−1.109 ms**
to **+4.427 ms**. **And the reader's own work is the same at every batch size**: the scan reads **68.380 ms**
to **69.915 ms** across all four of them. **So the batch size does not make the reader do more or less
work, and it does not flush the build table.**

**What is left is the number of times the probe is run.** The engine's own batch size runs the probe
**48** times over six million rows; a batch of a row group runs it **six**. And the cost falls with the
number of runs: **14.944 ms** at the engine's own batch size, **9.549 ms** at a row group's worth — and the
sweep's difference against the engine's own is **−4.926 ms**, faster in **24 of 24** paired samples. **The
curve is monotone**: **62,500** is **+0.397 ms**, a wash — 10 of 24 faster — and **250,000** is
**−1.438 ms**, faster in **19 of 24**.

**And the value column's decode is more than half of the reader's charge.** With the key column alone the
reader's path costs **12.306 ms** against the ladder's **10.002 ms** and the gap over it is **+1.834 ms**;
with the key and the value the gap is **+4.273 ms**. **The decode of the value column — the reader's own
work between two probes — is 2.439 ms of the charge, and the batch size is what sets it.** The key-alone
sweep says the same thing, more weakly: **+0.298 ms** at 62,500, **−0.190 ms** at 250,000 and **−2.054 ms**
at a row group's worth, faster in **20 of 24** — so the decode is **2.872 ms** of the lever.

**And the query pays it.** The engine's join part, taken apart at every batch size: **14.759 ms** at 62,500,
**15.823 ms** at the engine's own **125,000**, **12.370 ms** at 250,000 and **9.641 ms** at **1,000,000** —
**6.182 ms** less than the engine's own, with the scan flat. The **paired** difference against `DuckDB`'s
join falls from **+8.083 ms** to **+1.898 ms**, and the engine is slower in **18 of 18** iterations at every
batch size. **So most of the engine's disadvantage on the join part — 6.182 ms of 8.083 ms — is the
reader's context, and the reader's context is a knob.**

**And the knob is a knob: the default does not move.** Nothing in the engine's behaviour changes. **The
next increment** measures what a batch of a million rows costs in memory — the reader holds the key column
and the value column of a batch in memory at once — and decides whether the default moves.

**What is not known, and is the next question.** **Why a shorter run of the probe costs more per row**, when
the table is equally cold either way and the reader's work is the same. The curve is monotone in the batch
size and flattens at one row group — **the number of batches cannot go below the number of row groups** —
so the effect is the number of runs of the probe, and what a run costs beyond the lookup itself is not yet
named. **That is the open question this increment leaves behind.**

**And the probe is memory-bound, which is why the load average does not order the campaigns.** The probe
side's keys fall **2,999,506** times in **6,000,000** rows, so each of the six million lookups lands
somewhere else in the build table, and the table is **5,999,995** bytes — bigger than any cache the machine
has. **What moves the probe is the host's memory traffic, and a load average does not measure that.**

**And the published figure stands, and its spread is published with it.** Spec 046's join part reads
**14.625 ms** on campaigns taken at a load average of **8.54** to **8.77**; this increment's re-measurement
of the same part, at the engine's own batch size, reads **15.823 ms** at a load of **6.98** to **7.94** —
**1.198 ms** apart. **The engine did not change** — the knob's default is the engine's own — **so the
difference is the host's state, and the figure is not reproducible on this host to better than about a
millisecond.** The **sign** is reproducible: the engine's join is slower in **18 of 18** iterations here, as
it is in spec 046's eighteen. **§3ac is not corrected, and this section says so where §3ac's figure is
read.**

**And the instrument's own repeatability is published.** The engine's own batch size reads **14.944 ms** in
the sweep's slot and **14.675 ms** in the paired slot — the same measurement, two positions, **0.269 ms**
apart. **That is the yardstick the −4.926 ms is read against.**
