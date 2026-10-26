# Spec 013 — The kernel in a browser

**Status**: done
**Started**: 2026-10-26
**ADR**: [ADR-0019](../../docs/adr/0019-the-kernel-in-a-browser.md)
**Predecessor**: [012-the-scan-does-not-expand-the-predicates-column](../012-the-scan-does-not-expand-the-predicates-column/spec.md)

## The problem

`just demo` is the hook for a reader who has a terminal. It is not the hook for a reader who has a
link, and a link is the thing that travels.

The kernel is the project — one predicate, five implementations, one answer — and the answer is
the same in every implementation because the implementations are held to the oracle bit for bit.
That claim is currently only checkable by a reader who can build Rust and run the differential
suite. It should be checkable by a reader who opens a page.

A browser is a **third** SIMD target, and it is a real one: WebAssembly's `simd128` is a 128-bit
vector ISA, like NEON and like AVX2's low half, and Rust's `core::arch::wasm32` exposes it as
intrinsics. So the browser is not a place where the kernel degrades to a plain loop. It is a place
where the kernel is a **third** implementation that must agree with the other five.

## The requirements

### R1 — The browser runs the engine's kernel, not a copy of it

- [x] The WASM module is compiled from **`columnar-kernels`**, the same crate the native build uses. The
      browser has no second implementation of the predicate.
- [x] The `simd128` path is a **third SIMD path** in the same module as the NEON and AVX2 paths, with
      the same shape: one 64-row block per step, one mask, the oracle's semantics.
- [x] The C++ translation units are **not** built for the browser target, and the browser's build does
      not pretend otherwise.

### R2 — The browser's answer is the native's answer

- [x] The column the browser measures over has the **same multiset** as the column the native
      benchmark measures over, so the number of matching rows is **identical**: **12,726,115** at
      20,000,000 rows, and **3,817,685** at 6,000,000.
- [x] The multiset is carried by **two small histograms** — the whole column's, and the first
      2,000,000 rows' — because the native benchmark *tiles* the column, and the tile's partial
      copy has to be reproduced exactly.
- [x] The claim is **checked**, not asserted: the browser's own match count is compared with the
      native's published number, and a mismatch fails the build.

### R3 — The page checks itself

- [x] The page compares the `simd128` mask against the **oracle's** mask, **bit for bit**, over the
      same column, before it prints anything.
- [x] If the masks disagree, the page prints the disagreement and **no throughput number**. A page
      that publishes a speed for a wrong answer is worse than a page that publishes nothing.
- [x] The page shows the **browser, the engine and the machine** the numbers came from, because a
      browser's number is not a machine's number.

### R4 — The measurement is a measurement

- [x] The browser's throughput is measured **in a real browser**, on a named browser and a named
      machine, and published with its conditions and its spread.
- [x] The browser's number is **never** compared with the native number. They are different
      runtimes; the ratio between them is a claim about two engines, not about one kernel.
- [x] The comparison the browser *can* make — `simd128` against the oracle, over the same data, in the
      same process — is the one it makes.

### R5 — The tests

- [x] The column builder is tested **natively**: the histogram produces the exact multiset, and the
      match count is the published one.
- [x] The WASM target is a **build gate**: `just check-wasm` compiles the module for `wasm32`, so the
      browser build cannot rot silently — the way the AVX2 path rotted once already.
- [x] The **mutations were run**, and each one was caught.

### R6 — The publication

- [x] ADR-0019 records the decision, the conditions and the limits.
- [x] `PERF.md` publishes the browser's number with everything that number depends on.
- [x] The README says what the page is and what it is not.

## The non-goals

- **The engine does not run in the browser.** The browser runs the *kernel*, over a column it
  rebuilds from a histogram. Parquet, the query planner and the engine's scan stay out of it.
- **The C++ does not go to the browser.** There is no wasm C++ toolchain in this repository's
  dependencies, and a browser build that dragged one in would be a worse trade than the browser
  being the Rust path.
- **The browser's number is not a native number.** It is published as a browser's number.
- **The native path does not change.** This increment adds a target; it does not touch the two
  that exist.
