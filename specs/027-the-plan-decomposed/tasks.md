# Spec 027 — The plan, decomposed — tasks

**Status**: in progress
**Spec**: [spec.md](spec.md)

## The steps

Each step is one commit. Each one builds, passes the whole test suite, is clippy-clean on arm64
and x86-64, and leaves the published figures exactly where they were.

- [x] **Step 1 — `plan/`.** `plan.rs` becomes `plan/{mod,spec,resolved,resolve}.rs`: the caller's
      plan, the executor's plan, the bridge, and the module's surface. A move, and nothing else.
- [x] **Step 2 — the mapping.** The four `remap` siblings become one `ColumnMapping` type, with one
      constructor and one way to move an index. The panic in `remap_index` goes.
- [x] **Step 3 — `scan.rs`.** The scan's decision and the mapping it produces leave the crate
      root and live beside each other.
- [x] **Step 4 — `pipeline.rs`.** The pipeline leaves the crate root, and `run_pipeline`'s ten
      parameters become a context. `#[allow(clippy::too_many_arguments)]` is gone.
- [x] **Step 5 — one stage shape.** The four stage idioms become one, so the plan output is
      assembled in one place.
- [x] **Step 6 — the estimates.** `estimate_join` and `estimate_sort` join the other estimates in
      `memory.rs`.
- [ ] **Step 7 — the surface.** The crate root keeps the public surface and the story, and nothing
      else.

## The invariants

- [x] The **whole test suite** is green after every step, not only at the end.
- [ ] **No behaviour changes**: the answer, the plan output, the memory estimates and every
      published figure are exactly what they were.
- [ ] **No new `#[allow]`**, no `unsafe`, no new dependency, and no `unwrap`/`expect` introduced.
- [ ] The **public API does not move**: `columnar_engine::plan::JoinSpec` and the crate root's
      re-exports stay where they are.
- [ ] The **guard is green** after every step: `python3 scripts/check-published-figures.py` and
      `python3 scripts/check-page.py`.

## The gate

- [ ] `cargo fmt`, clippy on arm64 and x86-64, the whole test suite, `cargo deny`, the wasm32
      check, the published-figures guard and the page checks all green.
