# Spec 034 — The engine, as a library

**Status**: in progress
**Started**: 2026-11-04
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

The names an embedder needs are on **one list**: `execute`, `execute_on`, `QueryPlan`, `Predicate`,
`Expr`, `Aggregate`, `AggregateSpec`, `JoinSpec`, `SortSpec`, `AggregateResult`, `AggregateValue`,
`Group`, `EngineError`, `PlanOutput`, `StageStats`, `ExecOptions`, `MemoryBudget`, `Dataset` and
`ParquetSource`. A `facade` module and a `prelude` carry them. The research modules — `operators`,
`pushdown`, `dictionary` and `queries` — are **not** part of the surface, and `source` and `dataset`
keep only the two types the embedder needs, re-exported at the crate root.

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
- **And the related weakness is published, not hidden.** The compact protocol the reader does use
  allocates the declared length eagerly, bounded by a u32, so a **hostile file's metadata can ask for
  four gigabytes before a byte of payload is read**. The engine does not fix it — the fix is
  upstream — but a reader who cannot see it cannot decide whether to read an untrusted file, and the
  library's whole claim is that it says what it is.

The finding is in `STATUS.md`'s open questions as well, and it is **not** this increment's to fix: see
the scope.

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
| [`STATUS.md`](../../STATUS.md) | where the project's open questions live |
