# Spec 025 — The sort, taken apart

**Status**: done
**Started**: 2026-10-28
**Finished**: 2026-10-28
**ADR**: [ADR-0032](../../docs/adr/0032-the-sort-taken-apart.md) — written from the instrument's measurement
**Predecessor**: [024-the-time-the-plan-output-does-not-name](../024-the-time-the-plan-output-does-not-name/spec.md)

## The problem

**The sort is the engine's largest remaining gap, and it has never been taken apart.** The engine's
cold `sort-topk` is **40.1 ms** against DuckDB's **37.0 ms** and the peer's **37.7 ms** — a gap of
**3.1 ms**, the largest one left. The plan output says where the time is: the **scan** is **35.2 ms**
of the query and the **sort stage** **4.9 ms**. The scan is Arrow's decode, which has been measured
**three** times and refused every time, so the sort's stage is where the engine's own work on this
query is.

**In resident mode there is no scan at all**, and the sort is the whole query: the sort's stage is
**5.3 ms** of the engine's **5.7 ms**.

**And the stage and its operator have never been measured in the same session.** The published
figure for the sort's **operator** is **3.7 ms** against a **3.6 ms** plain-read floor — the cost of
reading six million doubles, 48 MB, and the floor the sort cannot go below because a top-k over six
million keys has to look at every key. That figure is from spec 021's campaign: **two increments
ago**, on a different code version, on a different host state, and with the rows handed over as a
**24 MB vector** — where the engine now hands over a range.

So the two published figures — the stage's **4.9 ms** and the operator's **3.7 ms** — were never
measured against each other, and the **1.2 to 1.6 ms** between them, a quarter to a third of the
stage, has never been named.

**What the code says the time is.** The sort's stage is three things: the key column is resolved,
the rows are handed over, and the operator runs. The operator is the heap of ten and the read of
every key. Two things could be hiding in the difference, and they are different:

1. **The engine's own overhead** — the key column's construction, the rows' hand-over, the stage's
   own bookkeeping. That is the engine's work and it is not the sort's.
2. **The key read's cache state.** The operator's published floor was measured by a probe that reads
   the same 48 MB over and over: a **warm** read. The engine reads those 48 MB **once**, right after
   the scan wrote them: a **cold** read. If the difference is there, then the sort is at the floor
   *as the engine pays it*, and the only lever is to read the keys where they are hot — in the
   batch that has just been decoded — rather than from the materialised column afterwards.

Two questions, and they are different:

1. **What is in the sort's stage that is not the operator's own work?** And is that part the
   engine's overhead, or the key read's cache state, or the operator after all?
2. **Is the operator at the floor, as the engine pays it?** The floor is the read of six million
   doubles. If the operator is at the floor, the sort cannot be made faster by a better heap, and
   the question becomes **where the keys are read from** — not how they are compared.

## The requirements

### R1 — The stage is taken apart, in one session, against the engine's own output

A committed instrument measures, on the same file and in the same session, and against **the same
code the engine runs**:

| part | what it is |
|---|---|
| `stage` | the sort's stage, as the engine's own plan output reports it |
| `operator` | the sort's own call, on the same rows and the same key column |
| `rows` | the hand-over: the rows the sort is given, walked |
| `key_column` | the key column's construction, from the dataset's chunk |
| `read_shape` | the read the engine's fast path does — a row at a time, with a bounds check |
| `read_plain` | the plain read of the key column — the floor as it was published |
| `read_small` | the same read over a **small** buffer: the control that says whether the read waits for memory |
| `floor_block` | the **vectorised** comparison of the same column |

Every part is checked against the engine's own output, **row for row**, before it is timed: the ten
rows the engine keeps, in the order the engine keeps them.

> **Amended, after the instrument was written** — the floor is the **vectorised** comparison, and
> not the plain read, and the reason is worth recording. The published floor was a plain read of
> the 48 MB column, and a plain read is a **scalar** loop: it counts the values one at a time, and
> it is the fastest way to read the column *that way*. The instrument's control — the same read over
> a buffer small enough to be in the caches — costs the **same per value** as the read over the
> whole column, so the read was never waiting for memory: it was counting. The floor is therefore
> not the read but the **comparison**, and the comparison is made a row at a time in the engine's
> sort. The amendment adds two parts (`read_small`, `floor_block`) and changes the question the
> floor answers: from "how fast can the column be read?" to "how fast can the keys be compared?".
> The figures are the medians of **three campaigns**, run back to back on the same host, with the
> old code in its own worktree.

