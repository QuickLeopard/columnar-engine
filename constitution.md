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

### I. One Predicate, Six Implementations, One Answer

The engine's hottest operation is a single comparison: `(column OP threshold) → mask`. Six
implementations of it exist in this repository, and they MUST agree bit for bit, always.

| Implementation | Where | Role |
|---|---|---|
| Rust oracle | `columnar-kernels/src/oracle.rs` | the correctness reference |
| C++ scalar | `columnar-kernels/src/comparison.cpp` | the A/B **baseline** |
| C++ SIMD | `columnar-kernels/src/comparison.cpp` | the headline kernel (NEON / AVX2) |
| Rust SIMD | `columnar-kernels/src/rust_simd.rs` | measures what the FFI seam costs (NEON / AVX2 / `simd128`) |
| C++ SIMD fallback | inside the C++ dispatch | the no-SIMD-machine path |
| Rust `simd128` | `columnar-kernels/src/rust_simd.rs` | the browser's path, over WebAssembly |

The pushdown is **not** a seventh implementation. It runs the same kernel the filter runs, over the
reader's own value slice, so that the two paths cannot disagree by construction. See
[ADR-0010](docs/adr/0010-pushdown-equivalence.md), [spec 003](specs/003-kernel-pushdown/spec.md)
and [ADR-0015](docs/adr/0015-the-engine-keeps-the-mask.md).

- **MUST**: every implementation agree with the oracle, bit for bit, on every input.
- **MUST**: this be **machine-checked in CI** by a differential test suite, not asserted in prose.
- **MUST**: the C++ scalar reference be **branchless** and compute the same predicate as the SIMD
  paths, so the published ratio measures vectorisation and not branch prediction. A speedup
  measured against a naive branching baseline is not a claim about SIMD.
- **MUST**: a SIMD path on a **new target** be held to the same oracle, and the check MUST run where
  the path runs. A path that is only checked on the machine that built it is a path that has never
  been checked. See [ADR-0019](docs/adr/0019-the-kernel-in-a-browser.md).

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
- **MUST**: a predicate pushed down into the scan compute **the engine's** answer, not the
  reader's. Arrow's own float comparison kernels use IEEE *totalOrder*, under which `NaN != 5.0`
  is true and `-0.0 != 0.0`; a pushdown that used them would answer a different question from
  the filter, on the same query, on the same data. See [ADR-0010](docs/adr/0010-pushdown-equivalence.md)
  and [ADR-0015](docs/adr/0015-the-engine-keeps-the-mask.md).

**Rationale**: these two rules are the easiest thing in a vectorized engine to get wrong, and the
failure is silent — the engine returns rows instead of an error. They are pinned by explicit tests
rather than left to the random cases. The pushdown is a **second** place where they can be got
wrong, because the reader's comparison kernels disagree with them; so the pushdown implements the
engine's rules itself, and a differential test holds the two paths to the same answer, row for
row, on every operator and every edge the rules mention.

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
- **MUST**: when the engine is itself multi-threaded, the **symmetry** extend to every thread
  count published. Publishing a single-threaded comparison alone is not a symmetric comparison
  once the engine has threads: it flatters the engine, because the incumbents use every core by
  default. A published thread-count sweep MUST therefore run the engine and every incumbent at
  the *same* thread count at every point of the sweep.
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
- **MUST**: known simplifications (no SQL parser, single-threaded *operators*, `Utf8` dates, no
  decimal type, no vectorised arithmetic) be stated on the README, not buried in a design
  document.
- **MUST**: a simplification that a measurement says does not matter yet be **measured and
  published as such**, rather than assumed. The arithmetic aggregate is scalar, and the
  measurement says the arithmetic is 0.2 ms of the real Q6's 70 ms — so the scalar path is the
  right amount of work for now, and the number is published rather than the omission.
- **MUST**: a limit that is not the implementation's own be named as such. The parallel read is
  capped by the **file's row groups**, not by the machine's cores, and a published curve that
  flattens must say which of the two it is.

