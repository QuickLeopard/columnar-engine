# Spec 034 — The engine, as a library — tasks

**Status**: in progress
**Spec**: [spec.md](spec.md)

## Phase 1 — The contract

- [x] The **surface list** is written down: the exact names that are public, and the research
      modules that are not. It is written **before** anything moves, because after 0.1.0 the surface
      is a compatibility promise.
- [x] The **module layout** follows it: a `facade` module and a `prelude`, and `operators`,
      `pushdown`, `source`, `dictionary`, `queries` and `plan::resolve` made private or
      `#[doc(hidden)]`.
- [x] The **enums that will grow** are `#[non_exhaustive]`: `EngineError`, `Predicate`, `Expr` and
      `ScalarValue`. And `EngineError` **gains** the variants the facade needs — an unknown table,
      and a table that cannot be opened.
- [x] The **entry points** are on the facade: `execute`, `execute_on`, and the open helper an
      embedder needs to get from a path to a source.
- [x] Every name on the surface list is reachable from the `prelude` and from **nothing else**.

**Amended**: the last item — the crate root re-exports the same list as the prelude, because
[ADR-0033](../../docs/adr/0033-the-crates-modules.md) says the crate root *is* the public surface.
The reason is recorded in the spec's header.

## Phase 2 — the naming

- [x] The **`Catalog`** maps names to Parquet paths, and it is the embedder's.
- [x] The **plan builder** names its tables, and builds an ordinary `QueryPlan` underneath — **the
      facade owns the naming and the engine keeps its path**.
- [x] The **resolution** happens **when the plan is built**, before any data is read, and an unknown
      name is an error that **names the table**.

## Phase 3 — the ergonomics

- [x] The **expression builder** — `col("x").gt(3)` — covers the predicate and the aggregate
      expressions, and it is the way the guide shows a plan being written.
- [x] The **error messages** an embedder shows a user are written for a user: what went wrong, and
      what to do about it.

## Phase 4 — the documentation

- [x] Each **published crate carries its own README**, written for an embedder — what it is, the two
      entry points, a runnable example, the guarantees and the boundaries. The repository's root
      README is **not** the crate's README.
- [x] The **rustdoc** carries a **runnable example**, and the doctest runs in CI.
- [x] The **embedding guide** — `docs/embedding.md` — says how to open a file, build a plan, run it,
      read the plan output, set the budget and the threads, and **what is frozen**.

## Phase 5 — the distribution

- [x] The **manifests** carry per-crate metadata, `columnar-testdata`, `columnar-bench`,
      `columnar-cli` and `columnar-web` are `publish = false`, and the **fourteen probes** are
      excluded from the package.
- [x] The **package check** runs `cargo package` for the three crates **in the order their
      dependencies decide**, and inspects each published tree: its own README, its `src/`, and **no
      dataset, no report, no probe**.
- [x] The **CI job** runs the package check and `cargo semver-checks` against the last published
      version, so a break fails the build and not the embedder.
- [x] The **publish order** and the commands are written down in the guide.

**Amended**: the package check — `cargo package` succeeds for `columnar-core` today, and for the
other two once their sibling is on crates.io. The reason is recorded in the spec's header, and the
check says which sibling each crate waits for.

## Phase 6 — the evidence

- [x] The **embedder test** — `crates/columnar-engine/tests/embed.rs` — imports **only** the facade
      and is written the way an embedder would write it. It is the surface's test.
- [ ] The **benchmark campaigns are re-run** and the published figures **stand unchanged**, or the
      change is explained and published old-and-new.
- [ ] The **ADR** records the decision, and the publication names it — `PERF.md`, the README's Next
      row and `STATUS.md`'s last section — and the raw reports are committed.

## Phase 7 — the closure

- [x] The **stability policy** is written down: the plan, the result and the error types are
      **frozen**; the plan output's rendering and the report's text are **experimental**; a break to
      the frozen surface is a **major version**.
- [ ] The spec's status is `done` with the date it finished, and CI is green on the publish commit.

## The gate

- [ ] `cargo fmt`, clippy on arm64 and x86-64, the whole test suite, `cargo deny`, the wasm32 check,
      the published-figures guard and the page checks all green.
- [ ] `cargo package` green for the three crates, in order, and `cargo publish --dry-run` green for
      each.
- [ ] CI green, and the spec's status is `done` with the date it finished.
