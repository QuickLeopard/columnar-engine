# Columnar Engine

A vectorised columnar query engine in Rust, with a measurement harness that is willing to say the
engine loses.

This is **Project 02** of the portfolio: a columnar engine that stores columns in its own
dictionary-aware format, filters them with a hand-written SIMD kernel, and compares itself against
DuckDB and Polars under conditions it cannot choose for itself.

The first increment is storage → filter → project → aggregate: the TPC-H Q6 path.

---

## The headline

> **The comparison kernel on the Q6 predicate set: 8.7 G rows/s, 2.5× faster than a genuinely
> scalar reference and 1.5× faster than what clang's auto-vectoriser produces from the same
> loop.**
>
> Apple M4 Max, aarch64, NEON, one thread. Full conditions and the two other baselines are in
> [`PERF.md`](PERF.md).

Two things are deliberate about that sentence.

**"A genuinely scalar reference."** The first version of this number was measured against a
baseline that clang had auto-vectorised behind the author's back, so it was comparing two
vectorised implementations and calling it a vectorisation speedup. The scalar reference is now
compiled with `-fno-vectorize` into its own translation unit, and the claim is checked against the
disassembly. See [ADR-0006](docs/adr/0006-auto-vectorised-baseline.md).

**"What clang's auto-vectoriser produces from the same loop."** That is the baseline that decides
whether the hand-written kernel is worth its complexity. When the baseline was fixed, the
hand-written kernel was *losing* to it — 0.84× — because it extracted two lanes to scalar
registers every two rows. Rewriting the mask accumulation to stay in a vector register moved it to
1.50×. The intermediate, unflattering number is in the ADR too.

**What this is not:** this is a claim about a kernel on a query. It is not a claim that the engine
beats DuckDB or Polars, and where it does not, [`PERF.md`](PERF.md) says so with the same
precision — including the 1.92-second Parquet decode that makes the engine 63× slower than
DuckDB on a one-shot query against the file. (97.6% of that gap is the engine decoding all
sixteen columns when the query wants two, not the reader being slow; `PERF.md` breaks it down.)

---

## What is in the box

```
crates/
  columnar-core      types, validity masks, encodings, column chunks, selection vectors
  columnar-kernels   the comparison kernel: oracle, C++ scalar, C++ auto-vectorised,
                     C++ NEON/AVX2, Rust intrinsics -- all agreeing bit for bit
  columnar-testdata  a seeded, reproducible TPC-H-shaped `lineitem` generator
  columnar-engine    filter -> project -> aggregate, with late materialisation
  columnar-bench     the kernel A/B, the machine spec, the external comparison driver
  columnar-cli       dataset / query / bench / compare
docs/adr/            the decision record
specs/001-columnar-core/  the spec, the plan, the tasks
```

**One predicate, five implementations, one answer.** The comparison kernel is the project. There
is a plain Rust oracle, a C++ scalar reference compiled two ways, a C++ NEON/AVX2 path and a Rust
intrinsics path, and the differential test suite holds every one of them to the oracle's mask,
bit for bit, on random inputs and on the cases that are easy to get wrong: a NULL row never
matches (including under `!=`), a NaN never matches (including under `!=`), the tail block, the
empty input, the bits past the end.

---

## Running it

```bash
# 1. Build and check. This is the whole gate.
just check

# 1a. Compile the x86-64 half of the kernel from this ARM64 machine.
#     The AVX2 path only builds on x86-64, so without this it rots
#     silently. It rotted silently once already.
just check-x86

# 2. Generate the dataset. Deterministic: the same (scale factor, seed) gives
#    byte-identical output, and the manifest records what it was.
just dataset                      # 6,000,000 rows, 231.6 MiB, about 2.8 seconds

# 3. Run a query and read the plan output.
just query                        # the TPC-H Q6 predicate set

# 4. Measure the kernel, four ways.
just bench

# 5. Measure the engine against DuckDB and Polars.
just compare

# 6. Verify the AVX2 path's correctness, under Rosetta 2.
#    Correctness only -- the benchmark refuses to measure with the
#    override in force, because emulated timings mean nothing.
just verify-avx2
```

`just compare` needs `DuckDB` on `PATH` or in `tools/bin/`, and the `polars` Python package
installed. It says so and skips the missing comparator rather than failing quietly — and it
cross-checks every answer against the engine's own, because a comparison where the two sides
computed different values is not a benchmark.

