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
> Apple M4 Max, aarch64, NEON, one thread for the headline table; the thread-count sweep runs
> the same file at 1, 2, 4, 6, 8 and 14 threads. Full conditions and the two other baselines
> are in [`PERF.md`](PERF.md).

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
precision.

**And the engine itself has two ways to read a file.** The eager one decodes every column before
the query starts. The lazy one resolves the plan against the file's *schema*, reads only the
columns the plan needs, and pushes the plan's predicates into the Parquet reader so the rows that
fail them are dropped during the scan. For TPC-H Q6 that is **two columns of sixteen** and
**12.1 MiB of the file's 231.6 MiB**:

```
$ just compare
cold · read the Parquet file and answer in one go
query          engine   engine-lazy   duckdb   polars
--------------------------------------------------------
scan         564.7           0.1      2.0      1.1
filter-numeric     583.3           5.2     23.0     45.5
filter-string      583.7          13.6     23.0    169.2
filter-range       588.9          17.5     33.0     16.4
q6          595.1          20.7     33.0    200.6
q6-revenue   612.3          63.1     72.0    203.4
```

**The lazy path is 28.8× faster than the eager one on `q6`**, and it beats Polars on every cold
query except `filter-range`. It beats DuckDB on `filter-numeric` — 5.2 ms against 23.0 ms — and
on `filter-string`, and on `scan`, and on `q6-revenue`. On `q6` it is **1.6× ahead**, where it used
to be 2.6× behind. The gap was never pruning: DuckDB prunes nothing on this file either (its own
plan says `Row Groups Scanned: 6 / 6`, and the file has no page index). It was the engine's own
mask, on its way to the reader as a **list of 2.78 million row ranges**, applied one range at a
time. Measured and attributed in [`PERF.md`](PERF.md), not glossed.

**`q6-revenue` is TPC-H Q6 itself** — `sum(l_extendedprice * (1 - l_discount))` — and the engine
runs it: 63.1 ms cold against DuckDB's 72.0 ms, and 16.6 ms resident against DuckDB's 51.0 ms.
The arithmetic is **0.5 ms of the 63 ms**, because it is evaluated over the 110,915 rows that
survived the filter and not over the 6,000,000 rows of the file. The query's cost is reading
`l_extendedprice`, not computing with it. See
[ADR-0012](docs/adr/0012-arithmetic-expression.md).

**And the query the engine used to lose, it now wins.** `filter-range` — `sum(l_quantity)` over a
two-predicate range — was the one loss in the table: 14.6 ms resident against Polars' 11.5 ms,
because the projection materialised a column that the aggregate only streamed over. The aggregate
now folds the source **where it lies**, over the filter's selection, and the projection materialises
nothing at all: **7.5 ms resident against Polars' 12.4 ms and DuckDB's 20.0 ms**, and the
projection stage is 0.0 ms. See
[ADR-0016](docs/adr/0016-the-aggregate-reads-in-place.md).

**And the scan itself got faster, because it was copying rows nobody asked for.** The lazy scan
used to filter the **whole** batch — including the column the predicates read and the query does
not materialise — and then copy the survivors a **second** time out of the filtered batch. It now
writes the survivors straight out of the reader's array into the engine's own column: one copy, of
the values that are kept. The lazy path's batch size was swept for the lazy shape as well, and it
was **inherited** rather than measured — 65,536 rows is 10–12% faster than the 8,192 it was
using. The whole lazy path moved: `q6` **20.7 ms**, `q6-revenue` **63.1 ms**, `filter-string`
**13.6 ms**, `filter-range` **17.5 ms**. See
[ADR-0017](docs/adr/0017-the-scan-filters-only-what-it-keeps.md).

**And the engine now has threads too.** The read splits the file's row groups across the workers
and merges them back in the file's order, so the answer does not depend on the scheduler. On the
identical query, with the incumbents at the identical thread count:

```
$ just sweep
threads   workers     engine   engine-scan     duckdb     polars
---------------------------------------------------------------
      1        1      20.5          20.4       33.0      196.9
      2        2      10.3          10.3       22.0      103.7
      4        4       7.2           7.2       15.0       69.9
      6        6       4.0           3.9       10.0       39.6
```

