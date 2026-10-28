# Spec 026 — The join's stage, taken apart

**Status**: in progress
**Started**: 2026-10-28
**ADR**: [ADR-0034](../../docs/adr/0034-the-joins-rules.md) — the join's rules, and why each has one owner
**Predecessor**: [025-the-sort-taken-apart](../025-the-sort-taken-apart/spec.md)

## The problem

**The join is the last published query where the engine is not ahead.** The engine's cold
`join-orders` is **96.7 ms** against DuckDB's **95.0 ms** — level, **1.7 ms** apart, inside both
spreads — while the peer's is **168.5 ms** and Polars' **222.8 ms**. The plan output says where the
engine's own time is, and it is the **join's stage**:

| stage | rows in | rows out | elapsed |
|---|---|---|---|
| scan | 6,000,000 | 216,783 | 73.6 ms |
| project | 216,783 | 216,783 | 0.6 ms |
| **join** | 216,783 | 216,783 | **24.2 ms** |
| aggregate | 216,783 | 1 | 0.3 ms |

**Accounted: 98.66 ms of the query's 98.67 ms — 0.01 ms outside the stages.** The join's stage is a
quarter of the query, and it is **all** the engine's own work: the build side's read, the build
side's keys, the table, and the probe.

The figures are the medians of the current three campaigns
(`bench-results/compare-run{1,2,3}.json`), and the plan output is in those reports, so every number
in this section is derived from a committed report.

**In resident mode there is no scan at all, and the join's stage is almost the whole query.** The
scan is **0.0 ms** — the data is already in memory — the project is **2.3 ms**, the aggregate
**0.3 ms**, and the join's stage is **21.3 ms** of the engine's **24.2 ms**. That query is the
engine's **10.8 ms** margin over DuckDB (**35.0 ms**). And the join's stage is the **largest single
piece of the engine's own work on any published query**: the filter's stage is **12.3 ms** at its
largest, the sort's **2.8 ms**, the aggregate's **3.2 ms**, the project's **2.8 ms**. So the join's
stage is both where the engine is closest to losing and where the engine's own work is largest.

**And the stage has never been taken apart.** The join's **probe** was measured once, in spec 021's
campaign, two increments ago, on a different code version and a different host state: over
**6,000,000** probe rows against a table of **216,345** build keys, the engine's probe ran at
**9.0 ms** against a floor of **1.7 ms** — the cost of reading the probe key column's own integers.
The build side's read was measured separately, at **9.99 ms**, as part of the *scan's* decomposition.
**No measurement has ever put the stage and its parts in the same session**, and no measurement has
ever asked what the stage's **other** parts are.

**And the two paths do not agree, and nobody has said why.** The same parts are charged to the
join's stage on both paths: the build side's open, the table's construction, and the probe over
six million rows. On the cold path the probe runs **inside the scan**, over the reader's own
batches, before anything is copied; on the resident path it runs over the engine's own column, in
memory, after the copy. The cold path's join stage is **24.2 ms**; the resident path's is
**21.3 ms**. **The cold copy is 2.9 ms slower for the same charge.** The cold path's stage also
reports **216,783** rows in and the resident path's **6,000,000**, for the same probe — so the two
numbers are not even about the same thing, and neither is about what the stage *does*.

**So this spec does not assume there is a win.** It assumes there is a number that has never been
taken apart, and that the number is worth naming either way. The premise is that the join's stage is
the engine's own work on the query where the engine is level. **If the measurement shows the stage is
mostly the build side's read** — Arrow's decode, which has been measured three times and refused
every time — **then the requirement is the same: the part that is the engine's own work is
separated from the part that is not, and the whole is named.**

## The requirements

### R1 — The stage is taken apart, in one session, against the engine's own output

The join's stage is measured against its own parts **in the same session, on the same data, on the
same rows**, on **both** paths — the cold one, where the scan feeds the join, and the resident one,
where the join probes the dataset itself. The parts are at least:

| part | what it is |
|---|---|
| the build side's **open** | opening `orders.parquet`: its metadata, its schema |
| the build side's **read** | reading the build side's two columns through the engine's own reader |
| the build side's **keys** | the per-row walk over the build side that collects its keys |
| the **table** | the 216,345-key table the probe looks in: a direct-addressed one, or a hashed one |
| the **probe** | the lookup of six million probe rows against the table |
| the **output** | the 216,783 rows the join produces |

The engine's own answer is checked **before** anything is timed: a probe that measures a different
join is not a measurement.

### R2 — The two paths are compared, and the difference is named

The cold path's join stage and the resident path's join stage are **charged with the same
parts**, and they differ by **2.9 ms**. The difference is attributed to a part, or **named** as the
remainder. Either the cold path's probe is more expensive than the resident path's probe — it runs
inside the reader, over the reader's own batches, against the caches the decode is using — or the
build side's read is more expensive when the file is not warm, or the two paths' clocks cover
different things. **Whichever it is, it is measured and published.** The two paths' row counts are
made comparable, or the difference between them is named in the plan output itself.