### VIII. Unsafe Is a Last Resort, and Always Documented

- **MUST**: every `unsafe` block carry a `SAFETY:` comment stating the invariant it depends on.
- **MUST**: `unsafe_op_in_unsafe_fn` be denied at the workspace level.
- **MUST**: the FFI boundary be as thin as it can be — the C ABI is two comparison functions and
  a probe, and nothing else crosses it.

---

## Performance Standards

These are the standards any change must hold or improve. They are the floor.

- **Kernel correctness**: bit-exact agreement across all six implementations. A mismatch is a
  critical defect, not a flaky test.
- **Comparison honesty**: the scalar baseline and the SIMD path run side by side, in the same
  process, on the same data, with the same warm-up.
- **Portability**: the engine must run, correctly, on a machine with no SIMD path at all. The
  scalar fallback is a correctness requirement, not a degraded mode.
- **Memory**: the per-query working set is bounded by a declared budget, and the planner refuses a
  query that exceeds it rather than exhausting the machine.
- **Benchmark hygiene**: a fixed dataset generated from a pinned seed, a fixed warm-up, a fixed
  sample count, and the warm/cold distinction stated explicitly.
- **Repeated runs, published medians**: a published timing MUST be the **median of at least three
  full runs**, and the raw reports MUST be committed. The **spread** MUST be published wherever it
  is wide enough to change a reader's conclusion, and a run that disagrees with the median MUST be
  published rather than dropped. The published table MUST be derivable from the committed reports
  by a checked-in script, so the prose and the evidence cannot drift apart.
- **Gates that can say "I could not tell"**: a gate MUST distinguish *no regression*, *within the
  noise*, *regression* and *inconclusive*. An inconclusive result MUST NOT be reported as a pass.
  A statistical gate MUST also carry a **practical threshold**, set from a measurement of the
  machine's own run-to-run drift, because a sensitive test on a shared machine finds the machine's
  drift every time — and a gate that fires on the machine is a gate nobody runs.
- **Order-preserving parallelism**: a parallel read MUST return the file's rows in the file's
  order, and MUST be bit-identical to the single-threaded read. `SUM` is not associative, so a
  read that ordered its rows by *completion* rather than by *position* would make the answer
  depend on the scheduler — and an answer that depends on the scheduler is not an answer.
- **Parallel operators, and the one that may never be parallel**: a parallel operator MUST be
  bit-identical to its serial counterpart — the same rows, in the same order, with the same
  values and the same validity bits — and the comparison MUST be made against the **rows**, not
  against an aggregate, because an aggregate cannot see a mis-ordered merge. The **aggregate** is
  exempt, and permanently: `SUM` is not associative, so a parallel aggregate's answer would depend
  on the block boundaries, and the block boundaries depend on the thread count. The exemption is a
  permanent property, not a deferral, and any proposal to parallelise it must be refused on those
  grounds.
- **A published worker count is the count that ran**: a plan that reports its parallelism MUST
  report the workers that **ran**, not the threads that were **asked for**. The two differ — the
  read's parallelism is capped by the file's row groups, and the operators' by the row count — and
  a reader who is shown the requested count will read the curve wrong.
- **A tuning constant that depends on the shape of the work MUST be measured for that shape**: a
  constant measured for one workload and reused for another is not a measured constant. The
  per-batch cost of a Parquet read scales with the number of columns it reads, so the batch size
  that is right for a two-column lazy read is not the one that is right for a sixteen-column eager
  read — and inheriting the reader's default is not a measurement either.
- **A parallel path's peak memory MUST be measured, not assumed**: a merge that copies its inputs
  holds two copies of the data at its peak, and one that moves them holds one. The difference is
  the difference between a budget that fits and one that does not.
