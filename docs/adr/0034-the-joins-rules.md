# ADR-0034 — The join's rules

**Status**: accepted
**Date**: 2026-10-28
**Spec**: [026-the-join-stage-taken-apart](../../specs/026-the-join-stage-taken-apart/spec.md)
**Amends**: [ADR-0030](./0030-the-scan-that-feeds-the-join.md) — the scan that feeds the join
**Extends**: [ADR-0033](./0033-the-crates-modules.md) — the crate's modules

## Context

ADR-0030 moved the join's probe **inside** the reader, so that the rows the join does not match are
never copied. That made the join run on **two** paths: the cold one, where the probe happens inside
the scan over the reader's own Arrow arrays, and the resident one, where the probe happens over the
engine's own materialised column.

To keep the two paths from answering differently, ADR-0030 put the key's lookup behind one
interface — `KeySource` — with two implementations: `KeyColumn`, over the engine's column, and
`ArrowKeys`, over the reader's array. **The interface is shared. The rules are not.**

Five of the join's rules turned out to have **two owners each**:

| rule | the two owners | what the review found |
|---|---|---|
| a NULL key never matches | `KeyColumn::value_at` (the validity bitmap) and `ArrowKeys::value_at` (each variant's own `is_null`) | ADR-0030 says the rule "lives in the shared lookup"; it does not. The interface also has no `len()`, so "the row is in range" is answered **differently**: `None` on one path, a **panic** on the other. |
| the two keys have the same type | the cold path reads the **file's** schema; the resident path reads the **materialised** chunk | Nothing establishes that the two sources agree, and an absent column is silently typed `Int64` instead of refused. |
| what to read, and what to push down | `operators/join.rs::decide_read` and `scan.rs::decide_scan` | The operator **reaches up** into `crate::scan` for its type list, and because it cannot say "the reader did all of it", a predicate the reader declined is evaluated by **two** evaluators. |
| the probe can read this column type | `key.rs::ArrowKeys::new` and `scan.rs::pushdown_supports` | Two lists of the same set in two modules. A wrong "yes" fails **mid-read**, after the plan was accepted and the table built. |
| the file's column is the dataset's column | `operators/join.rs::dataset_indices` and `plan::ColumnMapping::of` | The same rule written twice, and the join's copy keeps the silent fallback that spec 027 removed from the plan's. |

And two things the measurement depended on were **wrong**:

- The join's stage reports `selection.len()`, and the selection means different things on the two
  paths. The cold path's selection is over the dataset the probe **already reduced**; the resident
  path's is over the whole dataset. So the same probe over the same six million rows is reported as
  `216783 → 216783` on one path and `6000000 → 216783` on the other. **The two paths cannot be
  compared, because they do not report the same work.**
- `estimate_build_side` is the only estimate with **no test**, and it is not an upper bound: it
  charges the build filter's **survivors** where the probe side's estimate charges the file's
  **rows**, one copy where the parallel read holds **two**, and the key column alone where the join
  also reads every declined predicate's column. The budget is checked before a row is read; a check
  against an estimate that is not an upper bound is not a budget.

## Decision

**Every rule of the join has one owner, and the other caller asks.**

1. **The key's interface owns the key's rules.** `KeySource` says how long the column is, so that
   "the row is in range" is one answer on both paths, and both implementations answer it the same
   way. The NULL rule is stated once, in the interface, and the two implementations are held to it
   by a **differential test** over the same columns — NULLs, strings, dictionaries, the plain-integer
   fast path, and a row past the end.
2. **The reader owns the read decision.** What to read and what to push down is decided
   **once**, by the reader — `ParquetSource::decide_read` — and **every** read asks it: the
   probe side's scan with the plan's materialised set and the plan's predicates, and the
   build side with its own key as its materialised set and its own predicates. The operator
   does not reach up into the scan, and it does not re-derive the rules.
3. **The key module owns the probe's type support.** `ArrowKeys::new` publishes what the probe can
   read, and the scan asks it, instead of borrowing the pushdown's list.
4. **The plan owns the mapping.** The join's copy of "the file's column is the dataset's column"
   goes, and `plan::ColumnMapping` is the only one.
5. **The type check is made once, from one source.** The two keys' types are resolved where both are
   known, and an absent column is **refused** rather than silently typed.
6. **The reader reports what the filter examined**, and the join's stage reports that, so the two
   paths report the same work in the same place.
7. **The estimate is charged the way the read happens**: the file's rows, the read set the join
   decides to read, and the copies that read holds — and it is tested.

### The decision moved, and the reason is recorded

This ADR first said that **the scan** owns the read decision, and that the build side asks
`scan.rs` for it. **The decision moved down to the reader**, and the reason is the layering.

The build side is an **operator**, and the estimate is a **leaf**: both are **below** the scan,
and ADR-0033's rule is that nothing below depends on anything above it. A rule that two modules
below ask cannot live above them — so the operator asking the scan would have been the very
reach-up this ADR sets out to remove.

The reader is below all three. `scan.rs` asks it, `operators/join.rs` asks it, and `memory.rs`
asks it — so the estimate charges **exactly** the read set the join reads, rather than a copy of
the rules that guesses at it. The pushdown's list of the types it evaluates exactly moved with
it, into `pushdown.rs`: the pushdown owns that list, and a reader that borrows somebody else's
list promises the pushdown something it may not be able to do.

## The layout

```
lib.rs            the surface
pipeline.rs       the orchestration: the stages, in the plan's order
scan.rs           the scan's own rules: whether the scan feeds the join, and what it fetched
source.rs         the reader: what a read takes, and what it pushes down
plan/             the two faces and the bridge: the types, the mapping
operators/key.rs  the key's two sources, and the rules they share
operators/join.rs the table and the probe
pushdown.rs       what the reader can evaluate exactly
memory.rs         every estimate
```

The dependency runs **downward only**: `scan.rs` asks `operators/key.rs` what the probe can read
and asks `source.rs` what the read takes; `operators/key.rs` and `operators/join.rs` ask `plan/`
for the types and the mapping, and `source.rs` for the read; `memory.rs` asks `plan/` for the plan
and `source.rs` for the file. Nothing below reaches up.

## Consequences

- **The two paths are comparable**, because they report the same work: the join's stage reads
  `6000000 → 216783` on both, and the scan's stage says what it kept.
- **The build side's estimate becomes an upper bound**, and the budget it guards is worth guarding.
  A plan that passes the check has done no work at all, and now that is true.
- **The join's helpers take the pipeline's context**, the way the other stages do.
- **Two documents stop saying the opposite of the code**, and the dead surface goes.
- **The plan's shape does not change.** No new plan, no new join type, no new operator, a different
  table, a different hash, or a different reader. What changes is **where a rule lives** and **what
  the stage reports**.
- **The join's answer does not change.** The same rows, in the same order, with the same values; a
  NULL key never matches, and the multiplicity is unchanged. The differential test and the existing
  join suite hold it to that.
