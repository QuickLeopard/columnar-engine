# Spec 054 — The crates are published — tasks

**Status**: planned
**Started**: 2026-11-09
**Spec**: [spec.md](spec.md)

The order is the house's: **the publish, then the receipt, then the checks, then the record, then the closure.** The publish
is the user's act; everything after it is this increment's.

## The publish

- [ ] `cargo login` — the user's act, and the token never reaches this repository.
- [ ] The four crates are published in the order their dependencies decide — **`columnar-core`**, **`columnar-kernels`**,
      **`columnar-store`**, **`columnar-engine`** — each one's `--dry-run` first.
- [ ] **And the publish order is the one the manifests decide**, with the tie the graph cannot break broken by the list.

## The receipt

- [ ] crates.io's own record of each crate: the version, the packaged file list with its sizes, and the metadata.
- [ ] **And it is committed**, under the version it belongs to.

## The checks

- [ ] The **embedder test** is run **against the registry version**, from a crate outside the workspace with no path
      dependency.
- [ ] **And it resolves on crates.io and not in the workspace.**
- [ ] **And its answer is the workspace's own**: the same rows, the same value.
- [ ] **And the surface list is checked against the published crate**, and a name that is published and is not on the list
      is a defect.

## The record

- [ ] A **new section** in `PERF.md`, the decision in an **ADR**, and **the status's bullet moves out of "the work the
      project has not done"**.
- [ ] **And the guard checks the published figures inside the section that publishes them.**

## The closure

- [ ] The spec's status is `done` with the date it finished, and CI is green on the publish commit.

## What is not done

- **The version** — 0.1.0 is the first publish, and the surface is frozen at it.
- **A changelog and a release policy** — the house has never published, and the first publish is not the moment a policy
  is invented.
- **The wide shape's plain string's copy**, which is the wide shape's gap, and the fix wants its own measurement.
