# Spec 043 — The set's rows — the tasks

**Status**: open
**Started**: 2026-11-06

The order is the house's: **build, then measure, then publish.** The door has to exist before the set's rows
can be measured, and the house's rule is that the **publication** waits for the measurement — which it does.

## Phase 1 — the set's rows

- [x] **`FileSetSource::execute_rows`** — the set's own door to its rows. It calls the engine's one path:
      `facade::execute_source(plan, self, options, Output::Rows)`, the same call `execute` makes with
      `Output::Aggregate`.
- [x] **A plan with an aggregation is refused**, with the **same error and the same wording** as the eager
      and one-file paths — one rule, one message.
- [x] **No new public type, and no break.** `Dataset`, `QueryPlan` and `ExecOptions` are on the surface
      already; the surface list does not change and `cargo semver-checks` reports no change against the
      baseline spec 042 moved.
- [x] **The store changes not at all.** The glue stays the list of paths and the engine's rows.

## Phase 2 — the probe

- [x] **The round trip's probe.** The published file's own **six row groups** written as six files, read
      both ways in the same campaign: the set's rows and the one file's rows, each written by the store, each
      compared with the published file and with the other. It answers four questions: the **cost** (the read
      of six files against one, and the write of each one's rows), the **identity** (the two written files,
      byte for byte), the **round trip** (the written file against the published file, the bytes, and **the
      first differing byte**), and the **estimate** (what the budget is charged for the set).
      → `crates/columnar-store/examples/set_rows_probe.rs`
- [x] **And the decomposition of the read**: the engine's eager read against the eager rows, so the gather —
      the copy the rows path makes of everything it read — is measured rather than guessed.
- [x] Three campaigns, medians with the spreads, the raw reports committed under their own names.
      → `bench-results/set-rows-run{1,2,3}.json`

## Phase 3 — the tests

- [x] The store writes a table; the engine reads it as a set; the engine's rows go back into the store; the
      **data** comes out the same. → `crates/columnar-store/tests/set_rows.rs`
- [x] The set's rows are the one file's rows — the same values, in the same order, and **the same file**.
- [x] A plan with an aggregation is refused on the set's path, as it is on the other two.

## Phase 4 — the publication

- [x] The measurements are published in `PERF.md` §3aa, with the tables derived from the committed reports by
      `scripts/medians.py` and checked by the guard **inside the section that publishes them**.
- [x] **ADR-0055** records the decision: the set's rows are handed out, the identity is exact, the round trip
      is the file's data and not its metadata, and the surface does not move.
- [x] **`STATUS.md`** counts the specs and the ADRs, and the open questions are re-read.
- [x] **The guides** (`docs/embedding.md` and the three guides) carry the round trip's three lines.
- [x] The spec's amendment records what the measurement changed: the claim, the copy, and the next increment.

## The gate

- [x] `cargo fmt --all -- --check`
- [x] `cargo clippy --workspace --all-targets -- -D warnings` (and the same for `x86_64-apple-darwin`)
- [x] `cargo test --workspace --all-targets` and `cargo test --workspace --doc`
- [x] `cargo deny check`
- [x] `just check-wasm`
- [x] `cargo semver-checks` — the surface's one breaking change is still accounted for, and this increment
      adds none.
- [x] `./scripts/check-package.sh` — the four crates package, in the order their dependencies decide.
- [x] `python3 scripts/medians.py` and `python3 scripts/check-published-figures.py`
- [x] `python3 scripts/check-links.py` and `python3 scripts/check-page.py`
- [ ] **CI green**, and the closure commit says on which commit.