- **A measurement's runtime is part of its conditions**: a number taken in a browser is published
  with the browser, the engine, the machine and the host's load, and it is **never** compared with a
  native number — a different runtime, a different clock and a different compiler make the ratio
  between them a claim about two engines rather than about one kernel. What a browser *can* compare
  is the two implementations **in the same process, over the same data**, and that is what it does.
- **A published number MUST be checkable where it was taken**: a page that measures the kernel in a
  browser MUST compare the two implementations' masks **before** it prints a throughput, and MUST
  print no throughput at all when they differ.
- **A row selection MUST have one representation, and it MUST be the cheap one**: a selection
  carried as a mask and then converted into a list of row ranges has **two** representations, and
  the conversion's cost is the list's length — a function of how *fragmented* the surviving rows
  are rather than of how many of them there are. A scattered mask over a random column yields a
  range per surviving run, so the list is longer than the row set it selects, and applying it
  costs several times the decode it was meant to save. The engine's mask is one bit per row, and
  it stays that way: the reader is given **no** row filter, and there is no range list anywhere in
  the engine.
- **A column that one operator streams over MUST NOT be materialised**: an intermediate that
  exists only to be read once, in order, and thrown away is a copy of the data for nothing. A
  **global** aggregate has one group and folds the survivors one after another, so it never needs
  their values aligned with anything, and the engine reads them where they lie — the projection
  materialises nothing, and the copy is never made. A column that an **expression** reads, or
  that a **grouped** aggregate keys on, *is* aligned with something, and it MUST be materialised.
  The distinction is a property of the operator's shape, so it is measured rather than assumed:
  the fold that follows the selection and the fold that tests it per row differ by a factor of
  ten, and the engine's answer is the first one.
- **A scan that filters MUST copy each surviving row once, and nothing else**: a scan that applies
  a predicate MUST NOT copy the rows the predicate rejects, and MUST NOT copy a column the query
  does not materialise. A predicate's column is read by the reader and then **dropped** — it is
  the predicate's input, not the query's output — and a materialised column's surviving rows are
  copied **once**, out of the reader's array and straight into the engine's own column, where the
  copy *is* the output rather than a step towards it. Filtering the batch and then copying the
  survivors out of the filtered batch copies every row twice and every column once, including the
  ones nobody wanted, and the two shapes are measured against each other before either is
  chosen: **the filter-and-copy cost 4.7 ms and the compaction 2.5 ms**. The scan's output is
  unchanged by this — it still holds the survivors, in the file's order, and the plan output
  still reports them — because the requirement is about the **copies**, not about the result.

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
| Perf regression | workspace | No statistically significant **and** practically significant regression vs. the stored baseline (Mann-Whitney, shared runners). A regression must be slower by more than the baseline's practical threshold, which is a measured number. |
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

**Version**: 2.0.0 | **Ratified**: 2026-10-23 | **Last Amended**: 2026-10-26

### Amendment history

**2.0.0 — 2026-10-26 — the kernel is in a browser, and there are now six implementations.**
A third SIMD target joined the project: WebAssembly's `simd128`, in the same module as NEON and
AVX2, reached from a page a reader can open. That makes Principle I's count wrong — there are
**six** implementations of the predicate now, not five — and the amendment rule says a change to
Principle I is a MAJOR bump, so this is one. The rule exists so that the principles the project
lives by cannot drift quietly, and this is exactly that case.

Two Performance Standards were added with it, both about the same thing:

- **a new target is a new implementation, and it is held to the same reference** — the `simd128`
  path must agree with the oracle bit for bit, and the check must run **where the path runs**;
- **a measurement's runtime is part of its conditions** — a browser's number is published with the
  browser, the engine and the machine, and is never compared with a native number.

The browser's own check is the page's: it compares the two masks before it prints a throughput,
and prints nothing if they differ. See [ADR-0019](docs/adr/0019-the-kernel-in-a-browser.md) and
[spec 013](specs/013-the-kernel-in-a-browser/spec.md).

