# Spec 054 — The crates are published

**Status**: planned
**Started**: 2026-11-09
**Predecessor**: [spec 053 — the cold path's context](../053-the-cold-paths-context/spec.md)

## The problem

**The engine is measured, and it is a library, and it cannot be installed.** [ADR-0040](../../docs/adr/0040-the-engine-as-a-library.md)
decided that the engine is a library — a facade, a `prelude`, a `Catalog`, and a surface of **forty-one** names beside
`columnar-core`'s **four** — and it is all there in the tree. **And none of it is reachable**, because each crate's internal
dependency carries a `version` beside its `path`: cargo resolves a packaged crate's dependencies from crates.io and not from
the workspace, and **nothing is on crates.io yet**. So `cargo package -p columnar-engine` fails outright, and the publish
order is forced by the graph — `columnar-core`, `columnar-kernels`, `columnar-store`, `columnar-engine` — because the
dependency direction is total and acyclic.

**And the publish has no receipt.** Spec 040 recorded that **the publish itself is the user's act**, and it left the record
to this increment: nothing in the tree says **what landed**, **what the published surface is**, or **that the published
crate is the crate this repository measured**. The house's rule is that a published claim carries its evidence: *a number
is a claim about a measurement, and a claim without its evidence is not published* — and a crate on a registry is the
largest claim this repository makes.

**And a published surface is a promise.** After 0.1.0 the surface is the only part of this repository that other code is
allowed to depend on, and it is the one part that **cannot be done twice**: a surface published and then changed is a break
in every embedder's build. **So the surface is checked against the published crate and not against the workspace's**, and
the check is the same check that was made before the publish.

**And the engine is measured against DuckDB and Polars with a harness that lives inside it.** The fourteen measurement
probes are the crate's own examples, and they are not part of the surface. **What is published is the engine and not the
harness**, and the proof that the two are the same crate is that the **embedder test** — the one that imports only the
facade — runs against the registry version and answers the same.

## The requirements

### R1 — The publish happens, in the order the graph decides, and its receipt is taken

- [ ] The four crates are published in the order their dependencies decide — **`columnar-core`**, **`columnar-kernels`**,
      **`columnar-store`**, **`columnar-engine`** — each one's `--dry-run` first.
- [ ] **And the receipt is crates.io's own record of each crate**: the version, the packaged file list with its sizes, and
      the metadata — the description, the licence, the repository, the keywords.
- [ ] **And the receipt is committed**, so that the tree says what landed and the figures are checkable from it.

### R2 — The published crate is the crate this repository measured

- [ ] The **embedder test** — `crates/columnar-engine/tests/embed.rs`, the one that imports only the facade — is run
      **against the registry version**, from a crate outside the workspace with **no path dependency**.
- [ ] **And it resolves on crates.io and not in the workspace**, which is the whole point of the publish.
- [ ] **And its answer is the workspace's own**: the same rows, the same value, the same plan output's shape.

### R3 — The published surface is the **frozen list**

- [ ] The surface list — the **forty-one** engine names and the **four** of `columnar-core`'s vocabulary — is checked
      against the **published** crate, and not only against the workspace's.
- [ ] **And a name that is on the list and is not published is a defect**, and **a name that is published and is not on
      the list is a defect**: the promise is the list.

### R4 — The record is published

- [ ] A **new section** in `PERF.md`, the decision in an **ADR**, and **the status's bullet moves out of "the work the
      project has not done"** into the done list.
- [ ] **And the guard checks the published figures inside the section that publishes them.**

### R5 — The closure

- [ ] The spec's status is `done` with the date it finished, and CI is green on the publish commit.

## The scope

- **In**: the publish, the receipt, the installability check, the surface check, and the record.
- **Out**: **the version** — 0.1.0 is the first publish, and the surface is frozen at it.
- **Out**: **a changelog and a release policy**. The house has never published, and the first publish is not the moment a
  policy is invented.
- **Out**: **the wide shape's plain string's copy**, which is the wide shape's gap and wants its own measurement.
- **Out**: **the crates.io account and the token**. The publish is the user's act; this increment is what records it.

## The instrument

**`crates/columnar-engine/tests/embed.rs`** — the embedder test: it imports **only** the facade, it writes its own
Parquet file, and it is the surface's own test. **Run against the registry, it is the proof that the published crate is
installable and complete** — and it needs no change, because the same file is the check before the publish and after it.

**crates.io's own API** — the receipt: the versions, the packaged file lists with their sizes, and the metadata.

**`scripts/check-package.sh`** — the packaging, the manifests and the public surface, checked before the publish and
unchanged by it.

## The references

- [docs/embedding.md](../../docs/embedding.md) — the guide an embedder reads, and the publish order it publishes.
- [ADR-0040](../../docs/adr/0040-the-engine-as-a-library.md) — the surface, the facade, and the promise a published
  surface is.
- [ADR-0047](../../docs/adr/0047-the-stores-publish.md) — the store publishes with the others.
- [PERF.md §3aj](../../PERF.md#3aj-the-contexts-two-sides-in-one-account) — the last increment, and the last open question,
  closed.
- [constitution.md](../../constitution.md), principle **IV — Evidence Over Claims**: a number is a claim about a
  measurement, and a claim without its evidence is not published.
- [constitution.md](../../constitution.md), principle **VII — Explicit Scope, No Silent Deferral**: the version, the
  changelog and the release policy are **named** as out of scope, rather than left as omissions.

## The amendments

### A1 — The receipt is taken from crates.io, and not from the tree

**Why**: the tree cannot vouch for what is on a registry, and the house's rule is that a figure is derived from the
committed reports and checked by the guard. **So the receipt is crates.io's own record**, taken at the moment of the
publish and committed with the version it belongs to — and the guard checks the section's figures against it, the way it
checks every other section.

### A2 — The check is the embedder test, unchanged

**Why**: the test was written to be the surface's own test, and it runs anywhere because it writes its own data. **Copying
it unchanged into a crate that depends on the registry version is the strongest statement the repository can make**: the
same test, the same imports, resolved against crates.io instead of the workspace. **And if it needs a change to run there,
the surface is wrong** — and that is a defect to publish, not a patch to make quietly.