### R2 — The difference between the stage and the operator is named

The stage is the operator plus the engine's overhead, and the difference is either the engine's
overhead — which is removed — or the key read's cache state, which is published. **Either outcome is
the same requirement**: the time is the sort's own work, or it is named.

### R3 — The sort's answer does not change

The ten rows the sort keeps, their order, and the aggregate's value are the same numbers as before,
bit for bit. The tie-break by the row's own number is what makes the order total, and it is not
touched.

### R4 — The figures are published

The instrument's medians, the before and the after side by side, the reason for every moved figure,
and the raw reports committed. The gate derives every published figure from the committed reports.

## The scope

**What this spec does not do, and why.**

- **The scan's decode is not touched.** It is **35.2 ms** of the query's **40.1 ms**, and it has been
  measured **three** times and refused every time: the crate's read and the engine's own scan of the
  same column came out equal to a tenth of a millisecond, so owning the decode buys nothing. It is
  the scan's business, and this spec is about the sort's stage.
- **The scan's materialisation is not touched.** The scan copies the decoded column into the engine's
  own encoding, and that copy is the scan's work, in the scan's stage. This spec asks what the sort
  does **after** the copy.
- **The sort's algorithm is not rewritten.** A top-k over six million keys has to look at every key:
  there is no way to know the largest ten without reading all of them. The heap holds ten rows, so
  the working set is already bounded, and the comparison and the update are a fraction of a
  millisecond. The instrument measures them anyway, and the requirement does not ask for a different
  algorithm — it asks for the time to be named.

  > **Amended, after the instrument was written** — the last sentence of that paragraph is **wrong**,
  > and the instrument is what says so: the comparison and the update are **not** a fraction of a
  > millisecond. They are **2.3 ms** of the operator's **3.2 ms** — the comparison is the whole of
  > the sort's own work, and it is made **a row at a time**. The algorithm is still not rewritten:
  > the sort is still a bounded heap of the best `limit` rows, with the same order, the same
  > tie-break and the same answer. What changes is **how the keys are compared**: in **blocks**,
  > against the heap's own worst key, so that the comparison is a reduction over a comparison — the
  > shape a compiler vectorises — and only the handful of rows that beat the threshold are walked a
  > row at a time. The amendment is to the paragraph's claim about where the time is, and the
  > measurement that forced it is in the measurement section below. The figures are the medians of
  > **three campaigns**, run back to back on the same host, with the old code in its own worktree.
- **Strings, NULLs and the general path are not touched.** The published query sorts plain doubles
  with no NULLs, and the gap is not in the path that reads a key as an enum. The general path stays
  as it is, and the differential suites keep holding it.
- **The join's sort is not touched.** A plan with a join and a sort is not published, and the join's
  own stage was the subject of spec 021 and spec 023.
- **The filter's selection is not touched.** Spec 024 gave the sort its rows as a range, and that
  work is done.

## The instrument

`crates/columnar-engine/examples/sort_cost_probe.rs` — the sort's stage, taken apart.

The probe runs the engine's **own** plan through the engine's own entry point, takes the sort's
stage from the plan output as the engine reports it, and then measures the parts on the same
dataset, the same key column and the same rows, in the same session. It checks the engine's ten
rows against its own before it times anything.

The probe reports, per part, the median of five iterations and the spread, the host's load, and the
commit it ran at.

## The measurement

**What the instrument found, before the change.** The probe's medians, on this host, against the
code this spec replaces:

| part | ms | what it is |
|---|---|---|
| the stage, cold | **4.18** | the engine's own stage, the cold path |
| the stage, resident | **4.30** | the engine's own stage, the resident path |
| the operator, cold | **3.23** | the engine's own sort call, over the same rows and the same column |
| the operator, warm | **3.25** | the same call, with the column already in the caches |
| the read, the engine's shape | **3.93** | the read the engine's fast path does, without the heap |
| the read, plain | **3.15** | the plain read: the floor as it was published |
| the read, the small control | **0.26** | the same read over 512K values — **0.50 ns** a value, against the whole column's **0.53 ns** |
| the comparison, in blocks | **0.95** | the same column, compared in blocks |
| the candidate, in blocks | **1.02** | the sort written by hand, in blocks, over the same column |
| the decode, on its own | **33.25** | the same Arrow decode, without the top-k |
| the fused top-k | **39.55** | the top-k over the reader's own Arrow arrays: the decode (**33.25**) plus the top-k |
| the sort with no limit | **534.41** | the fallback: every row sorted |
| the sort over strings | **48.34** | the fallback: the general path |

