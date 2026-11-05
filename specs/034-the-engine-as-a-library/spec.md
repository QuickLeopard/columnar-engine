# Spec 034 — The engine, as a library

**Status**: done
**Started**: 2026-11-04
**Finished**: 2026-11-04
**Amended**: 2026-11-04 — **R3's list**: the list is now **closed by a rule** — *every type that appears
in the public signature of a surface item is on the list* — and the list itself is written out in
**The surface, exactly**. The reason: R3's first draft named nineteen items and **missed seven that
the surface cannot do without** — `ComparisonOp`, `ScalarValue`, `ArithOp`, `MemoryError`,
`DatasetError`, `PushdownMetrics` and `Flavor` — because a list written from the plan's shape is not
the same list as the one written from the code's signatures. A predicate cannot be built without
`ComparisonOp`, a group key cannot be read without `ScalarValue`, and the plan output cannot be read
without `PushdownMetrics`. **A surface list that misses a type in a public signature is not a
surface list.**
**Amended**: 2026-11-04 — **R4's `Predicate`**: `Predicate` is a **struct**, not an enum, so
`#[non_exhaustive]` on it means the other thing — it forbids the struct literal outside its crate
and keeps `Predicate::new` the one way to build one. The reason: the code decides what the attribute
can mean, and `Predicate` is never built by a literal anywhere in the workspace, so the attribute
costs nothing and buys the same promise the enum would have.
**Amended**: 2026-11-04 — **R8's "the engine's `src/` diff is the surface's visibility and nothing
else"**: the non-exhaustive attribute puts **wildcard arms into three of the engine's own match
sites** (`plan/resolve.rs` on `Expr` and on `ScalarValue`, and `memory.rs` on `ScalarValue`), because
an enum that is not exhaustive cannot be matched exhaustively outside the crate that defines it. The
reason: the attribute is not optional, the arms **preserve today's behaviour exactly** — they are
unreachable until a variant is added, and the variant that would reach them is added in
`columnar-core`, which is the crate whose compatibility promise it is — and **no figure moves**.
**Amended**: 2026-11-04 — **the research modules are `#[doc(hidden)]`, not private**: `operators`,
`pushdown`, `dictionary`, `queries`, `explain`, `memory` and `plan` stay reachable and become
undocumented. The reason: this repository's **fourteen measurement probes**, the CLI and the
crate's own tests live in the same crates and name those modules; making them private would move the
evidence harness into a fork of itself, and a harness that forks is a harness that stops measuring
the same thing. `source` and `dataset` are **private**, as R3 says, and the two places that named
them move to the crate root and to a `testing` module.
**Amended**: 2026-11-04 — **R1's `cargo package` succeeds for the three crates**: it succeeds for
`columnar-core` today, and for `columnar-kernels` and `columnar-engine` **once their sibling is on
crates.io** — cargo resolves a packaged crate's dependencies from the registry, not from the
workspace, and no local arrangement changes that. The reason: the check verifies everything that
can be verified before the publish — each crate's manifest, each crate's **tree**, and the **publish
order computed from the manifests** rather than written down — and for the two crates that wait it
reports exactly which sibling they wait for. The publish itself is a separate act, with its own
account, and it is **not** this increment's.
**Amended**: 2026-11-04 — **the published set does not slide**: the rule that each increment's set
takes the plain names, and the last increment's set moves aside, holds for the increments that
**publish a new set of figures**. This one publishes none — the published set is the standing one, and
the re-measurement takes its own names (`compare-spec034-run*.json`,
`compare-spec034-before-run*.json`, `join-cost-spec034-run*.json` and
`join-cost-spec034-before-run*.json`). The reason: the host this increment measured on is **faster**
than the one the published set was taken on — the incumbents moved with it — so sliding the published
set forward would have published a **host's** movement as the engine's. The re-measurement is
published **beside** the published set, and the code's share of every difference is measured against
the code this increment replaces, back to back on the same host.
**Predecessor**: [033-the-drift-taken-apart](../033-the-drift-taken-apart/spec.md)

