# ADR-0040 — The engine, as a library

**Status**: accepted
**Date**: 2026-11-04
**Spec**: [spec 034](../../specs/034-the-engine-as-a-library/spec.md)
**Predecessor**: [ADR-0039](0039-the-drift-taken-apart.md)

## Context

The engine is measured. Its kernels are compared against six implementations, its operators against
Arrow's, its pipeline against DuckDB's and Polars', and its own drift against itself — and every one
of those numbers is published, with the raw report beside it.

**And none of it is a library.** The engine lives inside a research harness: the fourteen measurement
probes are the crate's own examples, the crate's own tests reach into its internals, and the public
surface is whatever the internals happen to expose. There is no `facade`, no `prelude`, no
`Catalog`, no way to name a table without writing a path into the plan, and no way to write a
predicate without naming a `ComparisonOp` and a `ScalarValue` by hand.

**And the crates do not ship.** `cargo package -p columnar-engine` fails outright: the crate
depends on `columnar-core` and `columnar-kernels`, and cargo resolves a packaged crate's
dependencies from crates.io, not from the workspace. `columnar-core` packages — fourteen files,
151.7 KiB — and the other two cannot, because their siblings are not on crates.io yet. The
dependency direction is total and acyclic, so the **publish order is decided by the graph**:
`columnar-core`, then `columnar-kernels`, then `columnar-engine`.

**And the crate that does package carries the wrong front page.** The packaged `columnar-core` has
the *repository's* README in it — the portfolio's front page, not the crate's — and no licence file
of its own.

**And a published crate's surface is a promise.** After 0.1.0 the surface is the only thing in this
repository that other code is allowed to depend on, and it is the one part of the increment that
cannot be done twice: a surface published and then changed is a break in every embedder's build.

## Decision

### 1. The surface is decided before the first publish, and it is written down

The list is in [spec 034](../../specs/034-the-engine-as-a-library/spec.md)'s **The surface, exactly**,
and it is closed by one rule: **every type that appears in the public signature of a surface item is
on the list**. A name that is not on it is not part of the surface; a name that is on it is.

**Forty-one names** are the engine's own, and **four more** are `columnar-core`'s vocabulary — the
`DataType`, `ColumnChunk`, `Encoding` and `NullBitmap` that the plan's types are made of. The list is
the crate's rustdoc, the crate's README and the embedder test's imports: one list, read three
times, in three places that must agree.

**And the list is closed by a rule rather than by taste, because the first draft of it was wrong.**
R3 named nineteen items and missed seven the surface cannot do without — `ComparisonOp`,
`ScalarValue`, `ArithOp`, `MemoryError`, `DatasetError`, `PushdownMetrics` and `Flavor` — because a
list written from the plan's shape is not the list written from the code's signatures. A predicate
cannot be built without `ComparisonOp`; a group key cannot be read without `ScalarValue`; the plan
output cannot be read without `PushdownMetrics`.

### 2. The facade owns the naming, and the engine keeps its path

A `Catalog` maps names to Parquet paths, and it is the **embedder's**: the engine never reads one
from disk and never mutates one. The plan builder holds a **reference** to the table it was given —
a name, or a path — and resolves it **when the plan is built**, before any data is read. The plan the
engine receives is an ordinary `QueryPlan`, with a path where the name was.

**So the engine's `src/` diff is the surface's visibility and nothing else.** The builder is
compile-time only; the catalog resolves a name; the engine's operators never learn a name existed.

**And the reference is the repository's own shape.** `TableAddress` — a file, or a name — was the
harness's decision of the same question, and the catalog's reference is that decision with the name
resolved. The type moved to the facade, because the facade owns the naming.

### 3. The types that will grow are closed for construction

`EngineError`, `Expr` and `ScalarValue` are `#[non_exhaustive]` enums, and `Predicate` is a
`#[non_exhaustive]` struct. A type this general grows, and the marking is what makes the growth
cheap: **adding a variant to an enum, or a field to the struct, is not a breaking change**, while
removing one, or changing a field's type or its meaning, is.

**`EngineError` gained the two variants the naming layer needs** — `UnknownTable`, for a name the
catalog does not hold, and `TableUnreadable`, for a table that could not be opened — and both are
**written for a user**: the error names the table, lists the names the catalog does hold, and says
what to do about it.

**And the attribute has a price that is paid in the engine's own code.** An enum that is not
exhaustive cannot be matched exhaustively outside the crate that defines it, so three of the engine's
match sites gained a wildcard arm. The arms **preserve today's behaviour exactly** — they are
unreachable until a variant is added, and the variant that would reach them is added in
`columnar-core`, which is the crate whose promise it is.

### 4. The research modules are reachable and undocumented

`operators`, `pushdown`, `dictionary`, `queries`, `explain`, `memory` and `plan` are
`#[doc(hidden)]`: reachable, because this repository's **fourteen measurement probes**, its CLI and
its own tests live in the same crates and name them, and **not promised**, because they are not
documented and not in the prelude.

**A harness that cannot reach the engine's internals measures nothing**, and a harness that forks
is a harness that stops measuring the same thing. The probes are the evidence, and the evidence
needs the internals.

### 5. The three crates package in the order their dependencies decide — and the order is computed

`scripts/check-package.sh` reads every internal dependency out of the manifests and takes the
topological sort of them, so the order is a **fact about the graph** rather than a line in a script.
A cycle, or a crate that depends on something outside the published set, fails the check.

