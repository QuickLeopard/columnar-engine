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
- [x] The **claim is corrected where it was published**: spec 034's R10, `STATUS.md`'s open
      questions, and `deny.toml`'s comment — the hole is in the **page headers**, the metadata path
      is **bounded**, and thrift's limit is **opt-in** and parquet does not opt in.
- [x] **A new ADR records the correction**, with the evidence and the reason.

## Phase 3 — the bump

- [ ] `parquet` **54.3.1 → 58.4.1** and `arrow` **54 → 58.4.1**, in the workspace's shared
      dependency table.
- [ ] The churn the compiler finds is **fixed**, and the engine's behaviour for an honest file does
      not change: the same rows, the same answers, the same plans.
- [ ] The **store's** bytes are checked: the published file is the published file, byte for byte.

## Phase 4 — the after, and the figures that moved

- [ ] **Three campaigns** on the bumped tree, the same probe, and the medians published beside the
      "before" — **the hole is closed when the reader asks for nothing like what the file declared**.
- [ ] **Every published figure that moved is published old and new**, in the section that publishes
      it, with the reason, and the machine's own floor beside it.
- [ ] `scripts/medians.py` and `scripts/check-published-figures.py` know the new reports and the
      new figures.

## Phase 5 — the policy, the record, the closure

- [ ] `deny.toml`'s exemption is **kept or dropped for the reason that is true after the bump**.
- [ ] `STATUS.md`, `README.md` and `CONTRIBUTING.md` say the versions they now use.
- [ ] The spec's **status is done**, and the record says **CI green on <sha>**.

## The gate

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` (and the same for `x86_64-apple-darwin`)
- [ ] `cargo test --workspace --all-targets` and `cargo test --workspace --doc`
- [ ] `cargo deny check`
- [ ] `just check-wasm`
- [ ] `cargo semver-checks` — the surface did not break
- [ ] `./scripts/check-package.sh` — the four crates package, in the order their dependencies decide
- [ ] `python3 scripts/medians.py` and `python3 scripts/check-published-figures.py`
- [ ] `python3 scripts/check-links.py` and `python3 scripts/check-page.py`
