# Spec 039 — The engine reads a store's table — tasks

**Status**: done
**Spec**: [spec.md](spec.md)

## Phase 1 — the instrument

- [x] The **six row groups as six files**, written through the store, so the rows are the published file's
      own rows.
- [x] The **one-file read** and the **six-files read**, in the same campaign, on the same host.
- [x] The **answers checked**: the six-files read gives the same rows, in the same order, as the one-file
      read.

## Phase 2 — the campaigns

- [x] Three campaigns of the probe, the raw reports committed under their own names.
- [x] The host's load average published with each, and the spread beside every median.

## Phase 3 — the source

- [x] **Moved to spec 040 by the amendment** in the spec: the measurement found the boundary free, so the
      source's case is correctness rather than performance, and a change to the engine's read path has its
      own increment here. **Spec 040 carries the source, and this increment publishes the figure that
      decides what it has to achieve: correct, and free.**

## Phase 4 — the publication

- [x] `PERF.md` publishes the boundary's cost, and the guard checks every figure **inside the section that
      publishes it**.
- [x] The **surface list** carries the new name, and the rule that every type in a public signature is on
      the list holds.
- [x] The **ADR** records the source's shape.

## Phase 5 — the closure

- [x] The spec's status is `done` with the date it finished, and CI is green on the publish commit.

## The gate

- [x] `cargo fmt`, clippy on arm64 and x86-64, the whole test suite, `cargo deny`, the wasm32 check, the
      published-figures guard and the page checks all green.
- [x] CI green, and the spec's status is `done` with the date it finished.
