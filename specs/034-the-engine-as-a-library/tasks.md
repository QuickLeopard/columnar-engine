# Spec 034 — The engine, as a library — tasks

**Status**: in progress
**Spec**: [spec.md](spec.md)

## Phase 1 — The contract

- [ ] The **surface list** is written down: the exact names that are public, and the research
      modules that are not. It is written **before** anything moves, because after 0.1.0 the surface
      is a compatibility promise.
- [ ] The **module layout** follows it: a `facade` module and a `prelude`, and `operators`,
      `pushdown`, `source`, `dictionary`, `queries` and `plan::resolve` made private or
      `#[doc(hidden)]`.
- [ ] The **enums that will grow** are `#[non_exhaustive]`: `EngineError`, `Predicate`, `Expr` and
      `ScalarValue`. And `EngineError` **gains** the variants the facade needs — an unknown table,
      and a table that cannot be opened.
- [ ] The **entry points** are on the facade: `execute`, `execute_on`, and the open helper an
      embedder needs to get from a path to a source.
- [ ] Every name on the surface list is reachable from the `prelude` and from **nothing else**.

## Phase 2 — The naming

- [ ] The **`Catalog`** maps names to Parquet paths, and it is the embedder's.
- [ ] The **plan builder** names its tables, and builds an ordinary `QueryPlan` underneath — **the
      facade owns the naming and the engine keeps its path**.
- [ ] The **resolution** happens **when the plan is built**, before any data is read, and an unknown
      name is an error that **names the table**.

## Phase 3 — The ergonomics

- [ ] The **expression builder** — `col("x").gt(3)` — covers the predicate and the aggregate
      expressions, and it is the way the guide shows a plan being written.
- [ ] The **error messages** an embedder shows a user are written for a user: what went wrong, and
      what to do about it.

## Phase 4 — The documentation

- [ ] Each **published crate carries its own README**, written for an embedder — what it is, the two
      entry points, a runnable example, the guarantees and the boundaries. The repository's root
      README is **not** the crate's README.
- [ ] The **rustdoc** carries a **runnable example**, and the doctest runs in CI.
- [ ] The **embedding guide** — `docs/embedding.md` — says how to open a file, build a plan, run it,
      read the plan output, set the budget and the threads, and **what is frozen**.

## Phase 5 — The distribution

- [ ] The **manifests** carry per-crate metadata, `columnar-testdata`, `columnar-bench`,
      `columnar-cli` and `columnar-web` are `publish = false`, and the **fourteen probes** are
      excluded from the package.
- [ ] The **package check** runs `cargo package` for the three crates **in the order their
      dependencies decide**, and inspects each published tree: its own README, its `src/`, and **no
      dataset, no report, no probe**.
- [ ] The **CI job** runs the package check and `cargo semver-checks` against the last published
      version, so a break fails the build and not the embedder.
- [ ] The **publish order** and the commands are written down in the guide.

## Phase 6 — The evidence

- [ ] The **embedder test** — `crates/columnar-engine/tests/embed.rs` — imports **only** the facade
      and is written the way an embedder would write it. It is the surface's test.
- [ ] The **benchmark campaigns are re-run** and the published figures **stand unchanged**, or the
      change is explained and published old-and-new.
- [ ] The **ADR** records the decision, and the publication names it — `PERF.md`, the README's Next
      row and `STATUS.md`'s last section — and the raw reports are committed.

## Phase 7 — The closure

- [ ] The **stability policy** is written down: the plan, the result and the error types are
      **frozen**; the plan output's rendering and the report's text are **experimental**; a break to
      the frozen surface is a **major version**.
- [ ] The spec's status is `done` with the date it finished, and CI is green on the publish commit.

## The gate

- [ ] `cargo fmt`, clippy on arm64 and x86-64, the whole test suite, `cargo deny`, the wasm32 check,
      the published-figures guard and the page checks all green.
- [ ] `cargo package` green for the three crates, in order, and `cargo publish --dry-run` green for
      each.
- [ ] CI green, and the spec's status is `done` with the date it finished.
