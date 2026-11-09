# Spec 051 — The default moves to the byte budget

**Status**: planned
**Started**: 2026-11-07
**Predecessor**: [spec 050 — the batch size's price, paid](../050-the-batch-sizes-price-paid/spec.md)

## The problem

**Spec 049 priced the reader's batch size and decided the rule's shape: a byte budget.** A **counting global
allocator** measured the **peak** a read holds at three widths, and the batch's optimum turned out to be
**width-dependent**: the join's part is **3.717 ms** cheaper at a whole row group and faster in **24 of 24**
paired samples, the filter-range's scan does not move, and the wide read is **14.455 ms** *dearer* at a whole
row group and faster in **one** of twenty-four ([§3af](../../PERF.md#3af-the-batch-sizes-price)). **So the budget
is between 12.8 MiB and 20.7 MiB, and it is 16 MiB** -- bounded by the measurement from both sides.

**Spec 050 built the statement the budget divides by**, and the estimate now **charges the batch**
([§3ag](../../PERF.md#3ag-the-width-statement-paid)). **And the default still does not move**, so the engine
still divides a byte budget by a **row count**, and the knob is still named in rows.

**And the row count is the wrong thing to divide by, because what a batch holds scales with the query's
width.** The same million-row batch is **12.8 MiB** on a two-column query and **165.8 MiB** on a sixteen-column
one, and the engine's own limit cannot see it: **131,072** rows is the largest divisor of a million-row row
group under a limit, and that is **125,000** rows whatever the read fetches.

**And the default is what every published figure was measured with.** The headline table, the thread sweeps,
the ladder, the join's parts, the scan's parts, the sort's parts, the set's rows -- **all** of them were taken
with the row limit's **125,000**-row batch, and **a default that moves moves every one of them**.

## The requirements

### R1 — The default is a **byte budget**, and the batch is the read's own

- [x] The default's rule is a **byte budget of 16 MiB**, and the batch size is **the largest divisor of the row
      group's row count** whose bytes fit the budget -- the same rule [spec 050](
      ../050-the-batch-sizes-price-paid/spec.md) wrote down, and the same **width statement** that divides it.
- [x] **And the budget is divided by the columns the read fetches**, not by the file's width: a two-column read
      and a sixteen-column read of the same file get different batches, because they hold different amounts.
- [x] **And the knob still overrides it.** `with_reader_batch_rows` names a size in rows, and it is the
      measurement seam the probes sweep with. **It is not the default.**
- [x] **And the batch divides the row group's rows**, so that no row group ends with a partial batch.

### R2 — The estimate charges **the batch the read will use**

- [x] The budget's verdict is made with the **same** batch the read holds, and the two are computed from the
      **same** place -- the read's own fetched columns.
- [x] **And the estimate's figure moves with the default**, and is published old-and-new.

### R3 — Every figure that moves is published **old-and-new**, and **re-measured**

- [x] Every campaign that was taken with the engine's own batch size is **taken again**, on this host, three
      campaigns each -- and the figures are **re-measured, not recomputed**.
- [x] **And each moved figure is published with the one it replaces**, and the reason: **the default moved**.
- [x] **And the figures that do not move are not re-taken**: the batch sweeps, the width statement and the
      memory campaigns name their own batch sizes, and the eager path has its own. **Amended by A4**: the three
      campaigns that name their own batch sizes were taken again as well, because the engine's own batch is one
      of their points.

### R4 — The public surface: the **row limit is gone** and the **budget is published**

- [x] `MAX_READER_BATCH_ROWS` leaves the public surface -- **the row limit is not the rule any more** -- and
      `DEFAULT_READER_BATCH_BYTES` takes its place, so that an embedder can see the rule the engine reads.
- [x] **The semver baseline moves**, and the move is recorded. **The crates are unpublished**, so a break costs
      nothing -- and it is still recorded.

### R5 — The answer is published either way

- [x] A **new section** in `PERF.md`, the decision in an **ADR**, the status's next question rewritten, and the
      raw reports **committed**.
- [x] The **guard** checks the published figures **inside the section that publishes them**, and the old ones
      are checked against the **before** reports.

## The scope

- **In**: the reader's default batch size, the estimate's charge for it, and the figures that move.
- **Out**: **the reader's own decode** -- that is **spec 052**, and this increment does not touch it.
- **Out**: **the cold path's context** -- that is **spec 053**.
- **Out**: the crates.io publish. The crates stay unpublished while this work lands, so a break that the work
  implies costs nothing.

## The instrument

**The existing probes** -- the campaigns are **taken again**, not replaced by a new instrument: the compare set,
the curve, the scan's parts, the query's parts, the join's parts, the sort's parts, the operator's parts, the
key filter, the probe paths, the materialised column, the build side, the set, the many files, the write side,
the store's write, the engine's rows and the set's rows.

**And `crates/columnar-engine/examples/batch_width_probe.rs`** -- the width statement's own probe -- **also
publishes the default's batch size** for each of the three widths, because **the rule's output is a figure** and
a figure with no instrument behind it is a claim.

## The references

- [PERF.md §3af](../../PERF.md#3af-the-batch-sizes-price) -- the price, the rule, and the budget's number.
- [PERF.md §3ag](../../PERF.md#3ag-the-width-statement-paid) -- the statement the budget divides by.
- [ADR-0061](../../docs/adr/0061-the-batch-sizes-price.md) -- the decision this increment ships.
- [ADR-0062](../../docs/adr/0062-the-width-statement.md) -- the statement, and the estimate that charges it.
- [constitution.md](../../constitution.md), principle **IV -- Evidence Over Claims**: a number is a claim about a
  measurement, and a claim without its evidence is not published.
- [constitution.md](../../constitution.md), principle **VII -- Explicit Scope, No Silent Deferral**: the decode and
  the context are **named** as the next increments rather than left as omissions.

## The amendments

### A1 — The default moves, and **every figure that moves is re-measured**

**Why**: the default's move is the point of the increment, and the price of it is **every published figure the
reader's batch size touches**. **A moved figure is re-measured and published old-and-new** -- recomputing it
from the old reports would be a claim about a measurement that was never taken.

### A2 — The budget is **per batch**, and the memory budget is the whole read's

**Why**: the rule divides a budget by what a row costs, and what a row costs is a **batch's** cost. A parallel
read holds one batch per worker, and the estimate charges it that way -- so the **16 MiB** is what a worker
holds, and the **256 MiB** budget is what the whole read may hold. **The two are different numbers and they do
not contradict each other.**

### A3 — The rule divides by the **statement**, and the rate is in **eighths of a byte**

**Why**: the rule's per-row cost has to be an **integer** to divide the budget by, and a column's rows carry a
**validity bit** -- **an eighth of a byte** a row. A whole-byte rate rounds that bit up to a whole byte, which is
**eight times the truth**, and the rule then gives every shape **less** than the budget allows: the two
materialised columns got **500,000** rows where the rule published **1,000,000**, and **§3af's own bound** -- that
the narrow shapes get the whole row group -- did not hold.

**So the rate is carried in eighths of a byte**, the rule divides the budget by it, and the estimate -- which
works in totals and charges the bit -- reads **the same statement**. The two cannot disagree about what a row
costs, because they are the same number.

**And the consequence is published**: the **wide** shape's batch is **100,000** rows where the rule published
**62,500**, because the rule divides by the **statement** and the statement is **9% low** on the wide shape
([§3ag](../../PERF.md#3ag-the-width-statement-paid)). The narrow shapes get the whole row group, as published.

### A4 — The three campaigns that name their own batch sizes were **taken again**, because the engine's own batch is one of their points

**Why**: R3 says the figures that do not move are not re-taken, and the **width statement**, the **batch sweep**
and the **memory** campaigns name their own batch sizes, so their **measurements** do not depend on the default.
**But the engine's own batch is one of their points**, and it moved: the sweep publishes each shape's own batch
beside the others, the width probe publishes the rule's output, and the memory probe publishes the estimate at the
engine's own batch. **So their sets were taken again** -- three campaigns each, on this host -- and the old ones are
kept under their own names, `bench-results/<family>-spec050-run<n>.json`.

**And the increment found the stale binary while doing it**: the first landing of those three sets carried a
report without the shape's own name, because the campaign had run a **release binary built before the probe
changed**. **The three campaigns were taken again with the binary the tree builds**, and the reports carry the
shape's name, the statement's bytes and the rule's output per worker count.

### A5 — The campaign's own sets were **taken again** where it dropped a point or named the wrong commit

**Why**: the campaign is the house's method, and it is **checked** by the guard and by the figures it publishes. Two
of its sets were wrong in ways the guard caught, and both are fixed by **taking the set again**, not by editing the
document:

- **The thread sweeps lost their six-thread point.** The published table has a row for it, and the campaign's
  instrument's default thread list does not name six, so the new set had five points where the published set had six.
  **The four sweep families were taken again with the six points**, both codes, three campaigns each, interleaved.
- **The old code's join-parts set named the wrong commit.** The campaign ran the old binary from the new tree's
  directory, so the report recorded the *new* tree's revision beside the old code's measurement. **The set was
  taken again** with the old tree as the working directory, so the report names the code it measured.

**And the sweep's own "before" was attributed by measurement.** The sweep is the only family whose "before" is the
set this file replaces, so **the same instrument was run against both codes**, three campaigns each, interleaved, on
this host: the code's share of the sweep is under **0.2 ms**, and the set this file replaces reads **7 ms** more
slowly than either. **So the movement between the two published sets is the host's**, and it is published in
[§3af](../../PERF.md#3af-the-batch-sizes-price) with the reports that say so.
