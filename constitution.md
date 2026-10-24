# Columnar Engine Constitution

**Project**: `columnar-engine` — a vectorized columnar query engine in Rust, with SIMD comparison
kernels reached over a C++ FFI seam.

**Purpose of this document**: this constitution is binding on every spec, plan and task in this
repository. A change to any principle below requires an amendment (see Governance). If a spec appears
to contradict this document, the spec is wrong — fix the spec.

**What this project is**: a CPU-only query engine whose defining property is that every published
number can be *re-derived* — by a reviewer, on their own machine, with one command, against a
baseline that is specified rather than implied.

**What this project is not**: it is not trying to beat DuckDB. DuckDB is a mature, world-class
system and it wins most queries outright. This project competes on exactly one axis: the
comparison kernel. The framing is always **"kernel X on query Y"**, never "this database versus
DuckDB". A claim that DuckDB beats is published alongside the claim that the kernel wins, because
a reader who cannot trust the loss will not trust the win either.

---

## Core Principles

### I. One Predicate, Five Implementations, One Answer

The engine's hottest operation is a single comparison: `(column OP threshold) → mask`. Five
implementations of it exist in this repository, and they MUST agree bit for bit, always.

| Implementation | Where | Role |
|---|---|---|
| Rust oracle | `columnar-kernels/src/oracle.rs` | the correctness reference |
| C++ scalar | `columnar-kernels/src/comparison.cpp` | the A/B **baseline** |
| C++ SIMD | `columnar-kernels/src/comparison.cpp` | the headline kernel (NEON / AVX2) |
| Rust SIMD | `columnar-kernels/src/rust_simd.rs` | measures what the FFI seam costs |
| C++ SIMD fallback | inside the C++ dispatch | the no-SIMD-machine path |

- **MUST**: every implementation agree with the oracle, bit for bit, on every input.
- **MUST**: this be **machine-checked in CI** by a differential test suite, not asserted in prose.
- **MUST**: the C++ scalar reference be **branchless** and compute the same predicate as the SIMD
  paths, so the published ratio measures vectorisation and not branch prediction. A speedup
  measured against a naive branching baseline is not a claim about SIMD.

**Rationale**: this project lives or dies on whether a reviewer believes the headline number. A
kernel that is fast but disagrees on one row in a million is not a fast kernel; it is a bug with a
speed number attached.

### II. The Baseline Is a First-Class Citizen

The C++ scalar reference is a permanent, separately-callable implementation — not a `#if 0` branch,
not deleted after the SIMD path works.

- **MUST**: the baseline be reachable at runtime through a distinct public flavor
  (`Flavor::Scalar`), so the A/B is a single run, not a rebuild.
- **MUST**: the README state the speedup ratio together with what each side of the ratio *is*, so
  a reader can judge whether the comparison is fair.

### III. NULL Is an Absent Bit, and It Never Matches

- **MUST**: a NULL be recorded by the absence of a validity bit, never by a sentinel value inside
  a value array.
- **MUST**: a row whose validity bit is clear **never match a predicate**, for every operator,
  **including `!=`**.
- **MUST**: a comparison involving a NaN value or a NaN threshold **never match**, for every
  operator, **including `!=`**. This is "unknown propagates", and it is stricter than SQL's
  treatment of NaN.

**Rationale**: these two rules are the easiest thing in a vectorized engine to get wrong, and the
failure is silent — the engine returns rows instead of an error. They are pinned by explicit tests
rather than left to the random cases.

### IV. Evidence Over Claims

Every performance or correctness claim MUST be accompanied by the command that reproduces it and
the machine it was measured on.

- **MUST**: every published number appear in a checked-in benchmark with a stored baseline, not
  in prose.
- **MUST**: `README.md` and `PERF.md` state the commit, the hardware, the optimization level, the
  warm/cold state and the thread count for every figure. A number without those qualifiers is a
  claim, not evidence.
- **MUST**: the benchmark harness emit machine-readable JSON. Prose numbers are hand-copied and
  drift.
- **MUST NOT**: publish a number copied from a third party. DuckDB's and Polars' numbers are
  quoted *only* as "measured on this machine, by this harness, against this file", with the
  version pinned.

### V. The Comparison Is Reproducible by a Stranger

- **MUST**: the engine build and run from one command, with no manual steps, no service to start,
  and no environment variable that must be set first.
- **MUST**: the external comparison run both incumbents in the *same* harness, against the *same*
  file, with the thread count pinned and the plan cache disabled. A comparison where DuckDB runs
  with its default thread count and this engine runs with one is not a comparison.