**1.9.0 — 2026-10-26 — the scan filters only what it keeps, and the cold path's last gap is 1.1 ms.**
`filter-range` was the last query the engine lost on the cold path, and the plan output said it was
a **scan** gap and not an operator gap: the scan was 17.6 ms of a 19.9 ms query, against Polars'
15.7 ms. The scan was decomposed before anything was built, and the decomposition said something
the repository had got wrong: **the predicates cost 0.7–1.4 ms of a 16 ms scan**, so the README's
recommendation to "narrow the predicates in the engine's own domain" had nothing to recover, and it
is corrected in the open rather than quietly dropped.

What the decomposition did find was a defect in the scan's own shape. The lazy scan filtered the
**whole** batch with Arrow's `filter_record_batch` — copying every column of it, including the
predicate's column that the query does not materialise — and then copied the survivors a **second**
time out of the filtered batch into the engine's own column. The same work in three shapes, on the
same file, in the same process: **filter the whole batch, then copy — 4.7 ms**; **filter only what
is kept — 3.3 ms**; **compact once, into the chunk — 2.5 ms**. The engine now takes the third: it
writes the survivors straight out of the reader's array into its own column, one copy, of the values
that are kept.

The lazy path's **batch size** was found in the same place. The 8,192 rows *were* measured — in spec 002,
for a two-column lazy read — but the scan's shape has changed **three times** since: the pushdown became
the engine's own kernel, the mask stopped going to the reader as a list of row ranges, and the scan
stopped filtering the whole batch and copying the survivors twice. A constant measured for a shape that
no longer exists is not a measured constant for the shape that does. Swept for the lazy shape and then
measured with the two candidates **against each other**, with the order alternating on every round so
that a drifting machine cannot bias the result, 65,536 rows is **10–12% faster** than 8,192 —
repeatable across three runs. The constant is now 65,536, and the measurement that chose it is published
with it.

*Rationale*: the requirement added is that **a scan that filters MUST copy each surviving row once,
and nothing else**. It is the same reasoning as the two standards above it — *one representation,
and the cheap one*, and *a column that one operator streams over MUST NOT be materialised* — applied
to the scan's own copies. A copy of a row the predicate rejected, and a copy of a column the query
never asked for, are copies for nothing, and the measurement says they cost **2.2 ms of a 16 ms
scan**. The standard also says the *result* does not change: the scan still holds the survivors, in
the file's order, and the plan output still reports them, because the requirement is about the
copies and not about the answer.

*Principles touched*: IV is extended, not changed; the Performance Standards gain one requirement.
Neither I, III nor II is touched, so it is a MINOR bump.
*Migration plan*: the headline table was re-measured, because the committed reports carry the
figures. **The two cold sweeps were re-measured** — they measure the lazy scan, which this increment
changed. The **resident sweeps and the load sweep were not**: they measure the eager path, whose
code is untouched. The **load sweep** is a measurement of the eager decode, and the eager path does
not go through the scan this increment changed.

**1.8.0 — 2026-10-26 — the aggregate reads the source where it lies, and the last loss is gone.**
`filter-range` was the one query the engine lost: 14.6 ms resident against Polars' 11.5 ms, and
26.6 ms cold against Polars' 14.9 ms. Its stages said why — filter 1.5 ms, **project 7.0 ms**,
aggregate 2.4 ms — and the projection was materialising a column that **nobody would ever see**:
the aggregate summed it and threw it away, and a plan *with* an aggregation ignores the projection,
because the aggregate result is the output.

The fusion was prototyped and measured before it was built. Four ways of getting the same answer
over the real file and the real selection: gather the survivors and fold the gathered column
**9.06 ms**; **fold the source at the selection's rows 1.83 ms**; fold the source, testing the
selection for every row **17.68 ms**; and the whole column, with no selection at all, 3.73 ms. All
four produced bit-identical sums. The engine now folds the source where it lies, and the
projection materialises nothing: **`filter-range` is 6.9 ms resident against Polars' 12.2 ms**,
and the projection stage is 0.0 ms.

