# Spec 040 — The source that reads a set of files — tasks

**Status**: open
**Spec**: [spec.md](spec.md)

## Phase 1 — the source

- [ ] The **merged metadata**: the files' schema checked against each other, the rows summed, the row
      groups numbered across the files, and the row groups' rows concatenated.
- [ ] The **pruning over the union**: every file's statistics, and a row group that cannot survive is not
      read, whichever file it is in.
- [ ] The **batches across the boundary**, and the rows in the files' order -- file by file, and inside a
      file row group by row group.
- [ ] The **parallel read** splits the merged row groups into contiguous blocks and joins the parts in
      block order, as the one-file read does.

## Phase 2 — the tests

- [ ] The set's rows against the one file's, **row by row, in order**.
- [ ] The pruning over a union that spans the files.
- [ ] The order across the boundary, and the engine's own `NULL` and `NaN` rules.

## Phase 3 — the campaigns

- [ ] Three campaigns: the set's read against the one-file read and against the files read one by one.
- [ ] The host's load average published with each, and the spread beside every median.

## Phase 4 — the publication

- [ ] `PERF.md` publishes the source's cost, and the guard checks every figure **inside the section that
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