- **MUST**: the harness record the CPU model, the SIMD path in use, the toolchain, and the cache
  state alongside every result.

### VI. The Dataset Is Named Honestly

- **MUST**: the generated dataset be described as **TPC-H-shaped**, not as "TPC-H SF-1". It is
  generated by this repository, from a pinned seed, and it is not byte-identical to the official
  SF-1 file.
- **MUST**: the manifest record the seed, the scale factor, the row count and the null rate, so
  any result derived from it is re-derivable.
- **MUST NOT**: compare against an incumbent on a dataset the incumbent did not also read. Both
  engines read the same Parquet file.

### VII. Explicit Scope, No Silent Deferral

Anything not implemented MUST be written down as out of scope, with the reason.

- **MUST**: every spec carry a Scope section listing what is explicitly excluded.
- **MUST**: known simplifications (no SQL parser, single-threaded execution, `Utf8` dates, no
  decimal type) be stated on the README, not buried in a design document.

### VIII. Unsafe Is a Last Resort, and Always Documented

- **MUST**: every `unsafe` block carry a `SAFETY:` comment stating the invariant it depends on.
- **MUST**: `unsafe_op_in_unsafe_fn` be denied at the workspace level.
- **MUST**: the FFI boundary be as thin as it can be — the C ABI is two comparison functions and
  a probe, and nothing else crosses it.

---

## Performance Standards

These are the standards any change must hold or improve. They are the floor.

- **Kernel correctness**: bit-exact agreement across all five implementations. A mismatch is a
  critical defect, not a flaky test.
- **Comparison honesty**: the scalar baseline and the SIMD path run side by side, in the same
  process, on the same data, with the same warm-up.
- **Portability**: the engine must run, correctly, on a machine with no SIMD path at all. The
  scalar fallback is a correctness requirement, not a degraded mode.
- **Memory**: the per-query working set is bounded by a declared budget, and the planner refuses a
  query that exceeds it rather than exhausting the machine.
- **Benchmark hygiene**: a fixed dataset generated from a pinned seed, a fixed warm-up, a fixed
  sample count, and the warm/cold distinction stated explicitly.

---

## Development Workflow & Quality Gates

Workflow is Spec Kit: **Specify → Plan → Tasks → Implement**. No implementation begins without a
spec whose acceptance criteria are written and reviewable.

Required gates before merge. All are automated; none are advisory.

| Gate | Scope | Rule |
|---|---|---|
| Format | workspace | `cargo fmt --all -- --check` must be clean. Formatting is never argued about. |
| Lint | workspace, all targets | `cargo clippy --all-targets -- -D warnings`, with `all` + `pedantic` enabled workspace-wide. |
| Dependency policy | workspace | `cargo deny check` — advisories denied, wildcards denied, `rand` denied. |
| Test | workspace | All tests pass. |
| Differential | `columnar-kernels` | Every kernel implementation agrees with the oracle, bit for bit. |
| Property tests | `columnar-core`, `columnar-kernels` | Proptest suites pass; the kernel suite covers NULL, NaN and block boundaries. |
| Perf regression | workspace | No statistically significant regression vs. the stored baseline (Mann-Whitney, shared runners). |
| Perf budget | `columnar-kernels` | Absolute budgets met. **Pinned runners only.** |

**Definition of done** for any task: tests written and passing, lint clean, the differential suite
green, and — if performance-related — the benchmark re-run and the baseline updated deliberately,
with the change explained in the PR body.

---

## Governance

This constitution supersedes README guidance, code style preferences and individual preference.
Where they conflict, this document wins and the other document is corrected.

**Amendments** require: a written rationale, the specific principle(s) touched, the migration plan
for existing code, and an explicit `Last Amended` date bump in the version line below. Amendments
are versioned `MAJOR.MINOR.PATCH`; a change to Principles I, III or IV is always a MAJOR bump.

**Compliance is checked per spec**, not globally. Before a spec is accepted, it is checked against
every principle above; the spec's References section must cite which principle each non-obvious
requirement enforces.

**Complexity requires justification.** Any `unsafe` block, any `#[allow]`, any deviation from the
crate layout in `specs/001-columnar-core/plan.md` must be justified in review. These accumulate as
ADRs under `docs/adr/`.

---

**Version**: 1.0.0 | **Ratified**: 2026-10-23 | **Last Amended**: 2026-10-23