*Rationale*: the requirement added is that **a column that one operator streams over MUST NOT be
materialised**. The projection was the engine's most expensive habit, and it was paying for a copy
that no operator asked for: the aggregate reads the survivors once, in order, and discards them.
The same reasoning as the row selection's single representation — *one representation, and the cheap
one* — applied to a different thing. And the shape of the fold is a measurement, not a preference:
following the selection and testing it per row differ by a factor of ten, so "fold the source" is
not a single decision but two, and only one of them is the right one.

*Principles touched*: IV is extended, not changed; the Performance Standards gain one requirement.
Neither I, III nor II is touched, so it is a MINOR bump.
*Migration plan*: the headline table was re-measured, because the committed reports carry the
figures. The **sweeps were not re-measured**: they measure `q6`, whose aggregate is `count(*)` over
no column, and the fusion does not reach it. The **load sweep** was not re-measured either: it is a
measurement of the eager decode, which this increment does not touch.

**1.7.0 — 2026-10-25 — the engine keeps the mask, and the last big gap is gone.** The engine's
lazy scan of the two columns TPC-H Q6 needs took 82.4 ms against DuckDB's 32.0 ms on the same file
at the same thread count, and that 2.6× had never been split. Split, it came out as: Arrow's own
decode 11.9 ms, the engine's kernel 6.0 ms, the output 1.4 ms, and **~60 ms of nothing**. The
nothing was the mask on its way to the reader: a Parquet row filter is not a mask but a **list of
row ranges**, and Q6's discount predicate produced **2,775,138 ranges for 2,178,861 surviving
rows**. Reading a column through that list cost 50.0 ms against 7.8 ms without it.

The engine now evaluates the pushed-down predicates itself, over the reader's arrays, and keeps the
**mask**. The lazy scan of Q6's two columns is **26.2 ms** against DuckDB's 32.0 ms — the engine is
ahead on the headline query, where it was 2.6× behind — and the real Q6 is 70.2 ms against 68.0 ms,
which is level.

*Rationale*: the requirement added is that **a row selection MUST have one representation, and it
MUST be the cheap one**. Two representations of a selection is one too many: the mask is one bit
per row, and the range list is one entry per surviving *run*, so the list's length is a function of
how scattered the data is and not of how much of it survives. A reader that is handed a mask and
converts it to ranges pays for the conversion at every scan, on data the engine never chose, and
the cost is invisible in the scan's elapsed time — it is inside the reader. The same mistake, in a
different guise, is the one spec 008 found: a constant (the batch size) inherited rather than
measured. Here it is a representation inherited rather than chosen.

*Principles touched*: IV is extended, not changed; the Performance Standards gain one requirement.
Neither I, III nor II is touched, so it is a MINOR bump. Principle I's note about the pushdown
being "not a sixth implementation" is updated only in wording: the pushdown now runs the same
kernel over the reader's value slice from the engine's own scan loop rather than from inside the
reader, which makes the argument stronger rather than weaker.
*Migration plan*: the headline table and both sweeps were re-measured, because the committed
reports carry the figures. The load sweep was **not** re-measured: this increment does not touch
the eager path, and the load is a measurement of the eager path.

**1.6.0 — 2026-10-25 — the eager decode is parallel, and the last big number is gone.** The
engine's eager load was 1277.8 ms — the largest number in the repository, 144× the cost of
everything spec 007 parallelised — and the engine was *behind* both incumbents at it. It is now
**550.2 ms**, and the engine is ahead.

*Rationale*: the constitution asks for evidence over claims, and this increment is what that
requirement is for. The measurement that motivated it — that Arrow's own decode was 31% of the cost
and the engine's own conversion was 69% — was not a measurement anybody would have guessed at, and
it is what made the difference between a parallel decode and a parallel *conversion*. Three
defects came out of it, and each is a different kind of mistake: a constant that was inherited
rather than measured, a merge that grew with the square of the number of parts, and a merge that
repointed the indices of NULL rows.