**And the check inspects what each package would carry**: its **own** README — not the
repository's — its `src/`, and a licence file; and **no probe, no dataset, no report**. The
fourteen probes are excluded from the package, and the development crates are `publish = false`.

**And the two crates that wait, wait for a named reason.** `cargo package` succeeds for
`columnar-core` today; for `columnar-kernels` and `columnar-engine` it succeeds **once their
sibling is on crates.io**, and the check says which sibling each of them waits for. The publish
itself is a separate act, with its own account, and it is not this increment's.

### 6. A break to the public surface fails this build, not the embedder

The CI job runs `cargo semver-checks` against the last **published** version of the crate — the
version an embedder depends on, and the one that must not break. The three crates are not published
yet, and there is no published version to compare against, so the recorded baseline — the revision
that established the surface — stands in for it, and the job says which baseline it used.

**And the criterion is the frozen surface, not the whole crate.** The plan, the result and the error
types are **frozen**: a break to them is a **MAJOR** version. The plan output's rendering and the
report's text are **experimental**: they are measurements and they move, and a reader should read
them rather than parse them.

### 7. The trust boundary is published, not implied

The engine reads **files an embedder's users provide**. It opens **no transport** and speaks **no
binary protocol**, and the one dependency-policy exemption it carries — the `thrift` advisory, whose
sink is the binary protocol `parquet` never constructs — is published with its reason in
`deny.toml`.

**And the related weakness is published too, rather than hidden.** The compact protocol the reader
*does* use allocates the declared length eagerly, bounded by a `u32`, so a hostile Parquet file's
metadata can ask for roughly four gigabytes before a byte of payload is read. That path **is**
reachable. The engine does not fix it: the fix is upstream (`thrift` ≥ 0.23), `parquet` 54 pins
`thrift = "0.17"`, and so the fix is a **`parquet` bump** — which is deliberately **not** this
increment's, because a `parquet` bump moves the published figures and wants its own measurement.
**A reader who cannot see the weakness cannot decide whether to read an untrusted file.**

## Consequences

**The engine's work did not change.** The `src/` diff is the surface's visibility, three wildcard
arms that no variant can reach today, and two **widenings** — `open_source` and
`Dataset::read_parquet` now take anything path-like, so an embedder with a `&str` does not have to
write `Path::new`. No operator, no kernel and no measurement changes.

**And the answer did not change.** The whole test suite passes, including the differential kernel
suite that keeps the six implementations honest, and the **embedder test** —
`crates/columnar-engine/tests/embed.rs` — imports **only** the facade and is written the way an
embedder would write it. It is the surface's own test: if it needs something the surface does not
carry, the surface is wrong.

**And the published figures stand, and the reason they stand is that the code did not change.** The
campaigns were re-run on the tree this increment ships and — **back to back, on this host** — on the
tree it replaces, a worktree at `79ec08f`; three campaigns each, both trees **clean**, the two codes
interleaved. **The code's share of every difference is nil**: the join's cold stage reads **18.366 ms**
against the old code's **18.455 ms**, a difference of **−0.089 ms** against a drift of **1.237 ms**, and
the cold `join-orders` query reads **82.0 ms** against **82.5 ms**.

**And the movement between the published set and the re-measurement is the host's, not the engine's.**
The incumbents, which this increment cannot have touched, moved with it: DuckDB's `join-orders` cold
went from **91.0 ms** to **85.0 ms**, Polars' from **219.1 ms** to **197.5 ms**. So the figures stand
where spec 033 published them, the re-measurement is published **beside** them under its own names, and
every figure — old and new — is derivable from a committed report.

**And a set that publishes no new figures does not take the plain names.** The rule that each
increment's set takes the plain names and the last one's moves aside holds for the increments that
**publish a new set of figures**. This one publishes none: the published set is the standing one, and
the re-measurement takes its own names — `compare-spec034-run*.json`,
`compare-spec034-before-run*.json`, `join-cost-spec034-run*.json` and
`join-cost-spec034-before-run*.json`. Sliding the published set forward would have published a
**host's** movement as the engine's, and the reason is recorded in the spec's header.

**And the crate is now a product.** Each published crate carries its own README, written for an
embedder; the crate-level rustdoc carries a **runnable** example, and the doctest runs in CI
(`cargo test --all-targets` does not run it); and `docs/embedding.md` says how to open a file, build
a plan, run it, read the plan output, set the budget and the threads — and **what is frozen**.

**And the questions this leaves are written down where the last one was** — the README's Next row
and [`STATUS.md`](../../STATUS.md)'s last section. The first is the one this increment could not
close: **the three crates are ready to publish and the publish is a separate act** — it needs a
crates.io account, a token, and the three names, and it makes the code public. The second is the
one that was deferred: **the drift's own parts**, which the measurement track's next question was,
and which this increment pushed back by one so the surface could be decided before the first publish.

## References

- [spec 034](../../specs/034-the-engine-as-a-library/spec.md) — the requirements, the surface list and
  the amendments whose reasons are recorded in the spec.
- [ADR-0039](0039-the-drift-taken-apart.md) — the drift, the yardstick and the published figures this
  increment re-measured.
- [ADR-0033](../../docs/adr/0033-the-crates-modules.md) — `lib.rs` is the public surface, and the
  crate root's re-exports are the surface's short path.
- [ADR-0005](../../docs/adr/0005-own-column-chunks.md) — the engine owns its column chunks, and it
  reads Parquet rather than a transport.
- [`PERF.md`](../../PERF.md) — the figures the campaigns re-measured.
- [`docs/embedding.md`](../../docs/embedding.md) — the guide an embedder reads.