---

## What is measured, and what is not

| | status |
|---|---|
| The comparison kernel, on AArch64 / NEON | **measured**, 5 runs, spread published |
| The comparison kernel, on x86-64 / AVX2 | **performance not yet measured** — no x86-64 machine exists. Its **correctness is verified**, on this ARM64 Mac, under Rosetta 2 |
| The engine's filter, project and aggregate | **measured**, against DuckDB 2.0.0 and Polars 1.36.1 |
| The engine's Parquet reader | **measured**, and it is the engine's weakest part |
| Multithreading | **not implemented** — the engine is single-threaded in this increment, and so are the incumbents in the comparison |

The x86-64 row is the important one, and it has two halves.

The blueprint's headline number ("AVX2 6.7× scalar" on EPYC/DDR5/NVMe) is **not reproducible on
the machine this was built on**, and this repository does not publish it, does not estimate it, and
does not quote it as its own.

But "no x86-64 machine" does not have to mean "no verification". Rosetta 2 on Apple Silicon
executes AVX2 instructions correctly while reporting no AVX2 in CPUID, so the kernel's runtime
probe correctly hides the AVX2 path. `just verify-avx2` overrides the probe, runs the differential
suite under the emulator, and uses a row counter to prove the AVX2 instructions actually executed
and produced the oracle's mask, bit for bit:

```
-- with COLUMNAR_FORCE_SIMD=avx2 --
    forced        : avx2
    simd_kind     : avx2
    avx2 rows     : 1024
    matches oracle: true
```

That is a correctness claim, and only a correctness claim. Under Rosetta the instructions are
*emulated*, so the benchmark harness **refuses to measure** while the override is in force — a
timing taken there would measure the translator rather than any hardware. AVX2 performance numbers
require an x86-64 machine with AVX2 and are published only from one.

Finding this also exposed a defect worth naming: **the AVX2 Rust path had never been compiled by
anyone.** It had two type errors in it. It is now gated by `just check-x86` and by CI, so that
class of rot cannot recur. See
[ADR-0009](docs/adr/0009-avx2-without-x86-hardware.md).

---

## Governance

This repository has a constitution, and it is not decoration.

- [`constitution.md`](constitution.md) — eight numbered principles, the quality gates, and the
  governance rules.
- [`docs/adr/`](docs/adr/) — every decision that cost something, including the two that cost a
  published number.
- [`specs/001-columnar-core/`](specs/001-columnar-core/) — the spec, the plan and the tasks, each
  requirement traced to a principle or to a review finding, with an explicit out-of-scope table.

The principles that shaped the code most:

- **I. One Predicate, Five Implementations, One Answer.** Every kernel implementation is checked
  against the oracle, bit for bit.
- **II. The Baseline Is a First-Class Citizen.** Two baselines, not one: a genuinely scalar
  reference and the compiler's own vectorisation. The second is the one that decides whether the
  hand-written kernel earns its keep.
- **III. NULL Is an Absent Bit, and It Never Matches.** For every operator, including `!=`.
- **IV. Evidence Over Claims.** Every published number carries its commit, its hardware, its SIMD
  path, its optimisation level, its warm/cold state and its thread count.
- **V. The Comparison Is Reproducible by a Stranger.** `just` is the entry point; the comparator
  drivers are checked in; the dataset is a function of a seed.
- **VI. The Dataset Is Named Honestly.** It is *not* the official TPC-H SF-1 file, and the
  manifest says so in as many words.
- **VII. Explicit Scope, No Silent Deferral.** The Q6 comparison uses Q6's predicate with
  `count(*)`, because this increment's aggregate sums a column and does not evaluate arithmetic
  over columns. That limitation is written down, not hidden.

---

## Status

| | |
|---|---|
| Spec | `specs/001-columnar-core/` — R1..R5, with the out-of-scope table |
| Increment | storage → filter → project → aggregate (the TPC-H Q6 path) |
| Tests | 90, all green, including the differential kernel suite and the x86-64 cross-check |
| Lint | `clippy --workspace --all-targets` with `all` + `pedantic` as errors; zero warnings |
| CI | fmt, clippy (ubuntu + macos), test, the differential job, an x86-64 cross-compile check, `cargo deny`, a no-committed-dataset check, and the two benchmark runners |
| Next | the Parquet reader (the 39.3× cold gap), the projection/aggregate fusion, the x86-64 measurement |