The requirement added is that a **tuning constant that depends on the shape of the work MUST be
measured for the shape it is used on**. The lazy path measured its batch size for two columns and
the eager path inherited the reader's default for sixteen — and the per-batch cost scales with the
column count, so the eager path was rebuilding sixteen dictionary tables 5,860 times instead of
733. A constant measured for one shape and reused for another is not a measured constant.

Two things are stated rather than assumed. The parallel eager decode's **peak memory is about one
copy** of the data rather than two, because the merge moves the parts' values into the result
instead of copying them — and a claim about memory that is not measured is a claim about nothing.
And the merge's **serial share** is published: it has to be serial, because the merged
dictionary's order depends on the parts being merged in order, and at six parts it is about a
tenth of the decode.

*Principles touched*: IV is extended, not changed; the Performance Standards gain two requirements.
Neither I, III nor II is touched, so it is a MINOR bump.
*Migration plan*: the load sweep is published as `PERF.md` §2g, derived from
`bench-results/scaling-load-run{1,2,3}.json` by the same script that derives the other tables. The
headline table was re-measured, because the committed reports carry the load figure: the engine's
eager load went from 1277.8 ms to 550.2 ms while the incumbents' figures did not move by more
than the machine's noise, so the difference was the engine's.

**1.5.0 — 2026-10-25 — the operators are parallel now, and the aggregate never will be.** The
engine's filter and projection split their work across the workers (spec 007). The aggregate does
not, and the reason is permanent rather than provisional.

*Rationale*: the 1.3.0 standard made order-preserving parallelism a rule for the **read**, because
`SUM` is not associative. The same rule extends to the **operators** — a parallel filter or gather
must be bit-identical to its serial counterpart — and it extends with one **permanent exemption**:
the aggregate. A parallel aggregate folds each block and combines the partials, and the combined
answer would depend on the block boundaries, and the block boundaries depend on the thread count.
The engine's answer being reproducible is not a performance feature that can be traded away for
0.1 ms, so this is stated as a permanent property rather than as a deferral.

Two requirements are added beyond the extension. First, the equivalence MUST be checked against the
**rows** and not against an aggregate: a mis-ordered merge leaves a sum unchanged, so an
aggregate-level check passes on the defect it is meant to catch. The tests are mutation-verified
for exactly this reason. Second, a published worker count MUST be the count that **ran** — the
read's parallelism is capped by the file's row groups and the operators' by the row count, so the
requested count and the actual one differ, and a reader shown only the requested one will read the
curve wrong.

*Principles touched*: VII and IV are extended, not changed; the Performance Standards gain two
requirements. Neither I, III nor II is touched, so it is a MINOR bump.
*Migration plan*: the thread-count sweep is published in two modes — `PERF.md` §2e (cold) and
`PERF.md` §2f (resident) — derived from the `bench-results/scaling-*-run{1,2,3}.json` reports by the
same script that derives the headline table. The headline table was re-measured under the new code,
because the plan output embedded in the committed reports changed. The operators' share of the
resident query and of the cold one is published side by side, because the two answers differ by an
order of magnitude and quoting only the flattering one would be the thing this constitution exists
to prevent.

**1.4.0 — 2026-10-25 — a simplification a measurement says does not matter yet is published as
such.** The engine now evaluates arithmetic expressions over columns and runs TPC-H's Q6 as Q6
rather than as Q6's predicate with a `count(*)`. The expression evaluator is a scalar loop, which
is a simplification — and the measurement says the arithmetic is **0.2 ms of the real Q6's 142
ms**, because the expression is evaluated over the rows that survived the filter and not over
the file.