## The problem

**The engine half of a database already exists in this repository.** `columnar-engine` runs a typed
query plan over Parquet — scan, filter, project, join, sort, aggregate — with two entry points, a
memory budget, a plan output that accounts for itself, and the differential suites that say the
answer is the same whichever of the six implementations computes it. It is measured against DuckDB
and Polars on the same files with the same SQL.

**What does not exist is the library.** The engine is a research harness that measures itself, and
none of the seven things that make a library into a product are there. They are named here because
each one was **measured**, not guessed.

**And it cannot be packaged.** `cargo package -p columnar-engine` fails outright:

```
no matching package named `columnar-core` found
  location searched: crates.io index
```

`columnar-core` packages — **14 files, 151.7 KiB** — and `columnar-kernels` and `columnar-engine`
cannot, because their siblings are not on crates.io yet. The dependency direction is total and
acyclic, so the **publish order is decided by the dependency graph**: `columnar-core`, then
`columnar-kernels`, then `columnar-engine`. The `version` beside every internal `path` is already
in the manifest, so the published crate resolves on crates.io; what is missing is the act.

**And the package that does build carries the wrong README.** `readme.workspace = true` resolves to
the repository's root README, so the packaged `columnar-core` opens with *"This is **Project 02** of
the portfolio"*. There is no crate README at all — `crates/*/README.md` does not exist. A reader on
docs.rs would find the portfolio's front page where the crate's documentation should be.

**And the public surface is the harness's, not an embedder's.** `operators`, `pushdown`, `dictionary`
and `queries` are all public, because the harness and the probes live in the same crate as the
engine. The names an embedder actually needs are scattered across six modules with no `prelude`:
`execute`, `execute_on`, `QueryPlan`, `Predicate`, `Expr`, `Aggregate`, `AggregateSpec`, `JoinSpec`,
`SortSpec`, `AggregateResult`, `AggregateValue`, `Group`, `EngineError`, `PlanOutput`, `StageStats`,
`ExecOptions`, `MemoryBudget`, `Dataset` and `ParquetSource`.

**And the join's build side is a `PathBuf`.** An embedder hands a filesystem path into the plan.
There is no table, no catalog and no name — so the library cannot be told *which* table to join,
only *where* one is.

**And the enums that will grow are not `#[non_exhaustive]`.** `EngineError` gains a variant in every
increment that finds one, and `Predicate`, `Expr` and `ScalarValue` will follow. Without the
attribute, every one of those is a breaking change to the surface.

**And the package carries the whole evidence harness.** All **fourteen** measurement probes are
examples of the engine crate, so `cargo package` would ship them to crates.io. And the sibling
crates — `columnar-testdata`, `columnar-bench`, `columnar-cli`, `columnar-web` — are development
tools with no `publish = false`, so a workspace-wide publish would try to ship them too.

**And no CI job checks the package.** There is no `cargo package` in the gate and no
`cargo semver-checks`, so the first breaking change to the public surface would be discovered by an
embedder rather than by this repository.

**The question this spec takes up: the engine works — what makes it a library somebody can depend
on?**

## The requirements

### R1 — The three crates package, in the order their dependencies decide

`cargo package` succeeds for `columnar-core`, `columnar-kernels` and `columnar-engine`, and the
**publish order is core → kernels → engine**, because the dependency direction is total and acyclic.
`cargo publish --dry-run` passes for each. Every internal dependency carries a `version` beside its
`path` — it already does — so the published crate resolves on crates.io rather than in the
workspace.

### R2 — Each published crate carries its own README

`readme.workspace = true` resolves to the repository's root README today, and the packaged crate
carries the portfolio's front page. **Each published crate gets a README of its own**, written for
an embedder: what the crate is, the two entry points, a runnable example, the guarantees — NULL
never matches, the plan output accounts for the whole query, the answer does not depend on the
schedule — and the boundaries: read-only, Parquet, one inner equi-join, no SQL.

### R3 — The public surface is the facade, and it is decided before the first publish

