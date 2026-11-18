# ADR-0069: The index is used when it can narrow, and the rule is applied

**Status**: accepted
**Date**: 2026-11-18
**Related**: [ADR-0066](0066-the-databases-own-price.md), [ADR-0067](0067-the-compaction-reads-a-file-at-a-time.md), [ADR-0068](0068-the-indexs-shape-and-its-price.md), [spec 057](../../specs/057-the-index/spec.md)

## Context

**The shape was measured, and the price was the figure that was missing.** ADR-0068 settled the shape — **a map from the key
to the files that hold it**, over the grid — and it left the price to be measured. **And the price is measured now**, on every
axis the rule turns on ([PERF.md §3an](../../PERF.md#3an-the-indexs-price-measured-on-every-axis) and
[§3ao](../../PERF.md#3ao-the-indexs-memory-and-the-row-count-at-which-it-passes-the-budget)).

**And the price says the index pays for itself — on a table of many files.** The index's build is **125.889 ms**, the lookup it
replaces is **45.689 ms** warm, and the saving is **42.898 ms** a lookup — **so the break-even is 2.935 lookups**, and a
snapshot that serves three lookups has had its money back. **And the write path pays nothing**: **27.333 ms** with no index
alive and **29.122 ms** with one, **and the difference is inside the spread**, because the index is the snapshot's and the
write path never reads it. **And the compaction's cost does not move** — **285.976 ms** without the index and **298.622 ms**
with it, **and the difference is inside the spread**, because the compaction never reads the index either.

**And the price says the index pays for nothing at all — on a compacted table.** The compaction rewrites the table's live rows
into **one file** (ADR-0067), **and the index names files** — so on a compacted table the index names the whole table, narrows
nothing, and **its build is the whole of the cost**. **And that was measured, and it was the defect**: the compacted table's
first lookup cost **181.172 ms** — the index's build, **135.009 ms**, and the read — **for a table the index could not narrow
at all.**

**And the memory is the price's second axis, and it is not bounded by anything.** The index holds **132,237,778** bytes over a
million rows — **126.1 MiB**, **143.6** bytes an entry — **and the row count at which it passes the engine's own 256 MiB
budget is between 2,379,393 and 2,381,835 rows**, which is **lower than the compaction's old ceiling** — **5,375,000** rows
compacted, **6,000,000** refused.

## Decision

**A1 — The index is built when the table has more than one file, and the lookup is the scan when it has one.**

**Why**: the index's granularity is the file, **and a table of one file cannot be narrowed** — the scan reads the file the
index would name, so the index would pay **135.009 ms** and save nothing. **And the rule says so in its own words**: *a point
lookup that a scan already answers more cheaply is not given an index, and the database says so.* **And the database says so by
not building one** — the lookup reads the table's file and answers with the same rows it always answered with. **And the
compaction is what makes a table one file**, so **this is the shape a compacted table has**, **and the compaction is the
database's own routine work** — so this is not an edge case and it is the common one.

**A2 — The write path pays nothing, and that is measured rather than argued.**

**Why**: the index is the snapshot's (ADR-0068), so **a write cannot touch it** — and the measurement says the same: the write
costs **27.333 ms** with no index alive and **29.122 ms** with one, **and the difference is inside the spread**. **And the
total work is the same either way**, because the keys have to be read once whichever side reads them.

**A3 — The index's memory is published, and the refusal is a later increment's question.**

**Why**: the index's memory is **126.1 MiB** over a million rows and it grows with the table's keys, **and above 2,381,835 rows
it passes the budget the engine holds a whole read to**. **And the database does not refuse it** — **so that is published as an
open question**, with the figure that asks it, **and not as a rule invented here**: a refusal needs an estimate of the index's
memory, and **the estimate is a second thing to get wrong** — and the cheaper encoding (§A4) may change the figure it is made
from.

**A4 — The index's build is expensive, and the cheaper encodings are not measured.**

**Why**: the build costs **125.889 ms**, **and the scan it replaces costs 45.689 ms** — **so the index costs more to build than
one scan costs to run**, and it needs **2.935** lookups to pay that back. **And the map holds a `Vec` a key** — **921,129**
little allocations — **so the shape of the cost is the map's own, and the cheaper encodings — a sorted vector, a file id with a
fallback, a per-file key set — are not measured.** **So that is published as an open question**, and the price is published with
it, so a reader knows the index's break-even is a number **2.935** and not a number **one**.

## Consequences

- **And the index's price is published on every axis**: the build, the write, the compaction, the lookup, the break-even, the
  memory and the memory's ceiling — in [§3an](../../PERF.md#3an-the-indexs-price-measured-on-every-axis) and
  [§3ao](../../PERF.md#3ao-the-indexs-memory-and-the-row-count-at-which-it-passes-the-budget), with the reports committed and the
  guard checking every figure inside the section that publishes it.
- **And the defect is recorded**: the compaction collapsed the table into one file and the index's value went with it — the
  compacted table's first lookup **181.172 ms** before, **43.281 ms** after, **and no index at all**.
- **And the engine's surface does not grow**: the index is the database's own, and the file's statistics are already reachable
  through the engine's own `ParquetSource`.
- **And the store's surface does not grow**: the store already writes the statistics, and the grid is the grid. **And the four
  crates publish at 0.1.0**, so a change to a published surface is a break in every embedder's build.
- **And the tombstone tax's shape is still open.** An indexed lookup reads one file, and that file's retired versions still cost
  a predicate each — **the index does not change the tax**.
- **And the index's own durability across a restart is still open.** The index is derived from the files (ADR-0068), so it is
  rebuilt at an open — **and the rebuild's price is the build's price, measured**.
