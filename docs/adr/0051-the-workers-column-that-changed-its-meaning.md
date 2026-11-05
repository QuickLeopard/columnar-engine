# ADR-0051 — The workers column that changed its meaning

**Status**: accepted
**Date**: 2026-11-05
**Spec**: [041-the-readers-four-gigabytes](../../specs/041-the-readers-four-gigabytes/spec.md)

## Context

**The sweep report carried one worker count, and two things are capped by different numbers.**

The constitution's performance standard says:

> A published worker count is the count that **ran**: a plan that reports its parallelism MUST
> report the workers that **ran**, not the threads that were **asked for**. The two differ — the
> **read's** parallelism is capped by the file's row groups, and the **operators'** by the row
> count — and a reader who is shown the requested count will read the curve wrong.

**Two counts. The report had one.**

`ScalingPoint::workers` was the read's worker count — `min(threads, row_groups)` — and the doc
comment on it said:

> In the **cold** mode this is the read's worker count; in the **resident** mode there is no read
> in the measurement, so it is the operator worker count instead.

**The comment describes spec 007's behaviour, and spec 020 changed the code without changing the
comment.** At spec 007 the resident mode took its worker count from the measurement's own plan
output — the filter's — and the cold mode took it from the source. At spec 020 the CLI was
rewritten and the resident mode's count was replaced by the source's, in **every** mode, and the
comment was left behind.

**So the resident sweep's published table has carried the operators' count under the header "the
engine's workers" since spec 020 — and the cold sweep's has carried the read's count under the
same header.** On `data/lineitem.parquet`, whose six row groups cap the read at six workers:

| | 8 threads | 14 threads |
|---|---|---|
| **the read's** workers — the file's row groups | **6** | **6** |
| **the operators'** workers — the row count | **8** | **14** |

§2e published the first row and §2f the second, under one header.

**The bump's re-measurement is what surfaced it.** The dataset was regenerated, so the sweep
families were re-taken on the bumped tree — and the new reports carry the **read's** count, the
way the code has behaved since spec 020, while the committed set they replace carries the
**operators'** count, the way the code behaved at spec 017. **The same column, the same header,
two different numbers, and no report says which is which.**

## Decision

1. **The report carries both counts, and says which is which.** `workers` is the **read's** worker
   count, in every mode — its doc comment is corrected, and it says that in the **resident** mode
   it is the file's *cap* rather than a count of anything that ran. `filter_workers` and
   `projection_workers` are the **operators'**, taken from the plan output the measurement already
   produces. Both are `Option`, and `None` means *not measured* — never zero: the load mode answers
   no query, so it has neither.

2. **The seven sweep families are re-measured**, so that every sweep report can say both. The
   superseded sets stay committed under their own names.

3. **The published tables name the two counts apart.** §2e, §2f and §2g publish the read's count
   as *the read's workers* and the operators' as *the operators' workers*, and §2f says what the
   column used to hold — **the operators' count, published under the read's header** — with the
   reason.

4. **No figure is changed by this.** The timings are the timings; the re-measurement moved them
   for the bump's reasons, which are published in
   [`PERF.md` §3x](../../PERF.md#3x-the-bump-and-the-figures-it-moved). This ADR is about **what a
   column means**, and the correction is of the meaning and not of a number.

## Consequences

- **The published sections that name a worker count are corrected where they publish it.** §2f's
  prose said *"the table records the workers that ran"* — and the table recorded the operators'
  count under the read's header. It now names both, and says what the previous set's column held.

- **The house rule that a table is derived from the committed reports still holds.** The operators'
  counts are in the reports, so `scripts/medians.py` derives the column and
  `scripts/check-published-figures.py` checks it in the section that publishes it.

- **The engine's behaviour did not change.** The count the code has reported since spec 020 is the
  one it reports now; **the comment, the doc and the published table are what changed**, and they
  changed to match the code.

- **The surface does not move.** The report's new fields are in `columnar-bench`'s report types,
  which are not the engine's surface; the engine's **46 names** are untouched.

## References

- [`constitution.md`](../../constitution.md) IV — evidence over claims: a column that changed its
  meaning was published as if it had not, and the correction is published where the column was.
- [`constitution.md`](../../constitution.md) VII — explicit scope, no silent deferral: the harness
  change this increment makes is named, and it is a correction rather than a feature.
- [`spec 041`](../../specs/041-the-readers-four-gigabytes/spec.md) — the reader's four gigabytes,
  and the amendment that records this.
- [ADR-0050](0050-the-bump-and-the-exemption-that-named-nothing.md) — the bump, and the two
  corrections it caused.
- [`crates/columnar-bench/src/scaling.rs`](../../crates/columnar-bench/src/scaling.rs) — the report
  type, and the doc comment this ADR corrects.
- [`PERF.md` §2f](../../PERF.md#2f-the-resident-sweep-where-the-operators-are-the-measurement) — the
  table that published the two counts under one header.
