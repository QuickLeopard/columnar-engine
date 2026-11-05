# ADR-0048 — The composition is a test, and the edge it needs is test-only

**Status**: accepted
**Date**: 2026-11-05
**Spec**: [040-the-source-that-reads-a-set-of-files](../../specs/040-the-source-that-reads-a-set-of-files/spec.md)

## Context

**The store's half and the engine's half were each tested, and the composition was not.**

The store's tests say what the store writes: the file the generator wrote, byte for byte; the
manifest; the commit; the grid; the table's paths. The engine's tests say what the engine reads
from a set of files it was handed: the rows, the order, the predicate, the statistics, the
schemas. **And the claim that ties them together -- the engine reads what the store wrote, as one
table -- was checked by hand, in a throwaway crate outside the workspace, and was not in the
suite.**

**The hand check was wrong the first time, and the way it was wrong is the reason this ADR
exists.** The first assertion compared the set's filtered count against the count over *one* of
the two files, and expected them to be equal. **They cannot be**: the set holds both files' rows
and the one file holds half, so the counts differ by exactly two. The code was right and the
assertion was wrong -- but the failure was in the only place the composition was ever checked,
and nothing in the tree would have caught it if the code had been wrong too.

**So the claim needs a test, and the test needs a dependency edge the design does not want.**

The engine must never depend on the store -- the glue is a list of paths, and that is the whole
of the coupling. A test that writes through the store and reads through the engine cannot live in
the engine's crate without inverting that. **The store's crate is the other side of it**: the
store is below the engine, and its own test suite is where a claim about the store's *output* --
the files it wrote, read by the reader they were written for -- belongs.

## Decision

1. **The composition is a test in the store's crate.** `crates/columnar-store/tests/glue.rs`
   writes a table of two commits, takes the table's paths, opens the set with the engine's
   `FileSetSource`, and says three things: the set holds the table's rows **row by row, in the
   files' order**; the engine's predicate reaches **every** file, so the count over the set is
   the files' counts added up; and the **sum** over the set is the files' sums added up, so the
   values are read and not merely counted.

2. **The edge is a dev-dependency, and it is path-only.** The store's *library* still depends on
   nothing in this workspace, and the engine still never depends on the store in any section.
   **The path-only form is the same rule ADR-0047 found:** a path-only dependency is **dropped**
   from the published manifest, and a dependency with a version is not -- so a versioned
   dev-dependency on the engine would make `cargo package -p columnar-store` wait for the engine
   to be on crates.io, and the publish is a separate act that has not happened.

3. **The two sentences that said the store depends on nothing are corrected where they were
   published.** The root manifest's dependency map and the store's own manifest said the store
   "depends on no other crate in this workspace". **The library does; the test does not.** Both
   now say so, and the reason is in each.

## Consequences

- **The claim is in the suite.** "The engine reads what the store wrote, as one table" is no
  longer a habit and a hand check: it is three tests that fail if the set reads one file instead
  of the table, which is what they were checked against -- the mutation was run, and all three
  fail.
- **The published manifest is unchanged.** The path-only dev-dependency is dropped, so
  `cargo package -p columnar-store` does not wait for the engine on crates.io, and the crate's
  published dependency list is the one ADR-0047 published.
- **The store's test job builds the engine.** The store's tests now compile the engine and its
  kernels. That is the price of the composition being checked where it is claimed.
- **The store's crate is still below the engine.** The edge is test-only and the direction of the
  *library* graph is unchanged: the store writes, the engine reads, and the glue is a list of
  paths.

## References

- [`constitution.md`](../../constitution.md) IV — evidence over claims: a correctness claim is
  accompanied by the thing that checks it.
- [`constitution.md`](../../constitution.md) VII — explicit scope, no silent deferral: the
  composition was in the scope of the store's half and it was not in the suite.
- [`ADR-0044`](0044-the-stores-half.md) — the store's shape, and the glue that is left.
- [`ADR-0047`](0047-the-stores-publish.md) — the path-only rule this ADR's dev-dependency
  follows, and the publish the rule protects.
- [`ADR-0046`](0046-the-source-over-a-set-of-files.md) — the source over a set of files, the other
  half of the composition.
- [`spec 038`](../../specs/038-the-stores-half/spec.md) — the store's half.
- [`spec 040`](../../specs/040-the-source-that-reads-a-set-of-files/spec.md) — the source over a set of
  files, and the glue.
- [`STATUS.md`](../../STATUS.md) — where the project's open questions live.
