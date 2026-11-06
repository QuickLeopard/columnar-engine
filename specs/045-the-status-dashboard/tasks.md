# Spec 045 — The status dashboard — the tasks

**Status**: planned
**Started**: 2026-11-06

The order is the house's: **build, then gate, then publish.** The page is an artefact of the
documents, so the gate comes with it and not after it.

## Phase 1 — the generator

- [ ] **The page holds no figure of its own.** The comparisons come from `STATUS.md`, the defect
      count from `PERF.md`, the counts from the tree, the increments from the specs. **One owner per
      figure.**
- [ ] **The generator** — `scripts/build-dashboard.py` — reads the documents and writes
      `docs/status.html`. The markdown the documents use is rendered: the headings, the tables, the
      lists, the links, the code.
- [ ] **The links are rewritten for the page's own place.** The documents' links are relative to the
      repository root, and the page lives in `docs/`.
- [ ] **It is a dashboard.** The comparisons stand open and the prose that explains what moved is
      folded; the open questions stand open and the ledger of the work not done is folded; the tree's
      shape, the gates and the increments are compact.
- [ ] **It is self-contained.** One file, no external asset, no script, no network.

## Phase 2 — the gate

- [ ] **`--check` rebuilds the page and compares it with the committed one**, and fails when they
      differ.
- [ ] **`just dashboard`** builds the page, and **`just check-dashboard`** checks it.
- [ ] **The CI job** — the page is rebuilt and compared, and a difference fails the build.
- [ ] **`CONTRIBUTING.md`'s table of gates** has the line, and `just check` runs it.

## Phase 3 — the house's documents

- [ ] **The spec** and **ADR-0057** record the decision: the page is generated, and the gate is what
      keeps it true.
- [ ] **STATUS.md** counts the spec and the ADR, and says what the increment found.
- [ ] **The links resolve** and the page's own links are checked.

## The gate

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace --all-targets` and `cargo test --workspace --doc`
- [ ] `cargo deny check`
- [ ] `just check-wasm`
- [ ] `cargo semver-checks check-release` — no new break.
- [ ] `./scripts/check-package.sh`
- [ ] `python3 scripts/medians.py`, `python3 scripts/check-published-figures.py`,
      `python3 scripts/check-links.py`, `python3 scripts/check-page.py`
- [ ] `python3 scripts/build-dashboard.py --check` — the page is current.
- [ ] **CI green**, and the closure commit says on which commit.