### R3 — The join's answer does not change

The 216,783 rows the join produces, their order, and the aggregate's value are the same numbers as
before, bit for bit. A NULL key never matches, on either side, and the multiplicity — one output
row per match — does not change.

### R4 — The figures are published

The instrument's medians, the before and the after side by side, the reason for every moved figure,
and the raw reports committed. The gate derives every published figure from the committed reports,
and the guard checks the figures **in the section that publishes them**.

### R5 — The join's stage reports the rows the probe consumed

**The two paths report different numbers for the same work, and the difference is the join's own.**

Measured on `join-orders`, the resident path's join stage reads
`join  6000000  216783` and the cold path's reads `join  216783  216783`. The probe examined
**six million** rows on both paths. On the cold path the probe runs **inside** the reader, so the rows
it examined are charged to the **scan** stage and the join's stage sees only what survived.

The join's stage reports `selection.len()`, and the selection means different things on the two paths:
on the cold path it is a selection over the dataset the probe **already reduced**, and on the resident
path it is a selection over the whole dataset.

**The reader reports how many rows the filter examined**, and the join's stage reports that. The two
paths then say the same thing: `6000000 → 216783`, and the scan's stage says what it kept.

### R6 — Every rule of the join has one owner

Five rules of the join are written **twice**, in two modules, with nothing checking that the two
copies agree:

| rule | the two owners |
|---|---|
| **a NULL key never matches** | `KeyColumn::value_at` (the validity bitmap) and `ArrowKeys::value_at` (each variant's own `is_null`) |
| **the two keys have the same type** | `pipeline.rs`'s streamed path (the **file's** schema) and its resident path (the **materialised** chunk) |
| **what to read, and what to push down** | `operators/join.rs::decide_read` and `scan.rs::decide_scan` |
| **the probe can read this column type** | `key.rs::ArrowKeys::new` and `scan.rs::pushdown_supports` |
| **the file's column is the dataset's column** | `operators/join.rs::dataset_indices` and `plan::ColumnMapping::of` |

**Each rule has one owner, and the other caller asks.** The NULL rule is stated once, in the
interface both key sources implement, and the interface says how long the column is so that "the row
is in range" has one answer. The type check is made once, from one source. The read decision is made
once, in the module that owns the scan. The probe's type support is published by the key module and
asked for by the scan. The mapping is the plan's.

**And the two key sources are compared, not merely trusted.** The join's key lookup has **two**
implementations, and the only test that compares the two paths uses **integer, non-NULL** keys. A
differential test holds the two implementations to the same columns — NULLs, strings, dictionaries,
the plain-integer fast path, and a row past the end — and they must agree, value for value.

### R7 — The build side's memory estimate is an upper bound

`estimate_build_side` is the one estimate with **no test**, and it is not an upper bound. It charges:

- the **survivors** of the build filter, where the probe side's estimate charges the **file's rows** —
  and the reader's peak is over the file's rows;
- **one** copy of the read set, where the probe side's estimate charges the parallel read's **two**;
- the **key column alone**, where the join reads the key **and every predicate the reader declined**.

The budget is checked **before a row is read**, and a plan that passes the check is a plan that has
done no work at all. That claim only holds if the estimate is an upper bound.

**The estimate is charged the way the read actually happens**: the file's rows, the read set the join
decides to read, and the copies that read holds. And it is **tested**, the way the other estimates are.

### R8 — The join's helpers take the pipeline's context

`ordered_stage` and `join_outcome` take **seven** parameters each, and `build_streamed_table` takes
six — one below the line where clippy would complain. Spec 027 gave the pipeline a `Pipeline` context
and moved these two helpers **verbatim**, leaving them behind.

**The join's helpers take the context.** A new stage is handed the same context as the others, and a
helper's parameter list stops being a copy of the context's fields.

### R9 — The documents say what the code does

Two documents say the **opposite** of what the code does:

- `explain.rs` says "the scan's elapsed carries the probe's cost". It does not: the scan's elapsed
  **excludes** the probe, and the join's stage **includes** it.
- `memory.rs` says the join "abandons the direct table" when a key more than 255 build rows carry,
  and that neither that case nor the overflow map is charged. The join keeps the table and spills the
  count into the overflow map, and the overflow map **is** charged.

**And the dead surface goes**: `KeyColumn::from_decoded` is unused, `KeySlice`'s dictionary variants
are built and never read, and `KeySlice::integers` duplicates `KeyColumn::integer_values`.

## The scope

**What this spec does not do, and why.**

- **The scan's stage is not changed.** It is **73.6 ms** of the cold query, and it is Arrow's decode of the
  two columns the query reads — the decode that was measured **three** times and **every time it was
  refused**. This spec is about the join's stage, and the scan's share of the join's stage is
  measured, not changed.
