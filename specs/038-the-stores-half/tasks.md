# Spec 038 — The store's half — tasks

**Status**: open
**Spec**: [spec.md](spec.md)

## Phase 1 — the crate

- [x] The **manifest**: a table's files, their rows and their bytes, and the table's own totals, in JSON.
- [x] The **catalog**: the manifest on disk, read back when the store is opened, and the tables it names.
- [x] The **writer**: rows in, a Parquet file out, with the file's own writer properties and the file's own
      row groups.
- [x] The **commit**: the five steps, in the order a commit has to take them.
- [x] The **grid rule**: 65,536 rows, and the error that says so when the caller breaks it.

## Phase 2 — the verification

- [x] The store writes the **published file's own rows**, and the file it wrote is compared with the
      published file **byte for byte**.
- [x] The manifest is read back after **every** step of the commit, and the table's rows and bytes are the
      ones the commit says they are.
- [x] The store's own **probe** writes a report, and **three campaigns** of it are committed.

## Phase 3 — the publication

- [x] The crate's **README** says what the store is, what it is not, and how a caller uses it.
- [x] `PERF.md` publishes the store's file -- its bytes and the verdict on its identity -- and the guard
      checks them **inside the section that publishes them**.
- [x] The **embedding guide** gains the store's section.

## Phase 4 — the ADR and the questions

- [x] The **ADR** records the store's shape: the crate, the manifest, the commit, the grid rule, and the
      scope that was refused.
- [x] The **next question** is written down where this one was: the README's Next row and `STATUS.md`'s
      last section.

## Phase 5 — the closure

- [ ] The spec's status is `done` with the date it finished, and CI is green on the publish commit.

## The gate

- [x] `cargo fmt`, clippy on arm64 and x86-64, the whole test suite, `cargo deny`, the wasm32 check, the
      published-figures guard and the page checks all green.
- [ ] CI green, and the spec's status is `done` with the date it finished.
