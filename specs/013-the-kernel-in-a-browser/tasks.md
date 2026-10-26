# Spec 013 — Tasks

**Spec**: [013-the-kernel-in-a-browser](spec.md)
**ADR**: [ADR-0019](../../docs/adr/0019-the-kernel-in-a-browser.md)
**Status**: done

## T1 — The third SIMD path

- [x] `columnar-kernels` builds for `wasm32-unknown-unknown`: the C++ translation units are skipped and
      the FFI module is gated out, so the browser build says what it is.
- [x] The `simd128` path is implemented in the same module as NEON and AVX2: one 64-row block per
      step, the mask reduced once per block, the oracle's semantics.
- [x] The `simd128` path's **ordered** comparisons are checked against the wasm SIMD spec's own
      `ne`, which is unordered and would make a NaN match. `ne` is spelled out as `gt | lt`.

## T2 — The module and the page

- [x] A `columnar-web` crate: `cdylib` + `rlib`, a tiny explicit ABI of eight exported functions, no
      `wasm-bindgen`.
- [x] The column is rebuilt in the browser from two committed histograms, in a deterministic
      interleaved order, with the exact multiset of the native column.
- [x] The page runs the oracle and the `simd128` path over the same column, compares the masks bit
      for bit, and prints the table only if they agree.

## T3 — The measurement

- [x] The page was run in a real browser on this machine — Chromium 148.0.7778.280, Electron
      42.8.0 — and the numbers were read back.
- [x] The conditions are recorded: the browser, the engine, the machine, the host's load average,
      the rows, the iterations, the rounds and the commit.
- [x] The browser's match count is **12,726,115** — the native's number, reproduced. Three
      campaigns, every one of them, masks identical bit for bit.

## T4 — The publication

- [x] [ADR-0019](../../docs/adr/0019-the-kernel-in-a-browser.md).
- [x] `PERF.md` §1a — the browser's number, with its conditions and its limits, and never beside
      the native one.
- [x] The README — what the page is, how to run it, and what it is not.
- [x] The constitution amended to **2.0.0**: Principle I now covers six implementations, and two
      Performance Standards were added with it.
- [x] `scripts/check-published-figures.py` guards the browser's match count, and
      `scripts/medians.py` derives its table from the committed reports.