The names an embedder needs are on **one list**, and the list is written out in **The surface,
exactly**, below. A `facade` module and a `prelude` carry them, and the crate root re-exports the
same list — the root is the surface's short path, and [ADR-0033](../../docs/adr/0033-the-crates-modules.md)
says the crate root *is* the public surface. The research modules — `operators`, `pushdown`,
`dictionary` and `queries` — are **not** part of the surface: they are reachable, because this
repository's harness lives in the same crate, and they are **undocumented**, because they are not
promised. And `source` and `dataset` are **private**: they keep only the two types the embedder
needs, re-exported at the crate root.

**After 0.1.0 the surface is a compatibility promise, so it is decided before the first publish,
not after it.** This is the one part of the increment that cannot be done twice: a published crate's
surface is the only thing in this repository that other code is allowed to depend on.

### R4 — The enums that will grow are `#[non_exhaustive]`

`EngineError`, `Predicate`, `Expr` and `ScalarValue` each gain a variant in every increment that
finds one, and a non-exhaustive enum is the only way that is not a breaking change. `EngineError`
also **gains the variants the facade needs**: an unknown table, and a table that cannot be opened.

### R5 — A table is a name, and the catalog is a set of names

A `Catalog` maps names to Parquet paths, and a plan names the tables it reads. **The shape is
already in the repository**: `TableAddress` is the harness's own decision of whether a table is a
`File` or a `Named` one, and the catalog's reference is the same decision with the name resolved.

**The facade owns the naming and the engine keeps its path**: the plan the engine receives is an
ordinary `QueryPlan`, and the names are resolved **when the plan is built** — before any data is
read. An unknown name is an error at build time, and the error names the table.

### R6 — The package carries only what an embedder needs

The **fourteen** measurement probes are excluded from the package — they are this project's evidence
harness, not an embedder's examples. `columnar-testdata`, `columnar-bench`, `columnar-cli` and
`columnar-web` are `publish = false`. The package carries no dataset, no benchmark report and no
instrument.

### R7 — CI checks the package, and the break is caught before the publish

A job runs `cargo package` for the three crates in order and `cargo semver-checks` against the last
published version. **A breaking change to the public surface fails the build, not the embedder.**

### R8 — The figures do not move

The benchmark campaigns are re-run and the published figures stand unchanged, or the change is
explained and published. **This increment changes no engine work**: the facade is a re-export, the
catalog resolves a name before the read, and the builder is compile-time only. The engine's `src/`
diff is the surface's visibility and nothing else.

### R9 — The stability policy is written down

**Frozen**: the plan, the result and the error types. **Experimental**: the plan output's rendering
and the report's text. A breaking change to the frozen surface is a **major version**, and the
policy says so in the guide an embedder reads.

### R10 — The library says what it reads, and what a hostile file can ask of it

A library is read by people who did not write it, and this one reads **files an embedder's users
provide**. So the trust boundary is published rather than implied:

- The README states that the engine reads **Parquet files**, opens **no transport**, and speaks no
  binary protocol — and it names the `thrift` advisory the dependency policy exempts, with the reason.
