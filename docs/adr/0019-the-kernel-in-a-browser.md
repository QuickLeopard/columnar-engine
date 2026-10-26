# ADR-0019 — The kernel runs in a browser

**Status**: accepted
**Date**: 2026-10-26
**Spec**: [spec 013](../../specs/013-the-kernel-in-a-browser/spec.md)
**Supersedes**: nothing

## The decision

The engine's comparison kernel is compiled to WebAssembly and runs in a browser, on a page that
rebuilds the benchmark's own column, runs the oracle and the `simd128` path over it, and compares
their masks **bit for bit** before it prints a number.

Three parts, and each of them is a decision rather than an implementation detail:

1. **The browser is a third SIMD target, not a fallback.** WebAssembly's `simd128` is a 128-bit
   vector ISA, and Rust exposes it as intrinsics in `core::arch::wasm32`. The browser runs a real
   vectorised kernel, not the scalar loop.
2. **The browser has no C++.** The C++ translation units are not built for `wasm32`. The browser's
   path is the **Rust** intrinsics path, and the browser's reference is the same plain Rust oracle
   every other target is held to.
3. **The browser's answer is the native's answer.** The column the browser measures over is the
   native benchmark's column, as a multiset, rebuilt from two small histograms — so the number of
   matching rows is **12,726,115**, the same number the native benchmark publishes.

## Why

The kernel is the project: one predicate, several implementations, one answer. That claim was only
checkable by a reader who could build Rust and run the differential suite. A link is the thing that
travels, and a claim that a reader can check in their own browser is worth more than a claim a
reader has to take on trust.

The alternative — a JavaScript re-implementation of the predicate for the page — was rejected
immediately. It would have been a **seventh** implementation, written by hand, that nothing held to
the oracle. That is the exact failure mode the project exists to avoid.

## The measurement

It is taken in a real browser, on a named browser and a named machine, and it is committed with
its raw reports. The table, the conditions and the spread are in the commit that follows this
one — the measurement was taken on **this commit's tree**, and the reports are committed beside
it.

The conditions are not optional, and they are these: the browser, the engine, the machine, the
host's load average, the warm-up, the rounds, and the commit.

## What this is not

**The browser's number is not a native number.** Putting the two side by side would be a claim
about WebAssembly and a browser's throughput on a busy laptop, not about the kernel, and the
repository does not make that claim. The two numbers are never compared — not in this ADR, not in
`PERF.md`, not in the README.

What the browser *can* compare is the two implementations **in the same process, over the same
data, with the same clock** — and that is the comparison it makes: the `simd128` path against the
oracle.

The browser's absolute numbers are also the noisiest thing this repository publishes, because
the browser is the one runtime the repository has no control over. The measurement section gives
the spread, and the spread is wide.

## The check that matters

The page compares the two masks **before** it prints a throughput. If they differ, it prints the
disagreement and no number at all. This is the same rule the native harness applies — every
flavor must agree on the match count, or the benchmark fails rather than publishing a faster wrong
number — applied where the number is produced.

And the match count is checked against the native's: the browser must arrive at **12,726,115**,
the number the native benchmark publishes. The column is rebuilt from a histogram of the file's
eleven distinct discount values, and the histogram is the file's:

- the whole column's counts, and
- the **first 2,000,000 rows'** counts — because the native benchmark *tiles* the column to reach
  20,000,000 rows, and the tile's partial copy has to be reproduced exactly.

The browser's column is assembled from five segments — prefix, whole-minus-prefix, whole, whole,
prefix — whose multiset is `3 × whole + prefix`, which is the native's multiset exactly.

**The row order is not the file's.** The browser has no Parquet reader, and it does not pretend to
one: the values are interleaved by a deterministic generator, so the multiset is exact and the
order is not sorted. The oracle is a row-at-a-time loop with a branch, and a column in value order
would hand its branch predictor an unrealistically easy time. A native test asserts that the
column did **not** come out in value order.

## The trade

The browser's build needs **no toolchain beyond Rust**: no `wasm-bindgen`, no `npm`, no
emscripten, no second compiler. `rust-lld` ships with the Rust toolchain, and the module is a
plain `cdylib` with an explicit ABI of eight exported functions.

The commit the module was built from comes from the **server**, not from the
module: cargo caches a build script, so a commit baked into the module can be
a commit the module was not built from. The server reads git when the page
asks, and stamps the same commit into the report it writes.

What that costs is the C++ path, which does not go to the browser. There is no wasm C++ toolchain
in this repository's dependencies, and adding one would cost every reader a second toolchain to
publish one more flavour of the same predicate. The browser's Rust path gives up the FFI seam
entirely — and the native A/B already says what that seam costs, which is nothing.

## The consequences

- `columnar-kernels` builds for `wasm32-unknown-unknown`: the C++ is skipped, the FFI module is
  compiled out, and the three C++ flavors fall back to the oracle there. The browser's build says
  what it is.
- The `simd128` path's **`ne` is spelled out** rather than taken from `f64x2.ne`. The WebAssembly
  SIMD specification defines `ne` as the **unordered** not-equal, so it answers `true` when either
  operand is a NaN. The engine's rule is that a NaN never matches, for every operator including
  `!=`, so the ordered form — `gt | lt` — is the one that is correct. A path that used the
  instruction directly would have been wrong on every NaN.
- The page is the differential check for the browser's path, and it runs in **every reader's**
  browser, not only in CI.
- The constitution is amended to **2.0.0**: Principle I now covers six implementations rather than
  five, and two Performance Standards were added — a new target is held to the same reference and
  the check runs where the path runs, and a measurement's runtime is part of its conditions.

## What was rejected

- **A JavaScript re-implementation of the predicate.** A seventh implementation, written by hand,
  held to nothing. Rejected.
- **The whole engine in the browser.** Parquet, the planner and the scan would have to come too,
  and the browser would be measuring the engine rather than the kernel. The browser runs the
  kernel.
- **A wasm C++ toolchain.** A second compiler for every reader, to publish one more flavour of the
  same predicate.
- **Comparing the browser's number with the native one.** A claim about two runtimes, not about
  one kernel.
