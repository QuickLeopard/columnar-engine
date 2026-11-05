# Spec 041 — The reader's four gigabytes — tasks

**The spec**: [spec.md](spec.md). **The rule**: the instrument comes before the change, the
"before" is measured on the tree as it stands, and every figure that moves is published beside the
figure it replaces.

## Phase 1 — the instrument

- [x] The **hostile page header** is written by hand: a Thrift compact header whose statistics field
      declares a length of four gigabytes, written into a real file's page.
- [x] The **file** is real: one column, one row group, one page, written by `parquet`'s own writer,
      and then the page's header is replaced — the footer is untouched, so the file's metadata still
      describes it.
- [x] The probe reads it through the **engine's own reader**, and records the **largest single
      allocation** the reader asked for (a counting global allocator), the elapsed time, and what
      the reader said.
- [x] The **curve**: 1 MiB, 64 MiB, 1 GiB, 4 GiB declared — the cost is the allocation, and the
      curve is what shows it.
- [x] The probe writes a **machine-readable report** (`bench-results/hostile-metadata-run{N}.json`),
      with the host's load, like every other probe in the tree.

## Phase 2 — the before, and the correction

- [x] **Three campaigns** on the tree as it stands, and the medians published.
- [x] The **claim is corrected where it was published** — in **five** places, not three: spec 034's
      R10, `STATUS.md`'s open questions, `deny.toml`'s comment, `crates/columnar-engine/README.md`
      and `docs/embedding.md`. The hole is in the **page headers**, the metadata path is
      **bounded**, and thrift's limit is **opt-in** and parquet does not opt in.
- [x] **A new ADR records the correction**, with the evidence and the reason — and the reason
      ADR-0049 gave for dropping the exemption is corrected too, because `thrift` 0.23.0 is still in
      the graph.

## Phase 3 — the bump

- [x] `parquet` **54.3.1 → 58.4.1** and `arrow` **54 → 58.4.1**, in the workspace's shared
      dependency table.
- [x] The churn the compiler finds is **fixed**, and the engine's behaviour for an honest file does
      not change: the same rows, the same answers, the same plans.
- [x] The **store's** bytes are checked: the published file is the published file, byte for byte.
- [x] **The dataset is regenerated**, because the bump changed the writer's bytes — the published
      file is **242,859,461** bytes where it was 242,857,841, and the data pages are identical.

## Phase 4 — the after, and the figures that moved

- [x] **Three campaigns** on the bumped tree, the same probe, and the medians published beside the
      "before" — **the hole is closed when the reader asks for nothing like what the file declared**.
- [x] **The whole campaign is re-taken**, because a regenerated dataset moves every figure that
      reads a file, and **the superseded sets are committed under their own names** so every figure
      published old-against-new stays derivable from a committed report.
- [x] **Every published figure that moved is published old and new**, in the section that publishes
      it, with the reason, and the machine's own floor beside it — and **§3x** publishes them all
      side by side.
- [x] `scripts/medians.py` and `scripts/check-published-figures.py` know the new reports and the
      new figures.
- [x] **The sweep's report carries both worker counts** — the read's and the operators' — because
      the constitution asks for the workers that *ran* and the two differ; **the seven sweep families
      are re-measured** so that every report can say both, and the column whose meaning changed at
      spec 020 is corrected where it was published. ADR-0051 records it.
- [x] **The size curve is re-measured at all three sizes**, because the campaign had measured it on
      the six-million-row file only and the size axis is the axis the curve exists for. The curve's
      report now carries **the engine's estimate** of its working set and **the host's load
      average**, and **§3i's estimate is corrected**: the published **391 MiB** was *derived* and the
      engine's own figure is **377.7 MiB**. It is corrected in both the places that published it,
      and ADR-0052 records it.

## Phase 5 — the policy, the record, the closure

- [x] `deny.toml`'s exemption is **kept or dropped for the reason that is true after the bump** —
      `RUSTSEC-2026-0435` is **removed**, and the reason is recorded: the advisory's crate is not in
      the graph's protocol sink.
- [x] `STATUS.md`, `README.md` and `CONTRIBUTING.md` say the versions they now use.
- [ ] The spec's **status is done**, and the record says **CI green on <sha>**.

## The gate

- [x] `cargo fmt --all -- --check`
- [x] `cargo clippy --workspace --all-targets -- -D warnings` (and the same for `x86_64-apple-darwin`)
- [x] `cargo test --workspace --all-targets` and `cargo test --workspace --doc`
- [x] `cargo deny check`
- [x] `just check-wasm`
- [x] `cargo semver-checks` — the surface did not break
- [x] `./scripts/check-package.sh` — the four crates package, in the order their dependencies decide
- [x] `python3 scripts/medians.py` and `python3 scripts/check-published-figures.py`
- [x] `python3 scripts/check-links.py` and `python3 scripts/check-page.py`
