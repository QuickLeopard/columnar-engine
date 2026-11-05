# Spec 040 — The source that reads a set of files — tasks

**Status**: open
**Spec**: [spec.md](spec.md)

## Phase 1 — the source

- [x] The **merged metadata**: the files' schema checked against each other, the rows summed, the row
      groups numbered across the files, and the row groups' rows concatenated.
- [x] The **merged statistics**, by the **amendment** in the spec: the range is the **union** of the files'
      ranges, and the row groups are the files' row groups numbered across the files. **There is no
      pruning to merge** — the engine does not prune row groups by the file's statistics, and the spec's
      original requirement said otherwise.
- [x] The **batches across the boundary**, and the rows in the files' order -- file by file, and inside a
      file row group by row group. **The files' readers are chained and decoded in one pass**, so the rows
      are copied once.
- [x] The **parallel read** splits the merged row groups into contiguous blocks and joins the parts in
      block order, as the one-file read does.

## Phase 2 — the tests

- [x] The set's rows against the one file's, **row by row, in order**.
- [x] The **statistics over a union** that spans the files, and the predicate reaching every file.
- [x] The order across the boundary, and the engine's own `NULL` and `NaN` rules.
- [x] The files that **disagree** are refused, and the error says which file disagreed and how.

## Phase 3 — the campaigns

- [x] Three campaigns: the set's read against the one-file read and against the files read one by one.
- [x] The host's load average published with each, and the spread beside every median.

## Phase 3a — the glue, and the store's publish

- [x] The **glue is one call**: `Table::paths` hands the engine the list of paths it takes, in the table's
      order, and the store's scope is amended with the reason recorded in the spec.
- [x] The **crate packages**: the dev-dependency on the generator is **path-only**, so it is dropped from
      the published manifest, and the store carries its own `LICENSE`.
- [x] The **publish order** is computed from the manifests, and the tie the graph cannot break -- the
      store depends on nothing -- is broken by the list.
- [x] The **record is corrected**: ADR-0044's two wrong sentences are corrected where they were published,
      and ADR-0047 records the decision.

## Phase 4 — the publication

- [x] `PERF.md` publishes the source's cost, and the guard checks every figure **inside the section that
      publishes it**.
- [x] The **surface list** carries the new name, and the rule that every type in a public signature is on
      the list holds.
- [x] The **ADR** records the source's shape, **and the correction** the increment found: the pruning
      claim was wrong about this engine, and it is corrected where it was published — in specs 039 and 040
      and in ADR-0045.

## Phase 5 — the closure

- [ ] The spec's status is `done` with the date it finished, and CI is green on the publish commit.

## The gate

- [x] `cargo fmt`, clippy on arm64 and x86-64, the whole test suite, `cargo deny`, the wasm32 check, the
      published-figures guard and the page checks all green.
- [ ] CI green, and the spec's status is `done` with the date it finished.