- **And the related weakness is published, not hidden.** The compact protocol the reader's **page
  headers** are read with allocates the declared length eagerly, bounded by a u32, so a **hostile
  file's page header can ask for four gigabytes before a byte of payload is read**. The engine does
  not fix it — the fix is upstream — but a reader who cannot see it cannot decide whether to read an
  untrusted file, and the library's whole claim is that it says what it is.

  **And the sentence is corrected where it was published, and the measurement is in
  [ADR-0049](../../docs/adr/0049-the-readers-four-gigabytes.md).** The hole is in the **page
  headers** and **not** in the metadata: the footer is read through the reader's own *slice*
  protocol, which refuses a declared length larger than the bytes that remain **before** it
  allocates, so a hostile footer asks for **8 KiB** and not for four gigabytes. **And the fix is
  `parquet`'s own reader, not `thrift` ≥ 0.23** — that limit is opt-in, `parquet` does not ask for
  it, and from **58.4.1** the reader does not read the page statistics at all, so the declared
  length is never parsed. The figure is in [`PERF.md` §3w](../../PERF.md#3w-the-readers-four-gigabytes).

The finding is in `STATUS.md`'s open questions as well, and it is **not** this increment's to fix: see
the scope.

## The surface, exactly

**This is A1 — the list, written down before anything moved.** It is the list the code carries, and
the rule that closes it is one sentence:

> **Every type that appears in the public signature of a surface item is on the list.**

A name that is not on it is not part of the surface; a name that is on it is. The list is written
here rather than in a requirement because **after 0.1.0 it is a promise**, and a promise written in
prose is a promise that drifts.

### The engine's surface — `columnar-engine`

| | the name | what it is |
|---|---|---|
| **the entry points** | `execute` | the eager path: a plan over a decoded dataset |
| | `execute_on` | the lazy path: a plan over a Parquet file, reading only what it needs |
| | `open_source` | the open helper: a path → a `ParquetSource` |
| | `open_table` | the open helper for a **named** table: a catalog and a name → a `ParquetSource` |
| **the plan** | `QueryPlan` | the plan the caller writes |
| | `Predicate` | the filter |
| | `ComparisonOp` | the predicate's operator |
| | `ScalarValue` | the predicate's threshold, and the group key's value |
| | `Expr` | the aggregate's expression |
| | `ArithOp` | the expression's operator |
| | `Aggregate` | one aggregate |
| | `AggregateSpec` | the aggregation |
| | `JoinSpec` | the join |
| | `SortSpec` | the sort |
| **the naming** | `Catalog` | the names → the Parquet paths |
| | `TableAddress` | a table's address: a file, or a name the catalog resolves |
| | `PlanBuilder` | the plan builder: a plan that names its tables |
| | `col` | the expression builder: `col("x")` |
| | `ColumnExpr` | the expression builder's column |
| | `Operand` | the expression builder's operand: a column or a constant |
| | `lit` | the expression builder's constant: `lit(1.0)` |
| **the result** | `AggregateResult` | the result of a plan |
| | `AggregateValue` | one aggregate's value |
| | `Group` | one group |
| **the errors** | `EngineError` | everything that can go wrong |
| | `MemoryError` | why a plan did not fit its budget |
| | `DatasetError` | why a file could not be read |
| **the plan output** | `PlanOutput` | the rendered plan |
| | `StageStats` | one stage's statistics |
| | `JoinReport` | the join's report |
| | `SortReport` | the sort's report |
| | `Attribution` | the query's own time, and how the stages divide it |
| | `PushdownMetrics` | what the pushed-down predicates cost |
| **the knobs** | `ExecOptions` | the knobs: the budget, the flavor, the threads |
| | `MemoryBudget` | the per-query memory budget |
| | `Flavor` | the kernel flavor |
| **the vocabulary** | `DataType` | the engine's type for a column |
| | `ColumnChunk` | a column's values, its encoding and its validity |
| | `Encoding` | how a column's values are stored |
| | `NullBitmap` | which of a column's values are NULL |
| **the data** | `Dataset` | the decoded columns, in memory |
| | `ParquetSource` | the file |
| | `FileSetSource` | the set: a table that is **many files**, read as one table |
| | `SourceColumn` | a column of the file: its name and the engine's type for it |
| | `RowGroupRange` | a row group's rows, and the range of a column's values in it |
| | `MAX_READER_BATCH_ROWS` | the reader's batch size, in rows |

**Forty-five names, and every one of them is reachable from `columnar_engine::prelude::*` and from
the crate root.** The list is the crate's rustdoc, the crate's README and the embedder test's
imports — one list, read three times, in three places that must agree.

### What is not on the list, and why

| | |
|---|---|
| `operators`, `pushdown`, `dictionary`, `queries`, `explain`, `memory`, `plan` | **Reachable, undocumented.** They are this repository's measurement harness — the fourteen probes live in this crate — and a harness that cannot reach the engine's internals measures nothing. `#[doc(hidden)]` says *reachable, not promised*. |
| `source`, `dataset` | **Private.** They keep only the two types the embedder needs, and the crate root re-exports them. |
| `testing` | **Reachable, undocumented.** The three decode counters the crate's own tests read. They are instrumentation, not an API. |
| `columnar-core`'s own modules | **Out of scope for this spec.** The vocabulary crate's root is already flat — every type it has is re-exported at its root — and R3 is about the engine's surface. |

### The two paths to a name

The crate root and the prelude carry the same list, and the reason there are two is the one the
house already decided: [ADR-0033](../../docs/adr/0033-the-crates-modules.md) — *`lib.rs` is the
public surface*. The root is where an embedder writes `columnar_engine::QueryPlan`, and the prelude
is where an embedder writes `use columnar_engine::prelude::*;`. **No research module offers a third
path to a surface name**, and the ones that do — `plan::QueryPlan`, `operators::aggregate::AggregateResult` —
are the undocumented ones, which is why they are not a promise.

## The plan's shape

**A plan is a set of functions and the order they can be done in.** This one is decomposed here,
function by function, and the decomposition is checked against the phases below — because a phase
with no function would be work with no description, and a function with no phase would be work with
no place.

| | the function | where it lands | the phase |
|---|---|---|---|
| **A1** | the surface list — the names that are public, written down before anything moves | this spec | 1 |
| **A2** | the module layout — a `facade` module and a `prelude`; the research modules made private or `#[doc(hidden)]` | `crates/columnar-engine/src/lib.rs` | 1 |
| **A3** | the non-exhaustive enums, and the new error variants | `crates/columnar-engine/src/lib.rs`, `crates/columnar-core/src/` | 1 |
| **A4** | the entry points — `execute`, `execute_on`, and the open helper an embedder needs | `crates/columnar-engine/src/facade.rs` | 1 |
| **B1** | the `Catalog` — names to Parquet paths | `crates/columnar-engine/src/facade.rs` | 2 |
| **B2** | the plan builder — a plan that names its tables | `crates/columnar-engine/src/facade.rs` | 2 |
| **B3** | the resolution — a name to a path, at build time, before any data is read | `crates/columnar-engine/src/facade.rs` | 2 |
| **C1** | the expression builder — `col("x").gt(3)` | `crates/columnar-engine/src/facade.rs` | 3 |
| **C2** | the error messages an embedder shows a user | `crates/columnar-engine/src/facade.rs` | 3 |
| **D1** | the crate READMEs — one per published crate, written for an embedder | `crates/*/README.md` | 4 |
| **D2** | the rustdoc — the crate-level doc, with a runnable example | `crates/*/src/lib.rs` | 4 |
| **D3** | the embedding guide — how to open, plan, run, read, and what is frozen | `docs/embedding.md` | 4 |
| **E1** | the manifests — per-crate metadata, `publish = false` on the dev crates, the probes excluded | `crates/*/Cargo.toml` | 5 |
| **E2** | the package check — `cargo package` for the three crates, in order, and the published trees inspected | `scripts/check-package.sh` | 5 |
| **E3** | the CI job — the package check and `cargo semver-checks` | `.github/workflows/ci.yml` | 5 |
| **E4** | the publish order — the documented sequence and the commands | `docs/embedding.md` | 5 |
| **F1** | the embedder test — an integration test that uses only the facade | `crates/columnar-engine/tests/embed.rs` | 6 |
| **F2** | the benchmark re-run — the campaigns, and the figures unchanged or explained | `bench-results/` | 6 |
| **F3** | the ADR and the publication | `docs/adr/0040-*.md`, `PERF.md`, `README.md`, `STATUS.md` | 6 |
| **G1** | the stability policy — what is frozen, and how a break is signalled | `docs/embedding.md` | 7 |
| **G2** | the closure — the spec closed, the status file | this spec | 7 |

**And every function carries the phase it is done in.** The column is the phases' own numbering, so
the check is not a claim about the plan — it is the plan, read against the phases.

| from | to | why |
|---|---|---|
| **A1** | **A2** | the module layout follows the surface list |
| **A1** | **A3** | the enums that grow are the ones on the surface |
| **A1** | **A4** | the entry points are the surface's |
| **A1** | **E1** | the manifest's metadata names the surface |
| **A2** | **B1** | the catalog is a facade type |
| **A2** | **B2** | the builder is a facade type |
| **A2** | **C1** | the expression builder is a facade type |
| **A2** | **D1** | the README names the public surface |
| **A2** | **D2** | the rustdoc documents the facade |
| **A2** | **F1** | the embedder test uses only the facade |
| **A3** | **B3** | the resolution reports the new error variants |
| **A4** | **D2** | the rustdoc's example calls the entry points |
| **A4** | **F1** | the embedder test calls the entry points |
| **B1** | **B2** | the builder resolves names in the catalog |
| **B1** | **B3** | the resolution looks the name up in the catalog |
| **B2** | **B3** | the builder hands the resolved plan to the resolution |
| **B3** | **F1** | the embedder test exercises the resolution |
| **B3** | **F3** | the ADR records the resolution's decision |
| **C1** | **D3** | the guide shows the expression builder |
| **C1** | **F1** | the embedder test uses the expression builder |
| **C2** | **F1** | the embedder test checks the error text |
| **D1** | **E2** | the package check inspects the packaged README |
| **D2** | **D3** | the guide follows the rustdoc's example |
| **D3** | **E4** | the publish order is in the guide |
| **E1** | **E2** | the package check runs on the manifests |
| **E2** | **E3** | the CI job runs the package check |
| **E2** | **E4** | the publish order follows what packages |
| **E3** | **G1** | the policy is what the CI job enforces |
| **F1** | **F3** | the ADR is written from the evidence |
| **F2** | **F3** | the ADR records that the figures did not move |
| **G1** | **G2** | the closure follows the policy |

**And the graph is acyclic.** No edge runs backwards: the phases are a total order and every
cross-phase edge runs forward, and within a phase the edges run in the order the functions are
listed. Every function has a phase and every phase has functions.

**And the phases are:**

1. **The contract.** The surface list, the module layout, the non-exhaustive enums, the entry
   points. Nothing else moves until this is written down, because everything else depends on it.
2. **The naming.** The catalog, the builder, the resolution.
3. **The ergonomics.** The expression builder and the error messages.
4. **The documentation.** The READMEs, the rustdoc, the guide.
5. **The distribution.** The manifests, the package check, the CI job, the publish order.
6. **The evidence.** The embedder test, the benchmark re-run, the ADR and the publication.
7. **The closure.** The stability policy and the spec's close.

## The scope

Stated explicitly, with the reason (Principle VII):

| Excluded | Reason |
|---|---|
| A SQL parser | Spec 001 declined it: *"a month of work that produces no evidence"*. The facade's builder is the interface; a parser is its own increment, and it wants its own measurement. |
| A write path | The engine is read-only over Parquet. A write path is a storage engine, and it would be measured against nothing this project compares against. |
| A server, a wire protocol, a client | The product is a library. |
| Transactions, MVCC, a catalog on disk | The catalog is in memory and the embedder owns it. |
| Outer joins, more than two tables, `AVG`, window functions | Each is a new operator, and each wants its own measurement before it is believed. |
| The engine's own work | This increment changes none of it. R8 holds the figures still. |
| **A `parquet` bump** | The fix for the compact protocol's eager allocation is upstream — `thrift` ≥ 0.23 — and `parquet` 54 pins `thrift = "0.17"`, so the fix is a `parquet` bump. **And a `parquet` bump moves the published figures**, because Arrow's decode is measured. It is its own increment with its own measurement, and R10 publishes the finding meanwhile. |
| **The measurement track's next question — the drift's own parts** | **Deferred by this increment, and the reason is recorded here**: this increment changes no engine work, so every published figure must stand unchanged, and the two tracks do not touch the same code. The drift's parts are still the next question on that track, and this increment does not answer it. |

## The measurement

**The acceptance is a list, and every item on it is checked by a command.**

1. `cargo package` succeeds for the three crates, **in the order their dependencies decide**, and
   `cargo publish --dry-run` passes for each.
2. The packaged tree of each published crate holds its **own** README, its `src/`, and its licence
   files — and **no dataset, no report, no probe**.
3. An integration test that imports **only** the facade compiles and passes. It is the surface's
   test, and it is written as an embedder would write it.
4. `cargo semver-checks` passes in CI against the last published version.
5. **The published figures do not move** — the same campaigns, the same medians, or a change
   explained and published.
6. The whole suite green: `cargo fmt`, clippy on arm64 and x86-64, the tests, `cargo deny`, the
   wasm32 check, the published-figures guard and the page checks.

**And the campaigns were taken again, on both codes, and the figures stand.** The comparison and the
join probe were re-run on the tree this increment ships and — **back to back, on this host** — on the
tree it replaces, a worktree at `79ec08f`: three campaigns each, both trees **clean**, the two codes
**interleaved**, so that a host which drifts moves both sides. **The code's share of every difference
is nil** — the join's cold stage reads **18.366 ms** against the old code's **18.455 ms**, a difference
of **−0.089 ms** against a drift of **1.237 ms**, and the cold `join-orders` query reads **82.0 ms**
against **82.5 ms**.

**And the movement between the published set and the re-measurement is the host's.** The incumbents,
which this increment cannot have touched, moved with it — DuckDB's `join-orders` cold from **91.0 ms**
to **85.0 ms**. So the published figures stand where spec 033 published them, the re-measurement is
published **beside** them under its own names, and every figure — old and new — is derived from a
committed report: [`PERF.md` §2i](../../PERF.md#2i-the-same-code-measured-again) and
[§3p](../../PERF.md#3p-the-joins-stage-measured-again).

## The references

| | |
|---|---|
| [constitution.md](../../constitution.md) | **I. One Predicate, Six Implementations, One Answer** — the differential suites keep passing, and the answer does not depend on the schedule |
| [constitution.md](../../constitution.md) | **IV. Evidence Over Claims** — R8: the figures do not move, and the reason is published if they do |
| [constitution.md](../../constitution.md) | **V. The Comparison Is Reproducible by a Stranger** — R2 and D3: an embedder is a stranger, and the guide is written for one |
| [constitution.md](../../constitution.md) | **"Definition of done": the benchmark re-run and the baseline updated deliberately, with the change explained** — R8 and F2 |
| [constitution.md](../../constitution.md) | **VII. Explicit Scope, No Silent Deferral** — the scope's exclusions, each with its reason, including the deferred question |
| [constitution.md](../../constitution.md) | **"Complexity requires justification"** — the facade, the catalog and the builder |
| [ADR-0005](../../docs/adr/0005-own-column-chunks.md) | the engine's own column representation, and Parquet as the I/O boundary |
| [ADR-0024](../../docs/adr/0024-the-repository-goes-public.md) | the repository goes public, and what that means for what it may contain |
| [ADR-0033](../../docs/adr/0033-the-crates-modules.md) | the crate's modules — `lib.rs` is *the public surface*, and R3 decides what that surface is |
| [ADR-0039](../../docs/adr/0039-the-drift-taken-apart.md) | the drift, taken apart — the yardstick this increment's figures are read against |
| [spec 001](../001-columnar-core/spec.md) | the scope, the pipeline, and the exclusions this spec inherits |
| [spec 033](../033-the-drift-taken-apart/spec.md) | the drift, the yardstick, and the deferred question |
| [`PERF.md`](../../PERF.md) §3o | the figures R8 says must stand |
| [`PERF.md`](../../PERF.md) §2i, §3p | the re-measurement: the same code, and the code it replaces, back to back |
| [`STATUS.md`](../../STATUS.md) | where the project's open questions live |
