# Spec 039 — The engine reads a store's table — tasks

**Status**: open
**Spec**: [spec.md](spec.md)

## Phase 1 — the instrument

- [ ] The **six row groups as six files**, written through the store, so the rows are the published file's
      own rows.
- [ ] The **one-file read** and the **six-files read**, in the same campaign, on the same host.
- [ ] The **answers checked**: the six-files read gives the same rows, in the same order, as the one-file
      read.

## Phase 2 — the campaigns

- [ ] Three campaigns of the probe, the raw reports committed under their own names.
- [ ] The host's load average published with each, and the spread beside every median.

## Phase 3 — the source

- [ ] The **merged metadata**: the row groups of every file, numbered across them.
- [ ] The **pruning over the union**: every file's statistics, and the engine's own predicate applied to
      every file's rows.
- [ ] The **batches across the boundary**, and the rows in the same order either way.
- [ ] The **single-file path untouched**, and its published figures unmoved.

## Phase 4 — the publication

- [ ] `PERF.md` publishes the boundary's cost, and the guard checks every figure **inside the section that
      publishes it**.
- [ ] The **surface list** carries the new name, and the rule that every type in a public signature is on
      the list holds.
- [ ] The **ADR** records the source's shape.

## Phase 5 — the closure

- [ ] The spec's status is `done` with the date it finished, and CI is green on the publish commit.

## The gate

- [ ] `cargo fmt`, clippy on arm64 and x86-64, the whole test suite, `cargo deny`, the wasm32 check, the
      published-figures guard and the page checks all green.
- [ ] CI green, and the spec's status is `done` with the date it finished.