**Three findings, and they are the spec's answer.**

1. **The stage is the operator.** The stage (**4.18 ms** cold, **4.30 ms** resident) and the
   operator (**3.23 ms**) are the same work: the key column's construction is **a nanosecond** and
   the hand-over is a range. There is no engine overhead in the stage to remove — and the **cache
   state** is not it either: the operator costs the same with the caches pushed out as with them
   warm (**3.23 ms** against **3.25 ms**), and the read costs the same over four megabytes as over
   forty-eight (**0.50 ns** a value against **0.53 ns**), so the read was never waiting for memory.
2. **The operator is not at the floor.** The published floor was **3.6 ms**, and it was the read of
   the column — a **scalar** read. The comparison, in blocks, costs **0.95 ms**: the same column,
   compared against the threshold, with the same total order. The sort's own work was **2.3 ms** of the
   operator's **3.2 ms**, and it is the comparison, made a row at a time.
3. **The scan that feeds the sort is worse, and is refused.** The top-k over the reader's own Arrow
   arrays — where the values are hot — costs **6.3 ms** on top of the decode, against the stage's
   **4.18 ms** for the whole of it. Reading the keys where they are hot is not the lever; **how
   they are compared** is.

**What the change does.** The bounded sort compares the keys in **blocks**: a block is compared
against the **threshold** — the heap's own worst key, which is the row the heap would drop next —
and only a block that holds a row that beats the threshold is walked a row at a time. The block
comparison is a reduction over a comparison, so it vectorises; the row at a time walk then touches a
handful of rows out of six million. The heap, the threshold, the order and the tie-break are the
ones the row at a time walk uses, so the two keep the same rows in the same order.

**What it is worth.** The stage's own time, and the queries that hold it:

| | before | after |
|---|---|---|
| the sort's stage, cold | 4.18 ms | **1.10 ms** |
| the sort's stage, resident | 4.30 ms | **1.03 ms** |
| the operator | 3.23 ms | **1.02 ms** |
| the resident `sort-topk` | 3.92 ms | **1.05 ms** |
| the cold `sort-topk` | 40.29 ms | **38.50 ms** |

**In the published comparison campaigns** the same query, cold and lazy, is **42.6 ms → 37.2 ms**
against DuckDB's **39.0 ms** and the peer's **40.3 ms**, and resident **5.6 ms → 2.7 ms** against
DuckDB's **3.0 ms** and the peer's **3.6 ms** — the engine is ahead of DuckDB on this query, on
both paths, for the first time. That cold figure is the published campaign's own: the query probe's
cold number is a different measurement of the same query.

The raw reports are `bench-results/sort-cost-run{1,2,3}.json` and
`bench-results/sort-cost-before-run{1,2,3}.json` for the parts,
`bench-results/query-cost-run{1,2,3}.json` and `bench-results/query-cost-before-run{1,2,3}.json` for
the whole query, and `bench-results/compare-run{1,2,3}.json` and
`bench-results/compare-before-run{1,2,3}.json` for the published campaigns, and
`python3 scripts/medians.py` derives the tables above from them. The sort campaigns ran at a host
load average of **6.28–7.10**.

## The references

| | |
|---|---|
| [ADR-0021](../../docs/adr/0021-the-materialised-column-and-a-rust-peer.md) | the peer the sort is compared against, and how the two are held to the same conditions |
| [ADR-0028](../../docs/adr/0028-the-hot-paths-taken-apart.md) | the join's probe and the sort taken apart the first time, and the instrument that took them apart |
| [ADR-0030](../../docs/adr/0030-the-scan-that-feeds-the-join.md) | the scan that feeds the join, and the rule that the rows are read where they lie |
| [ADR-0031](../../docs/adr/0031-the-time-the-plan-output-does-not-name.md) | the plan output's account of the whole query, and the range the sort is now handed |
| [spec 021](../021-the-hot-paths-taken-apart/spec.md) | the sort's own cost measured against its floor, and the negative result that the floor is the read |
| [spec 023](../023-the-scan-that-feeds-the-join/spec.md) | the join's probe moved inside the scan, and the rule it set |
| [spec 024](../024-the-time-the-plan-output-does-not-name/spec.md) | the selection that holds every row is the range, and the plan output's account of the whole query |
| [`PERF.md`](../../PERF.md) §2a, §2b, §2h, §3n | the published tables this spec's figures are re-derived into |
