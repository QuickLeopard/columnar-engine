# ADR-0033 — The crate's modules

**Status**: accepted
**Date**: 2026-10-28
**Spec**: [027-the-plan-decomposed](../../specs/027-the-plan-decomposed/spec.md)
**Amends**: [plan 001](../../specs/001-columnar-core/plan.md) — the crate layout

## Context

Plan 001 named the crates and, inside `columnar-engine`, named five things: a **Parquet reader**,
**operators**, a **plan**, a **memory budget** and a **plan output**. Those became `source.rs` and
`dataset.rs`, `operators/`, `plan.rs`, `memory.rs` and `explain.rs`.

Four days of increments later — the join, the sort, the dictionary, the pushdown, the attribution
— left two files carrying seven jobs between them:

- `plan.rs`, **756 lines**: the plan the caller writes, the resolution, the plan the executor
  reads, the late-materialisation policy, and the mapping that moves the resolved plan onto the
  read set.
- `lib.rs`, **1310 lines**: the public surface, the scan's decision, the join's build side, two
  memory estimates, and the pipeline.

The symptoms are not hypothetical. `run_pipeline` takes **ten parameters** and carries
`#[allow(clippy::too_many_arguments)]`, and the constitution's "complexity requires justification"
rule says an `#[allow]` accumulates as an ADR — **there was none**, because the allow was not a
decision anybody made. A stage reports its work in **four different shapes**. `remap_index`
**panics** on an invariant that another module establishes.

None of this makes the engine slow. It makes the code that decides *what* to run laid out so that
changing one decision means reading five files.

## Decision

The `columnar-engine` crate is laid out as **one layer per decision**, and the layers depend only
downwards:

```
crates/columnar-engine/src/
  lib.rs           the public surface, and the story: the error, the options,
                   execute, execute_on
  pipeline.rs      the orchestration: the stages, in the plan's order
  scan.rs          the scan's decision: what to read, what to push down, the
                   mapping onto the read set, and whether the scan feeds the
                   join
  plan/
    mod.rs         the plan module's surface and the pinned optimisation order
    spec.rs        what the caller writes
    resolved.rs    what the executor reads
    resolve.rs     the bridge: names to indices, the type checks, the mapping
  operators/       the leaves: filter, project, expr, aggregate, sort, join, key
  explain.rs       the plan output
  memory.rs        every memory estimate
  source.rs        the Parquet reader
  dataset.rs       the dataset and its chunks
  pushdown.rs      what the reader can evaluate exactly
  dictionary.rs    the dictionary-encoded column
```

**Bottom up.** The leaves are `operators/` and `plan/resolved.rs` — the vocabulary the operators
speak and the computations themselves. Above them sit the decisions: the bridge (`plan/resolve.rs`),
then the scan's decision (`scan.rs`), then the orchestration (`pipeline.rs`), then the surface
(`lib.rs`). Nothing below depends on anything above it.

**Top down.** The plan's journey is one function per step, and each step lives in the module that
owns it: `QueryPlan` → `resolve` → `ResolvedPlan` → `decide` → the read → the filter → the join →
the sort → the project → the evaluate → the aggregate → the plan output.

**Three rules the layout holds to.**

1. **The plan has two faces and they are two modules.** What the caller writes (`plan/spec.rs`)
   and what the executor reads (`plan/resolved.rs`) are different things with different
   representations, and the bridge between them (`plan/resolve.rs`) is the only module that knows
   both.
2. **A decision and what it produces live together.** The scan's decision and the **mapping** it
   produces are both in `scan.rs`, so the reader's view of the file — what it fetched, and where
   each column went — is in one place. The **late-materialisation set** is the resolved plan's own
   named field, `materialised`: the bridge produces it and the scan reads it by that name, so the
   boundary between them is a named field and not a bare vector.
3. **The crate root is the surface.** `lib.rs` is what a reader opens first: the error, the
   options, the two entry points, and the story. The machinery is in the module that owns it. The
   pipeline and the scan's decision are `pub(crate)` modules of the crate, not modules of its
   public surface: nothing outside the crate names them.

**`plan.rs` becomes `plan/`.** The public paths do not move: `columnar_engine::plan::JoinSpec`
and the crate root's re-exports stay where they are, so the change is invisible to a caller.

## Consequences

- **`lib.rs` is 378 lines**, down from 1310, and it is a reader's first stop rather than the place
  the pipeline lives.
- **A new stage is one module and one shape.** R5 of the spec: every stage reports in one shape,
  so the plan output is assembled in one place.
- **`#[allow(clippy::too_many_arguments)]` goes, and needs no ADR to justify its absence.** The
  pipeline's state is a context, not a parameter list.
- **The one panic that crosses a module boundary goes.** The operators' own `expect`s stay: they
  are each module's invariant, asserted where it is established. `remap_index`'s was different —
  its invariant is established by the scan and relied on by the plan, and a boundary invariant that
  nobody checks is a crash waiting for the right plan.
- **The crate layout in plan 001 is amended.** It named five modules inside the engine; there are
  now eleven, and the pipeline and the scan's decision are named as modules of their own.
- **Nothing the engine does changes.** Same answer, same plan output, same estimates, same
  published figures.

## What this does not do

- **It does not restructure the operators.** They are the leaves, one concern each, and a refactor
  that touched them would be a change of behaviour dressed as a change of layout.
- **It does not add a module for the sake of it.** `explain.rs`, `source.rs`, `dataset.rs`,
  `pushdown.rs` and `dictionary.rs` each own one thing and are left alone.
- **It does not introduce a `#[allow]`**, an `unsafe` block, a dependency or a panic.
- **It does not change the optimisation order.** The pinned order is a constant, printed in every
  plan output, and it is not this ADR's business.

## References

| | |
|---|---|
| [constitution.md](../../constitution.md) | "Complexity requires justification": an `#[allow]`, an `unsafe` block or a deviation from the crate layout accumulates as an ADR |
| [plan 001](../../specs/001-columnar-core/plan.md) | the crate layout this ADR amends |
| [ADR-0031](0031-the-time-the-plan-output-does-not-name.md) | the plan output's account of the whole query — the reason the stages report in one shape |
| [ADR-0030](0030-the-scan-that-feeds-the-join.md) | the scan's decision, and the rule that the rows are read where they lie |
| [spec 027](../../specs/027-the-plan-decomposed/spec.md) | the requirements and the steps |