**The engine gets 5.2× from six workers, against DuckDB's 3.3× from six, and it is ahead at every
thread count.** The engine's curve is steeper because its query is almost entirely scan, and the
scan is the part that parallelises. The parallelism is capped by the file's **six row groups**, so
the curve is flat past six — the ceiling is the file's, not the machine's. See
[ADR-0011](docs/adr/0011-parallel-decode.md).

**The operators are parallel too now, and that is where the engine is furthest ahead.** The filter
and the projection split their work across the workers; the answer does not depend on the thread
count, and that is checked row by row rather than asserted. Measured resident, with the incumbents
at the identical thread count:

```
$ just sweep mode=resident query=q6
threads   workers     engine   engine-ops     duckdb     polars
---------------------------------------------------------------
      1        1       6.6           6.5       43.0       67.7
      2        2       3.9           3.8       24.0       35.2
      4        4       2.5           2.5       13.0       17.9
      6        6       2.0           2.0       10.0       12.5
      8        8       1.9           1.9        8.0       10.1
     14       14       1.8           1.8        7.0        8.9
```

**The engine is 6.5× faster than DuckDB at one thread and 3.9× faster at fourteen.** Its
operators' worker count is not capped by the file's six row groups, so it keeps going where the
read stops. The **aggregate** is not parallelised and never will be: `SUM` is not associative, and
an answer that depends on the block boundaries is not an answer. See
[ADR-0013](docs/adr/0013-parallel-operators.md).

**The pushdown runs the engine's own comparison kernel**, over Arrow's raw value slice — so the
pushdown and the filter are no longer two implementations that have to be made to agree. They are
one implementation. That took the pushdown from 42.9 ms to 6.1 ms on `q6`, and it is why the
equivalence is now structural rather than merely tested. See
[ADR-0010](docs/adr/0010-pushdown-equivalence.md).

