# Spec 027 — The plan, decomposed

**Status**: done
**Started**: 2026-10-28
**Finished**: 2026-10-28
**ADR**: [ADR-0033](../../docs/adr/0033-the-crates-modules.md) — the crate's module layout, and why it is this one
**Predecessor**: [026-the-join-stage-taken-apart](../026-the-join-stage-taken-apart/spec.md)

## The problem

**The engine's plan has two faces and five jobs, and they all live in one file.** `plan.rs` is
**756 lines** and holds: the plan **the caller writes** (`QueryPlan`, `AggregateSpec`, `JoinSpec`,
`SortSpec`, `Aggregate`); the **resolution** that turns column names into indices and type-checks
them; the plan **the executor reads** (`ResolvedPlan` and its six siblings); the
**late-materialisation policy** — which columns the scan decodes; and the **mapping** that moves the
resolved plan onto the read set the dataset holds.

**And the crate root is the pipeline.** `lib.rs` is **1310 lines** and holds five more jobs: the
public surface (`EngineError`, `ExecOptions`, `execute`, `execute_on`), the **scan's decision**
(`ScanDecision`, `decide_scan`, `fetched_columns`, `build_mapping`), the **join's build side**
(`BuildSide`), two **memory estimates** that are not in `memory.rs`, and the **pipeline** itself.

Seven jobs, two files, no boundaries between them.

**The review, bottom up and top down.** These are the findings, each one with the code it is in:

1. **The pipeline is one function with ten parameters.** `run_pipeline` is **176 lines** and takes
   ten arguments, and it carries `#[allow(clippy::too_many_arguments)]`. The constitution says any
   `#[allow]` must be justified in review and accumulate as an ADR under `docs/adr/`. **There is no
   ADR for it.** The allow is not a justification; it is a symptom of the function doing too much.

2. **A stage has four shapes.** The filter, the project, the evaluate and the aggregate build a
   `StageStats` inline; the join and the sort return a `*Outcome` struct whose `stage` is an
   `Option<StageStats>`; and `Ordered` is a five-field bag holding two stages' statistics and two
   stages' reports. Four idioms for one idea.

3. **The mapping is a second, unspoken language.** The plan is resolved against the **file's**
   schema, and then every index in it moves onto the **read set** the dataset holds. That movement
   is `mapping: &[Option<usize>]`, four `remap` methods, and `remap_index`, which **panics**:
   `.expect("a materialised column is in the read set")`. The invariant is established in one module
   and relied on in another, and nothing checks it at the boundary.

4. **The materialisation policy and the decision that uses it are in two modules.**
   `materialised_columns` — which columns the scan decodes — is in `plan.rs`; `decide_scan`, which
   acts on it, is in `lib.rs`. The policy's result crosses the boundary as a bare `Vec<usize>`.

5. **One module owns the other estimates, and two live elsewhere.** `memory.rs` owns the dataset's
   and the lazy read's estimates; `estimate_join` and `estimate_sort` are in `lib.rs`.

6. **The crate root holds the story and the machinery.** A reader who opens `lib.rs` to learn what
   the engine does finds the error type, the options, the scan's decision, the build side, the
   estimates and the pipeline — in that order, interleaved.

**This is not a performance claim.** Nothing here says the engine is slow. It says the code that
decides *what* to run is laid out so that changing one decision means reading five files, and so
that a ten-parameter function is the normal way to pass state around.

## The requirements

### R1 — The plan's two faces are two modules, and the bridge is a third

`plan/spec.rs` holds what the caller writes. `plan/resolved.rs` holds what the executor reads.
`plan/resolve.rs` holds the bridge between them: the name lookups, the type checks, and the
late-materialisation policy. The three modules have one direction of dependency: `resolve` knows
both faces, and neither face knows `resolve`.

> **Amended after step 3, and the reason recorded.** The requirement as first written said the
> bridge also held "the mapping that moves the resolved plan onto the read set". It does not: the
> mapping answers *where a column went*, and the scan is what moved it -- so the mapping's
> **type** is the resolved plan's own vocabulary (`plan/resolved.rs`) and the mapping itself is
> produced by the scan's decision (`scan.rs`, `ScanDecision::mapping()`). A bridge that built the
> mapping would have to know the read set before the scan had decided it. The requirement's
> intent -- one way to move an index, and no panic doing it -- holds, and R2 states it.

### R2 — The mapping is one type, with one constructor, and it does not panic

The movement of a resolved plan's indices onto the read set is **one type** with **one
constructor** and **one way to move an index**. The panic goes: an index that is not in the read
set is a plan error the caller can see, not a crash inside the engine.

### R3 — The scan's decision and what it produces are in one module

`scan.rs` holds `ScanDecision`, the decision that produces it, and the **mapping** from the file's
columns onto the read set. The **late-materialisation set** is the resolved plan's own named field,
`materialised`: the bridge produces it, and the scan reads it by that name, so the boundary
between them is a named field and not a bare vector.

> **Amended after step 3, and the reason recorded.** The requirement as first written said the
> late-materialisation **policy** would move into `scan.rs` with the decision. It does not, and it
> cannot: the policy answers *what the query reads*, and only the bridge knows that -- the bridge
> is where the plan's column names become indices. Putting the policy in `scan.rs` would make the
> bridge depend on the scan and the scan depend on the bridge, and the cycle would close. What
> crosses the boundary is the policy's **result**, as a named field of the resolved plan, which is
> what the policy was for without the cycle. The requirement's *intent* -- that the scan's view of
> the file lives in one module -- holds, and more tightly than first written: `ScanDecision` now
> has a `mapping()` method, so the read set and the mapping it implies cannot drift apart.