*Rationale*: the constitution asks for known simplifications to be stated rather than hidden, and
it has not said anything about what a simplification is *worth*. A simplification that a reader
cannot size is indistinguishable from a gap, and a gap that has never been measured is
indistinguishable from a decision. The new requirement is that such a simplification carries its
own measurement: the vectorised arithmetic kernel is not implemented, and the published number
says how much it would have been worth.
*Principles touched*: VII is extended, not changed; the Performance Standards are untouched.
*Not a MAJOR bump*: Principle I, III and IV are untouched.
*Migration plan*: the real Q6 is published as `q6-revenue` beside the `count(*)` query that was
measured before it, so the arithmetic's cost is a **difference between two published numbers**
rather than an assertion. `PERF.md` §3b and §5 carry the measurement and the statement.

**1.3.0 — 2026-10-25 — symmetry extends to every thread count, and the parallel read has an
order.** The engine's read is now parallel over the file's row groups (spec 005), so the rule that
the incumbents be pinned to the engine's thread count can no longer be satisfied by pinning
everything to one thread and calling it done: a one-thread table on its own flatters the engine,
because the incumbents use every core by default.

*Rationale*: the constitution asked for symmetry, and symmetry is a property of the *comparison*,
not of one table. The engine gets 5.1× from six workers against DuckDB's 3.2×, and the `q6` cold
gap narrows from 2.6× to 1.6× — a fact that a one-thread table alone would have hidden, in the
engine's favour. Two requirements are added: the symmetry MUST extend to every thread count
published, and a published curve that flattens MUST name whether the cap is the implementation's
or the file's. A third is added to the Performance Standards: a parallel read MUST preserve the
file's row order and be bit-identical to the single-threaded one, because `SUM` is not
associative and an answer that depends on the scheduler is not an answer.
*Principles touched*: V and VII are extended, not changed; this adds one Performance Standard.
Neither I, III nor IV is touched, so it is a MINOR bump.
*Migration plan*: the thread-count sweep is published as `PERF.md` §2e, derived from
`bench-results/scaling-run{1,2,3}.json` by the same script that derives the headline table. The
headline table was re-measured under the new code, because the published lazy `q6` figure was
89.1 ms and the same measurement now gives 84.4 ms — the incumbents' figures did not move, so the
difference was the harness's ordering and not the machine. Both numbers are published and the
difference is explained rather than quietly overwritten.

**1.1.0 — 2026-10-25 — repeated runs, published medians.** Added a performance standard
requiring that a published timing be the median of at least three full runs, that the raw reports
be committed, that the spread be published where it changes a conclusion, and that the published
table be derivable from those reports by a checked-in script.

*Rationale*: the machine this project is built on is shared, and a single run of the comparison
was found to vary by up to 43% on one figure. A single run is an estimate of a number that moves
when the machine is busy, and publishing one is publishing a sample as if it were a population.
*Principles touched*: none of I–VIII; this extends the Performance Standards only, so it is a
MINOR bump.
*Migration plan*: the comparison table in `PERF.md` was re-measured under the new rule; three
full runs were committed as `bench-results/compare-run{1,2,3}.json`, and
`scripts/medians.py` was added as the derivation. The kernel A/B table already published its
median over eight runs and its spread, and is unchanged.

**1.2.0 — 2026-10-25 — the performance gates exist, and they can say "I could not tell".** The
constitution listed two gates that had never been implemented: a perf-regression gate and a
perf-budget gate. Spec 004 implements both.

*Rationale*: a constitution that lists a gate nobody implemented is a document that has stopped
describing the project. Implementing the gates also produced a finding worth recording: the first
version of the regression gate was statistically correct and **useless** — thirteen false
positives in twenty checks on unchanged code, because the machine's throughput drifts by a percent
or two between one moment and the next and a test with thirty-two samples on each side detects
that drift every time. So the gate now requires a regression to be statistically significant
**and** practically significant, and the practical threshold is a measured number.
*Principles touched*: none of I–VIII; this extends the Performance Standards and the gate table
only, so it is a MINOR bump.
*Migration plan*: the gate is `just perf-gate`, a command rather than a test, because it measures
the machine. The baseline is committed at `bench-results/kernel-baseline.json`. The gate table's
regression rule gained the practical threshold; the budget rule is unchanged.