**The eager path got faster twice.** The first time was not a change to the eager path at all:
the two paths now share one decode function, and that function stopped copying a dictionary
column's distinct values out of every batch. The lazy path never paid that cost; the eager path
paid it 5,860 times over. Putting the copy back and re-measuring gives 1890 ms, so the 1.60× is
a measurement rather than a story. See
[`PERF.md` §2b-bis](PERF.md#2b-bis-the-eager-path-got-faster-too-and-it-is-measured).

**The second time was the eager decode, and it is now parallel.** The *load* — the time to get
the file into memory — was **1277.8 ms**, the largest number in the repository and the one place
the engine was behind both incumbents. It is now **550.2 ms**, and at one thread the engine is
**2.2× ahead** of DuckDB on the same file:

```
$ just sweep mode=load
threads   workers     engine    duckdb    polars
---------------------------------------------------------------
      1        1      550.2    1207.0     995.5
      2        2      378.6     607.0     513.4
      4        4      304.6     412.0     349.5
      6        6      226.2     221.0     189.3
      8        6      223.7     227.0     186.6
     14        6      223.0     224.0     187.9
```

**Most of that was not the parallelism — it was a constant the eager path had inherited rather
than measured.** The Parquet reader's default batch size is 1,024 rows, and the per-batch cost
scales with the number of columns it reads; the eager path reads all sixteen, so it rebuilt
sixteen dictionary tables 5,860 times instead of 733. Fixing that one constant took the
one-thread load from 1145.6 ms to 587.0 ms, and the parallel decode took it the rest of the way.
The merge is **one pass, and it moves** the parts' values instead of copying them, so the peak
memory stays at about one copy of the data. The merged dataset is **bit-identical** to the serial
one — checked row by row, including the dictionary's order and the indices of the rows that are
NULL. See [ADR-0014](docs/adr/0014-parallel-eager-decode.md).

**And the lazy path got faster three times.** Its scan of Q6's two columns was **78.4 ms** and is
now **20.7 ms**, against DuckDB's 33.0 ms on the same file at the same thread count: the engine is
**1.6× ahead** on the headline query, where it was 2.6× behind. The first cause was not the decode
and not the comparison — it was the pushdown's mask, handed to the reader as a **row filter**,
which parquet-rs turns into a list of **2,775,138 row ranges** for 2,178,861 surviving rows and
applies one range at a time. Reading a column through that list costs 50.0 ms; without it, 7.8 ms.
The engine keeps the mask now — one bit per row — and applies it itself. See
[ADR-0015](docs/adr/0015-the-engine-keeps-the-mask.md). The second and third were the scan's own
shape: it was copying the rows it discards and the column nobody asked for, and its batch size was
a constant measured for a scan shape that has since changed three times. See
[ADR-0017](docs/adr/0017-the-scan-filters-only-what-it-keeps.md).

(The table is the median of three full runs; `python3 scripts/medians.py` re-derives it from the
committed raw reports.)

**The pushdown reproduces the engine's filter semantics exactly** — a NULL never matches, a NaN
never matches, for every operator including `!=` — which is stricter than both SQL and Arrow's
own comparisons, whose float kernels use IEEE *totalOrder* and would answer `NaN != 5.0` with
*true*. See [ADR-0010](docs/adr/0010-pushdown-equivalence.md). The equivalence is not asserted;
it is checked, row for row, by a differential test over every operator and every edge.

---

## What is in the box

```
crates/
  columnar-core      types, validity masks, encodings, column chunks, selection vectors
  columnar-kernels   the comparison kernel: oracle, C++ scalar, C++ auto-vectorised,
                     C++ NEON/AVX2, Rust intrinsics -- all agreeing bit for bit
  columnar-testdata  a seeded, reproducible TPC-H-shaped `lineitem` generator
  columnar-engine    filter -> project -> aggregate, with late materialisation
  columnar-bench     the kernel A/B, the machine spec, the external comparison driver,
                     the performance gate, the thread-count sweep
  columnar-cli       dataset / query / bench / compare / gate / sweep
docs/adr/            the decision record
specs/001-columnar-core/  the spec, the plan, the tasks
specs/002-lazy-columnar-scan/  the lazy scan: the plan, the pushdown, the equivalence test
specs/003-kernel-pushdown/  the pushdown runs the engine's own kernel
specs/004-performance-gates/  the regression gate and the budget gate
specs/005-parallel-decode/  the parallel read, and the order it preserves
specs/006-arithmetic-aggregate/  the arithmetic aggregate, and the real Q6
specs/007-parallel-operators/  the parallel filter and projection, and the aggregate that is not
specs/008-parallel-eager-decode/  the parallel eager decode, and the batch size it inherited
```

**One predicate, five implementations, one answer.** The comparison kernel is the project. There
is a plain Rust oracle, a C++ scalar reference compiled two ways, a C++ NEON/AVX2 path and a Rust
intrinsics path, and the differential test suite holds every one of them to the oracle's mask,
bit for bit, on random inputs and on the cases that are easy to get wrong: a NULL row never
matches (including under `!=`), a NaN never matches (including under `!=`), the tail block, the
empty input, the bits past the end.

---

## Running it

**Prerequisites**: a Rust toolchain (`rustup`), `just` (`cargo install just`), and Python 3 for the
two scripts under `scripts/`. `just` is the single entry point — every recipe in the `justfile` is
the documented way to do a thing.

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
just query-lazy                   # the same, reading only what the plan needs

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
| The engine's **arithmetic aggregate** | **measured** — the real TPC-H Q6, `sum(l_extendedprice * (1 - l_discount))`, against the same SQL on the same file |
| The engine's Parquet reader | **measured**, in two modes: eager (all sixteen columns) and lazy (two of sixteen, with the predicates pushed in) |
| The **read** under several thread counts | **measured** — the engine and both incumbents, at the same thread counts, on the same file: [`PERF.md` §2e](PERF.md#2e-the-thread-count-sweep) |
| The **query** under several thread counts | **measured** — the same sweep with the data already in memory, which is where the operators are the measurement: [`PERF.md` §2f](PERF.md#2f-the-resident-sweep-where-the-operators-are-the-measurement) |
| Multithreading of the **operators** | **measured** — the filter and the projection are parallel, and the answer does not depend on the thread count. The **aggregate** is not parallelised and never will be: `SUM` is not associative |
| The **eager** path's decode | **measured** — the eager path splits the file's row groups across the workers, and the merged dataset is bit-identical to the serial one's: [`PERF.md` §2g](PERF.md#2g-the-load-sweep--where-the-engines-biggest-number-was) |
| The **lazy** path's scan, decomposed | **measured** — Arrow's own decode, the engine's kernel, and the mask are separated, and the decomposition is in the tree as `crates/columnar-engine/examples/lazy_scan_decomposition.rs`: [ADR-0015](docs/adr/0015-the-engine-keeps-the-mask.md) |

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
- [`specs/002-lazy-columnar-scan/`](specs/002-lazy-columnar-scan/) — the lazy scan: a plan that
  resolves against a schema, a reader that is given only the columns the plan needs, and a
  predicate pushdown that reproduces the engine's filter semantics exactly.

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
- **VII. Explicit Scope, No Silent Deferral.** The real Q6 — `sum(l_extendedprice * (1 -
  l_discount))` — is now measured, beside the `count(*)` query that was measured before it, so the
  cost of the arithmetic is a **difference between two published numbers** rather than an
  assertion. What is *not* measured is stated in [`PERF.md` §5](PERF.md#5-not-measured): the
  arithmetic is not vectorised, the aggregate is not parallelised, and the eager path's decode is
  not parallel.

---

## Status

| | |
|---|---|
| Specs | `specs/001-columnar-core/` — R1..R5, with the out-of-scope table · `specs/002-lazy-columnar-scan/` — R1..R8, the lazy scan and the pushdown · `specs/003-kernel-pushdown/` — R1..R8, the pushdown running the engine's kernel · `specs/004-performance-gates/` — R1..R8, the regression and budget gates · `specs/005-parallel-decode/` — R1..R10, the parallel read and the thread-count sweep · `specs/006-arithmetic-aggregate/` — R1..R10, the arithmetic aggregate and the real Q6 · `specs/007-parallel-operators/` — R1..R6, the parallel filter and projection · `specs/008-parallel-eager-decode/` — R1..R6, the parallel eager decode and the batch size it inherited · `specs/009-the-engine-keeps-the-mask/` — R1..R8, the lazy scan's decomposition and the mask the engine keeps · `specs/010-fuse-the-projection-into-the-aggregate/` — R1..R9, the global aggregate reading the source in place · `specs/011-the-scan-filters-only-what-it-keeps/` — R1..R6, the scan compacting the survivors once, and the batch size it inherited |
| Increment | storage → filter → project → evaluate → aggregate (the TPC-H Q6 path), with two ways to read a file: eager and lazy |
| Tests | 222, all green, including the differential kernel suite, the lazy-scan differential suite, the expression suite, the parallel-operator suite, the parallel-eager suite, the fused-aggregate suite, the performance-gate suite and the x86-64 cross-check |
| Lint | `clippy --workspace --all-targets` with `all` + `pedantic` as errors; zero warnings |
| CI | fmt, clippy (ubuntu + macos), test, the differential job, an x86-64 cross-compile check, `cargo deny`, a no-committed-dataset check, the published-figures check, and the two benchmark runners. **This repository is private, so CI consumes the account's Actions minutes** — if billing is blocked, every job fails at once with a billing message rather than a test failure. `just check` covers all of it locally and does not. |
| Gate | `just perf-gate` — a Mann-Whitney test against a stored baseline, with a measured practical threshold. It can also say *inconclusive*, which is not a pass. |
| Next | **The cold path's one remaining gap is `filter-range`, and it is 1.1 ms.** The lazy path is 17.5 ms against Polars' 16.4 ms, and the scan that holds it has a floor: Arrow's own decode of the two columns is 10.7 ms of the 16.1 ms the scan takes. **The next thing is *not* what this file used to say** — it used to say "narrowing the predicates in the engine's own domain", and the measurement says the predicates cost 0.7–1.4 ms of a 16 ms scan, so there was nothing there to recover. The scan's own copy was the defect, and it is fixed. What is left: the conversion from Arrow's arrays into the engine's own column format, the morsel driver (still worth ~0 ms in the cold path, because the pushdown leaves the operators nothing to do), and the x86-64 measurement, which stays blocked until there is an x86-64 machine |