- **The build side's reader is not replaced.** The build side is read through the engine's own
  `ParquetSource`, so the same reader settings apply to it as to any other file, and that is the
  right thing. This spec measures what that read costs inside the join's stage; it does not replace
  it.
- **The table's shape is not rewritten.** The direct-addressed table — the key *is* the index, one
  byte a count — was chosen by measurement in spec 021 and beat every candidate in that campaign. A
  different table is a different spec, and it would need its own measurement to earn its place.
- **The probe's hash is not changed.** `FxHash` was chosen by measurement in spec 021 and is
  already the reason the probe runs at about a tenth of the default hasher's cost.
- **The eager path's load is not touched.** The load is **556.3 ms**, and it is the one published
  figure where the engine is behind the peer (**398.6 ms**). That is a real gap and it is a
  **different** measurement — the load sweep's, on the load sweep's file — and this spec does not
  mix the two.
- **The engine does not move the build side's read outside its own clock.** In resident mode the
  incumbents hold both tables in memory before the clock starts, and the engine's resident path
  reads the build side **inside** it. The asymmetry is real, it is documented, and it works
  **against** the engine. This spec **names** what the build side's read costs inside the join's
  stage; it does not hide it, and it does not claim the engine's stage is smaller than it is.
- **The join's plan is not changed.** No new plan, no new join type, no new operator. If the
  measurement shows the join's stage is at a floor, that is the finding, and it is published as one.

> **The scope is amended, and the reason recorded.** The requirements above were written for a
> **measurement**: take the stage apart, name the difference between the two paths, and publish.
> The review that followed found that the measurement **cannot be made** as it stands, because the
> two paths do not report the same work in the same place — the join's stage says
> `216783 → 216783` on one path and `6000000 → 216783` on the other, for the same probe over the
> same six million rows (R5). Five of the join's rules have two owners each (R6), the build side's
> memory estimate is not an upper bound (R7), the join's helpers were left behind by spec 027's
> context (R8), and two documents say the opposite of the code (R9).
>
> **So the structure is in scope, and the exclusions above still hold.** The table's shape, the
> probe's hash, the reader and the plan are all untouched: what changes is **where a rule lives**,
> **what the stage reports**, and **what the estimate charges**. A measurement of a stage that
> reports two different things for the same work is not a measurement of the stage, and a budget
> checked against an estimate that is not an upper bound is not a budget.


## The instrument

`crates/columnar-engine/examples/join_cost_probe.rs`, committed in the tree. It:

- runs the engine's **own** plan for `join-orders` through the engine's own entry point, on **both**
  paths, and takes the join's stage **from the plan output** — so its stage is the engine's stage and
  not a copy of it;
- measures each part of R1 on its own, in the same session, on the same data;
- measures the **probe** both ways — inside the reader's own batch loop, and over the engine's own
  column in memory — because the two paths probe differently and the difference is R2's question;
- measures the **controls**: the probe key column read plain, the floor the probe cannot go below;
  the same read over a buffer small enough to be in the caches, which says whether a read waits for
  memory; and the read of the build side's columns on their own;
- checks the engine's 216,783 rows against the probe's own, **in order**, before anything is timed.

Three campaigns on the same host, the reports committed as
`bench-results/join-cost-run{1,2,3}.json` — and the same probe against the code this spec replaces,
committed as `bench-results/join-cost-before-run{1,2,3}.json`.

## The measurement

To be filled from the instrument's campaigns. The parts, the medians, the two paths, and the
attribution of the **2.9 ms** between them.

## The references

| | |
|---|---|
| [ADR-0021](../../docs/adr/0021-the-materialised-column-and-a-rust-peer.md) | the peer the engine is compared against, and how the two are held to the same conditions |
| [ADR-0028](../../docs/adr/0028-the-hot-paths-taken-apart.md) | the join's probe taken apart the first time, and the instrument that took it apart |
| [ADR-0030](../../docs/adr/0030-the-scan-that-feeds-the-join.md) | the scan that feeds the join, and the rule that the rows are read where they lie |
| [ADR-0031](../../docs/adr/0031-the-time-the-plan-output-does-not-name.md) | the plan output's account of the whole query, and the rows the join is handed |
| [spec 021](../021-the-hot-paths-taken-apart/spec.md) | the join's probe measured against its floor, and the table's shape chosen by measurement |
| [spec 023](../023-the-scan-that-feeds-the-join/spec.md) | the join's probe moved inside the scan, and the rule it set |
| [spec 024](../024-the-time-the-plan-output-does-not-name/spec.md) | the selection that holds every row is the range, and the plan output's account of the whole query |
| [spec 025](../025-the-sort-taken-apart/spec.md) | the sort taken apart, the floor that was a scalar read, and the method this spec follows |
| [`PERF.md`](../../PERF.md) §2a, §2b, §2h, §3l, §3m, §3n | the published tables this spec's figures are re-derived into |