### R4 — The pipeline's state is a context, not ten parameters

`run_pipeline` takes a **context** — the options, the flavor, the workers, the clock, the
estimate, the build side — instead of ten arguments. `#[allow(clippy::too_many_arguments)]` is
**gone**, and no ADR is needed to justify its absence.

### R5 — A stage has one shape

Every stage — the filter, the project, the evaluate, the aggregate, the join, the sort — reports
its work in **one shape**, so that the plan output is assembled in one place and a new stage does
not have to learn three idioms.

### R6 — Every memory estimate is in one module

`memory.rs` owns **every** estimate: the dataset's, the lazy read's, the join's build side's and
the sort's working set.

### R7 — The crate root is the surface, and it tells the story

`lib.rs` holds the public surface — the error, the options, `execute`, `execute_on` — and the
crate's documentation. The machinery is in the modules that own it.

### R8 — Nothing changes that the engine does

The answer, the plan output, the memory estimates, the timings and every published figure are
**exactly what they were**. The **whole** test suite is green, the guard is green, and the
published tables are unchanged. This increment moves code; it does not change behaviour.

## The scope

**What this spec does not do, and why.**

- **The operators are not restructured.** `operators/` is already one concern per module — the
  filter, the projection, the expression, the aggregate, the sort, the join, the keys — and it is
  the **leaf** of the decomposition. A refactor that touched them would be a change of behaviour
  dressed as a change of layout.
- **`explain.rs`, `source.rs`, `dataset.rs`, `pushdown.rs`, `dictionary.rs` and `memory.rs`'s
  existing content are not touched**, except for the two estimates that move into `memory.rs`. Each
  is already one concern.
- **The `expect`s inside the operators are not touched.** They are each module's **own** invariant,
  asserted where it is established. The one that moves is `remap_index`'s, because that one is
  asserted in one module and relied on in another — that is the difference, and it is the whole of
  R2.
- **The public API does not move.** `columnar_engine::plan::JoinSpec` and the crate root's
  re-exports stay where they are, so `plan/` is a directory with the same paths inside it.
- **The optimisation order does not move.** The pinned order is a constant and it is printed in
  every plan output; changing it is a different spec.
- **No behaviour, no new dependency, no `unsafe`, and no new `#[allow]`.** A refactor that needs an
  `#[allow]` to fit has not been decomposed.

## The layout

**Bottom up.** Each layer depends only on the ones below it:

| layer | module | what it owns |
|---|---|---|
| the surface | `lib.rs` | the error, the options, `execute`, `execute_on`, the story |
| the orchestration | `pipeline.rs` | the stages, in the plan's order |
| the decisions | `scan.rs` | what to read, what to push down, the mapping, and whether the scan feeds the join |
| the bridge | `plan/resolve.rs` | names to indices, the type checks, the column mapping |
| the two faces | `plan/spec.rs`, `plan/resolved.rs` | what the caller writes; what the executor reads |
| the leaves | `operators/`, `columnar-core` | the computations; the vocabulary |

**Top down.** The plan's journey, and the module each step lives in:

```
QueryPlan            plan/spec.rs
  -> resolve         plan/resolve.rs      names to indices, type checks
  -> ResolvedPlan    plan/resolved.rs
  -> decide          scan.rs              the read set, the pushdown, the mapping
  -> read            source.rs
  -> filter          operators/filter.rs
  -> join            operators/join.rs
  -> sort            operators/sort.rs
  -> project         operators/project.rs
  -> evaluate        operators/expr.rs
  -> aggregate       operators/aggregate.rs
  -> PlanOutput      explain.rs
```

## The steps

Each step is **one commit**, and each one builds, passes the tests, is clippy-clean on arm64 and
x86-64, and leaves the published figures exactly where they were.

| # | step | what moves |
|---|---|---|
| 1 | `plan/` | `plan.rs` becomes `plan/{mod,spec,resolved,resolve}.rs` — a move, and nothing else |
| 2 | the mapping | the four `remap` siblings become one `ColumnMapping` type, and the panic goes |
| 3 | `scan.rs` | the scan's decision and the mapping it produces leave the crate root |
| 4 | `pipeline.rs` | the pipeline leaves the crate root, and its ten parameters become a context |
| 5 | one stage shape | the four stage idioms become one |
| 6 | the estimates | `estimate_join` and `estimate_sort` join the other estimates in `memory.rs` |
| 7 | the surface | the crate root keeps the surface and the story, and nothing else |

## The references

| | |
|---|---|
| [ADR-0033](../../docs/adr/0033-the-crates-modules.md) | the crate's module layout, and why it is this one |
| [constitution.md](../../constitution.md) | "Complexity requires justification": an `#[allow]` or a layout change accumulates as an ADR |
| [constitution.md](../../constitution.md) | "No implementation begins without a spec whose acceptance criteria are written and reviewable" |
| [spec 001](../001-columnar-core/spec.md) | the crate layout this one amends, and the pipeline it described |
| [`PERF.md`](../../PERF.md) | the published figures that must not move |
