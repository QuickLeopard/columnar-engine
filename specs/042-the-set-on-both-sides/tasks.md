# Spec 042 — The set on both sides — the tasks

**Status**: done
**Started**: 2026-11-06
**Finished**: 2026-11-06

The order is the house's: **measure, then build, then publish.** The probes come first because their
numbers decide the shapes — the estimate's charge, the table's shape over a set, and whether the engine's
rows write the published file byte for byte.

**The order moved once, and the reason is recorded.** The probes could not be run before the code they
measure: the build side's **cost** is the cost of a build side that takes a set, and the write path's
probe measures the **store's own** conversion. So the code came first, the probes measured it, and the
publication came last — the house's rule is that the **publication** waits for the measurement, and it
did ✓.

## Phase 1 — the probes

- [x] **The build side's probe.** The published file's own **six row groups** written as six files, read
      both ways in the same campaign. It answers four questions: the **cost** of building the table from
      six files against one, the **identity** (the same rows, the same counts per key, the same output
      rows in the same order), the **table's shape** (the set's union range against the one file's, and
      whether the direct-addressed table survives `DIRECT_TABLE_LIMIT`), and the **estimate's charge**
      against the bytes the build actually uses. → `crates/columnar-engine/examples/build_set_probe.rs`
- [x] **The write path's probe.** The published file read through the engine's own reader, the engine's
      own rows handed to the store, and the result compared with **two** files: the published file and
      the file the store's own Arrow path writes from the same rows. **The first differing byte is
      named**, whichever way it comes out. → `crates/columnar-store/examples/engine_rows_probe.rs`
- [x] Three campaigns each, medians with the spreads, the raw reports committed under their own names.
      → `bench-results/build-set-run{1,2,3}.json`, `bench-results/engine-rows-run{1,2,3}.json`

## Phase 2 — the build side takes a set

- [x] `EngineSource` — crate-private — gains `column_index`, `column_range` and `row_group_ranges`.
- [x] `BuildSide` holds a source: **one path opens a `ParquetSource`** and behaves exactly as it does
      today, **several paths open a `FileSetSource`**.
- [x] `build_table` and `build_key_type` take a source, not a file. `build_table` stays public for the
      one-file path and delegates to a crate-private generic implementation.
- [x] The estimate's build-side charge takes a source: the set's **rows** (the sum), the set's **range**
      (the union) and the set's **workers**.
- [x] `JoinSpec.build_path: PathBuf` becomes `build_paths: Vec<PathBuf>` — **the one breaking change on
      the published surface**, free while the crate is unpublished.
- [x] **The one-file path's published figures do not move.** The join probe measures the one-file build
      side and was changed to take the first path of the list; the measurement is the same measurement.

## Phase 3 — the engine writes a table

- [x] The store gains the **chunk-to-Arrow conversion**: the engine's format to Arrow's, value for
      value, dictionary for dictionary, and the validity mask with them.
- [x] The store gains an append that takes the engine's rows, **re-batches them on the grid**, writes
      them with the store's writer and policy, and commits. The **existing Arrow-taking append stays** —
      the same rule, one implementation.
- [x] **The grid has one owner.** The engine does not learn the number 65,536; the store keeps the rule
      and keeps the error.
- [x] The store's dependency on `columnar-core` is added, and **the engine's dependency on the store is
      not**.
- [x] The engine gains `execute_rows` and `execute_rows_on`, and **a plan with an aggregate is refused**.

## Phase 4 — the publication

- [x] The measurements are published in `PERF.md`, with the tables derived from the committed reports by
      `scripts/medians.py` and checked by the guard **inside the section that publishes them**.
- [x] **ADR-0053** records the build side's decision: a set on the build side, one path still one path,
      and the estimate's charge.
- [x] **ADR-0054** records the write side's decision: the conversion lives in the store, the grid has
      one owner, and the store depends on the core — the vocabulary, not the engine.
- [x] **Every place that published the store's dependency rule is corrected where it says it**:
      ADR-0044's decision 1, ADR-0046's decision 7, ADR-0047's correction, `STATUS.md`, the store's
      `Cargo.toml`, its README, the workspace manifest and `docs/embedding.md`.
- [x] The spec's amendment, if the measurements move a published figure, records the reason.

## The gate

- [x] `cargo fmt --all -- --check`
- [x] `cargo clippy --workspace --all-targets -- -D warnings` (and the same for `x86_64-apple-darwin`)
- [x] `cargo test --workspace --all-targets` and `cargo test --workspace --doc`
- [x] `cargo deny check`
- [x] `just check-wasm`
- [x] `cargo semver-checks` — the surface's one breaking change is accounted for: the recorded baseline
      moves to the commit that establishes the new surface, and the commit that moves it says why.
- [x] `./scripts/check-package.sh` — the four crates package, in the order their dependencies decide
- [x] `python3 scripts/medians.py` and `python3 scripts/check-published-figures.py`
- [x] `python3 scripts/check-links.py` and `python3 scripts/check-page.py`
