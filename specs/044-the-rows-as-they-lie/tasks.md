# Spec 044 — The rows as they lie — the tasks

**Status**: planned
**Started**: 2026-11-06

The order is the house's: **build, then measure, then publish.** The rows path changes first, and the
publication waits for the measurement — which is what moves the figures.

## Phase 1 — the rows as they lie

- [ ] **The test.** The rows are the rows that were read when the filter's selection is every row of the
      dataset and no join or sort produced them. **One test, both paths.**
- [ ] **The rows as they lie.** The dataset's own chunks, projected, with their own encodings. The lazy
      path **moves** them; the eager path **clones** them.
- [ ] **The rows as they were.** The gather stays, exactly as it is, for every plan that needs it — a filter
      that removed rows, a join, a sort.
- [ ] **The shape is stated.** The engine's documentation and the store's README say that the rows' shape
      depends on the plan, and that the store's conversion takes both.
- [ ] **No public break**, and no new type: the rows are the same type they were.

## Phase 2 — the tests

- [ ] A plan that filters nothing hands out the rows as they lie: the encoding is the reader's, and the
      dictionary is there.
- [ ] A plan that filters hands out the gathered rows, as today.
- [ ] A join's and a sort's rows are gathered, as today.
- [ ] The round trip through the store: the file that comes out is the file the rows came from, byte for
      byte, and the store's own rule still differs by the nullability alone.

## Phase 3 — the campaigns

- [ ] **The set's rows** (spec 043's probe) re-run: the rows out, the shape, the round trip's bytes, and
      the first differing byte.
- [ ] **The engine's rows** (spec 042's probe) re-run: the plan's rows, and the figures that moved.
- [ ] Three campaigns each, medians with the spreads, the raw reports committed under their own names.

## Phase 4 — the publication

- [ ] The measurements are published in `PERF.md` §3ab, with the tables derived from the committed reports by
      `scripts/medians.py` and checked by the guard **inside the section that publishes them**.
- [ ] **§3aa and §3z publish their moved figures old-and-new**, with the reason.
- [ ] **ADR-0056** records the decision: the rows are the rows that were read, when they are.
- [ ] **`STATUS.md`** counts the specs and the ADRs, and the open questions are re-read.
- [ ] **The guides** carry the rule: the rows' shape depends on the plan.
- [ ] The spec's amendment records what the measurement changed.

## The gate

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` (and the same for `x86_64-apple-darwin`)
- [ ] `cargo test --workspace --all-targets` and `cargo test --workspace --doc`
- [ ] `cargo deny check`
- [ ] `just check-wasm`
- [ ] `cargo semver-checks` — no new break.
- [ ] `./scripts/check-package.sh` — the four crates package, in the order their dependencies decide.
- [ ] `python3 scripts/medians.py` and `python3 scripts/check-published-figures.py`
- [ ] `python3 scripts/check-links.py` and `python3 scripts/check-page.py`
- [ ] **CI green**, and the closure commit says on which commit.
