# Columnar Engine

A vectorised columnar query engine in Rust, with a measurement harness that is willing to say the
engine loses.

This is **Project 02** of the portfolio: a columnar engine that stores columns in its own
dictionary-aware format, filters them with a hand-written SIMD kernel, and compares itself against
DuckDB and Polars under conditions it cannot choose for itself.

The first increment is storage → filter → project → aggregate: the TPC-H Q6 path.

---

## Try it

```bash
just demo
```

One command, about a minute, three things: the kernel measured four ways, the engine's own plan,
and the engine against DuckDB, Polars and DataFusion. It generates the dataset if it is not there.
The third part needs `DuckDB` on `PATH`, the `polars` Python package, and the Rust peer built with
`just peer-build`; if any of them is missing, the comparator is **skipped and named**, not faked —
the engine's own numbers still print.

**The kernel: four implementations, one answer.**

```
kernel A/B · 20000000 rows · 5 iterations · column l_discount >= 0.04
scalar = compiled with -fno-vectorize · cpp-auto = the same source with the vectoriser on
flavor      rows/sec      ns/row   vs scalar  vs cpp-auto   matches
---------------------------------------------------------------
scalar          3.517 G       0.284     1.00x       0.69x    12726115
cpp-auto        5.070 G       0.197     1.44x       1.00x    12726115
cpp-simd        8.341 G       0.120     2.37x       1.65x    12726115
rust-simd       8.045 G       0.124     2.29x       1.59x    12726115
```

**The `matches` column is the point.** 12,726,115 in all four: a C++ reference clang was told not to
vectorise, the same C++ with the vectoriser on, a hand-written NEON kernel over a C ABI, and a Rust
`std::arch` intrinsics kernel with no FFI at all. A fifth implementation — the plain Rust oracle —
is not timed here, because it is the reference the other four are held to: the differential suite
compares the **masks bit for bit**, not the counts (`just differential`).

**The ratios move; the matches do not.** That block is one run on a shared machine. The published
figures are the medians of **six** campaigns, and they say `cpp-simd` is **2.48×** the scalar
reference and **1.47×** what clang's auto-vectoriser produces from the same loop — this run said
2.37× and 1.65×. Both are true; the ratio is a timing and the machine is not isolated, the count of
matching rows is not a timing and does not move. The conditions, the medians and the spread are in
[`PERF.md`](PERF.md) §1.

**The engine's own plan.**

```
optimization order: predicate pushdown -> projection pushdown -> late materialization -> aggregate
stage      rows in  rows out  elapsed
scan       6000000  315203    15.2 ms
project    315203   315203    0.0 ms
aggregate  315203   1         0.0 ms
materialised columns: (none)
columns read: l_discount, l_shipdate
pushed down: l_discount >= 0.04 and l_discount <= 0.07, l_shipdate >= '1994-01-01' and l_shipdate < '1995-01-01'
```

Six million rows in, 315,203 out. Two columns of sixteen read, four predicates pushed into the
reader, and **nothing materialised** — the projection stage costs 0.0 ms because the aggregate reads
the source in place.

**The comparison** comes last: the engine, DuckDB and Polars on the same file, resident and cold.
The table is below.

**And there is a browser version**, for a reader who has a link rather than a terminal:

```bash
just web        # builds the wasm32 module and serves the page
```

The page runs the **engine's own kernel** — the same Rust crate, compiled to WebAssembly — over
the same column, and it is a **third SIMD path**: WebAssembly's `simd128`, beside NEON and AVX2.
It checks itself before it prints anything: the `simd128` mask is compared with the oracle's, bit
for bit, and **no throughput is printed if they differ**. The match count is the native
benchmark's — **12,726,115** — because the browser rebuilds the benchmark's column from a
histogram of the file's eleven distinct discount values.

The browser's number is a browser's number: a different runtime, a different clock, a different
compiler. It is published in [`PERF.md`](PERF.md) §1a with the browser, the engine, the machine
and the host's load, and it is never compared with the native one.

---

## The headline

> **The comparison kernel on the Q6 predicate set: 8.4 G rows/s, 2.48× faster than a genuinely
> scalar reference and 1.47× faster than what clang's auto-vectoriser produces from the same
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
query          engine   engine-lazy   duckdb   polars   datafusion
------------------------------------------------------------
scan         575.5           0.1      2.0      1.0      1.3
filter-numeric     590.4           5.0     22.0     45.0     15.6
filter-string      566.3           9.6     22.0    166.8     25.0
filter-range       575.1          16.5     32.0     16.4     22.6
q6           564.3          15.0     32.0    196.5     36.7
q6-revenue   578.2          62.0     69.0    200.7    117.0
```

**The lazy path is 37.6× faster than the eager one on `q6`**, and it beats Polars on every cold
query except `filter-range`. It beats DuckDB on `filter-numeric` — 5.0 ms against 22.0 ms, 4.4× — and
on `filter-string`, and on `scan`, and on `q6-revenue`. On `q6` it is **2.1× ahead**, where it used
to be 2.6× behind. The gap was never pruning: DuckDB prunes nothing on this file either (its own
plan says `Row Groups Scanned: 6 / 6`, and the file has no page index). It was the engine's own
mask, on its way to the reader as a **list of 2.78 million row ranges**, applied one range at a
time. Measured and attributed in [`PERF.md`](PERF.md), not glossed.

**`q6-revenue` is TPC-H Q6 itself** — `sum(l_extendedprice * (1 - l_discount))` — and the engine
runs it: 62.0 ms cold against DuckDB's 69.0 ms, and 17.3 ms resident against DuckDB's 49.0 ms.
The arithmetic is **0.2 ms of the 62.0 ms**, because it is evaluated over the 110,915 rows that
survived the filter and not over the 6,000,000 rows of the file. The query's cost is reading
`l_extendedprice`, not computing with it. See
[ADR-0012](docs/adr/0012-arithmetic-expression.md).

**And there is now a Rust peer to compare against.** `DataFusion` 49.0.2 reads the same Parquet
through the same Arrow stack, answers the same SQL, and is pinned to the same thread count:
**117.0 ms** against the engine's **62.0 ms** on the real Q6, cold. The engine is **1.9×** faster
than a Rust engine on its own stack, and the peer is **an instrument, not a dependency** — a
separate cargo project under `tools/`, with its own committed lock file, so the engine's published
dependency graph does not move. See
[ADR-0021](docs/adr/0021-the-materialised-column-and-a-rust-peer.md).

**And the query the engine used to lose, it now wins.** `filter-range` — `sum(l_quantity)` over a
two-predicate range — was the one loss in the table: 14.6 ms resident against Polars' 11.5 ms in
the set that found it, because the projection materialised a column that the aggregate only
streamed over. The aggregate now folds the source **where it lies**, over the filter's selection,
and the projection materialises nothing at all: **7.3 ms resident against Polars' 12.4 ms and
DuckDB's 20.0 ms**, and the projection stage is 0.0 ms. See
[ADR-0016](docs/adr/0016-the-aggregate-reads-in-place.md).

**And the scan itself got faster, because it was copying rows nobody asked for.** The lazy scan
used to filter the **whole** batch — including the column the predicates read and the query does
not materialise — and then copy the survivors a **second** time out of the filtered batch. It now
writes the survivors straight out of the reader's array into the engine's own column: one copy, of
the values that are kept. The lazy path's batch size was swept for the lazy shape as well, and it
was **inherited** rather than measured; spec 016 then found that the right answer was not a number
at all but the file's own row-group divisor, and the batch size is derived from the file now. The
whole lazy path moved: `q6` **15.0 ms**, `q6-revenue` **62.0 ms**, `filter-string` **9.6 ms**,
`filter-range` **16.5 ms**. See
[ADR-0017](docs/adr/0017-the-scan-filters-only-what-it-keeps.md) and
[ADR-0023](docs/adr/0023-the-per-row-group-cost.md).

**And the engine now has threads too.** The read splits the file's row groups across the workers
and merges them back in the file's order, so the answer does not depend on the scheduler. On the
identical query, with the incumbents at the identical thread count:

```
$ just sweep
threads   workers     engine   engine-scan     duckdb     polars
---------------------------------------------------------------
      1        1      15.4          15.4       33.0      193.6
      2        2       8.2           8.2       20.0      104.5
      4        4       6.1           6.1       14.0       67.8
      6        6       4.0           4.0       10.0       41.0
```

**The engine gets 3.89× from six workers, against DuckDB's 3.30× from six, and it is ahead at every
thread count.** The engine's curve is steeper because its query is almost entirely scan, and the
scan is the part that parallelises. The parallelism is capped by the file's **six row groups**, so
the curve is flat past six — the ceiling is the file's, not the machine's. See
[ADR-0011](docs/adr/0011-parallel-decode.md).

**And that ceiling is now a knob, and it has been measured.** The row-group size is a generator
flag, and files with **twelve** and **twenty-four** row groups were swept beside the six-row-group
one, in one session, with every incumbent at every thread count. The worker count is
`min(threads, row_groups)` at every point — **6**, **12** and **14** workers at fourteen threads —
and the engine's speedup no longer rises with it — it is **3.74×**, **3.63×** and **3.39×**
— because the cost that used to rise with the row-group count is gone.

**And the cost the more parallel file used to carry is gone.** Spec 015 found that the engine's
one-thread figure rose from 19.7 ms to 34.6 ms across the three files while DuckDB's, Polars' and
DataFusion's **did not move at all** — they read the same bytes. The engine was paying about
**0.83 ms a row group** that the other readers did not pay, so the more parallel file was the
slower one, and the mechanism was not measured yet. **Spec 016 measured it and removed it**: the
reader's batch size was a constant that divided no row group's row count, so every row group ended
on a **partial** batch, and a partial batch costs 1.32 ms. The batch size is derived from the file
now, and the engine's one-thread figure is **15.4, 15.7 and 16.4 ms** across the three files —
flat. See [ADR-0022](docs/adr/0022-the-parallelism-above-the-row-group-count.md) and
[ADR-0023](docs/adr/0023-the-per-row-group-cost.md).

**The operators are parallel too now, and that is where the engine is furthest ahead.** The filter
and the projection split their work across the workers; the answer does not depend on the thread
count, and that is checked row by row rather than asserted. Measured resident, with the incumbents
at the identical thread count:

```
$ just sweep mode=resident query=q6
threads   workers     engine   engine-ops     duckdb     polars    datafusion
---------------------------------------------------------------
      1        1       7.8           7.8       46.0       69.0       24.5
      2        2       4.2           4.2       27.0       37.9       16.9
      4        4       2.6           2.6       15.0       19.7       10.6
      6        6       2.0           2.0       11.0       13.2        7.8
      8        8       2.0           2.0        9.0       10.9        7.1
     14       14       1.8           1.8        8.0        9.5        6.4
```

**The engine is 5.9× faster than DuckDB at one thread and 4.4× faster at fourteen.** Its
operators' worker count is not capped by the file's six row groups, so it keeps going where the
read stops. The **aggregate** is not parallelised and never will be: `SUM` is not associative, and
an answer that depends on the block boundaries is not an answer.

**The engine's own curve is flatter than DuckDB's here, and the comparison still holds.** From
fourteen workers the engine gets **4.27×** and DuckDB gets **5.75×** — a resident query is
operators, not scan, and DuckDB's operators scale better. The engine is still ahead in absolute
terms at every thread count, by **5.9×** at one thread and **4.4×** at fourteen, and the answer
does not depend on the count. This increment changed the **read** path, and a resident query never
uses it. See
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
the engine was behind both incumbents. It is now **576.8 ms** in this set, and at one thread the
engine is **2.2× ahead** of DuckDB on the same file:

```
$ just sweep mode=load
threads   workers     engine    duckdb    polars    datafusion
---------------------------------------------------------------
      1        1      576.8   1275.0   1069.8      362.3
      2        2      423.9    635.0    536.1      242.0
      4        4      312.2    423.0    372.5      128.5
      6        6      251.0    239.0    193.5      127.7
      8        6      246.0    245.0    200.2       76.0
     14        6      243.9    235.0    204.8       83.5
```

**At six and fourteen threads DuckDB is 4–5% ahead**, because the engine's read is capped by the
file's six row groups and DuckDB's is not — a reversal of the set before this one, where the engine
was 1.2× ahead at six. One thread is the like-for-like comparison, and there the engine is 2.2×
ahead.

**Most of that was not the parallelism — it was a constant the eager path had inherited rather
than measured.** The Parquet reader's default batch size is 1,024 rows, and the per-batch cost
scales with the number of columns it reads; the eager path reads all sixteen, so it rebuilt
sixteen dictionary tables 5,860 times instead of 733. Fixing that one constant took the
one-thread load from 1145.6 ms to 587.0 ms, and the parallel decode took it the rest of the way.
The merge is **one pass, and it moves** the parts' values instead of copying them, so the peak
memory stays at about one copy of the data. The merged dataset is **bit-identical** to the serial
one — checked row by row, including the dictionary's order and the indices of the rows that are
NULL. See [ADR-0014](docs/adr/0014-parallel-eager-decode.md).

**And the lazy path got faster four times.** Its scan of Q6's two columns was **78.4 ms** and is
now **15.0 ms**, against DuckDB's 32.0 ms on the same file at the same thread count: the engine is
**2.1× ahead** on the headline query, where it was 2.6× behind. The first cause was not the decode
and not the comparison — it was the pushdown's mask, handed to the reader as a **row filter**,
which parquet-rs turns into a list of **2,775,138 row ranges** for 2,178,861 surviving rows and
applies one range at a time. Reading a column through that list costs 50.0 ms; without it, 7.8 ms.
The engine keeps the mask now — one bit per row — and applies it itself. See
[ADR-0015](docs/adr/0015-the-engine-keeps-the-mask.md). The second and third were the scan's own
shape: it was copying the rows it discards and the column nobody asked for, and its batch size was
a constant measured for a scan shape that has since changed three times. See
[ADR-0017](docs/adr/0017-the-scan-filters-only-what-it-keeps.md). The fourth was the batch size
itself: the constant became a **divisor of the file's row-group row count**, which is **23% faster**
than the old constant on the real Q6's predicate set, measured back to back with the two
alternating round by round. See
[ADR-0023](docs/adr/0023-the-per-row-group-cost.md).

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
  columnar-web       the browser build: the kernel as a wasm32 module, and the page
                     that runs it and checks it against the oracle
docs/adr/            the decision record
specs/001-columnar-core/  the spec, the plan, the tasks
specs/002-lazy-columnar-scan/  the lazy scan: the plan, the pushdown, the equivalence test
specs/003-kernel-pushdown/  the pushdown runs the engine's own kernel
specs/004-performance-gates/  the regression gate and the budget gate
specs/005-parallel-decode/  the parallel read, and the order it preserves
specs/006-arithmetic-aggregate/  the arithmetic aggregate, and the real Q6
specs/007-parallel-operators/  the parallel filter and projection, and the aggregate that is not
specs/008-parallel-eager-decode/  the parallel eager decode, and the batch size it inherited
specs/009-the-engine-keeps-the-mask/  the lazy scan's decomposition, and the mask the engine keeps
specs/010-fuse-the-projection-into-the-aggregate/  the global aggregate reading the source in place
specs/011-the-scan-filters-only-what-it-keeps/  the scan compacting the survivors once
specs/012-the-scan-does-not-expand-the-predicates-column/  the scan's own decode, measured and refused
specs/013-the-kernel-in-a-browser/  the third SIMD path, and the page that checks it
specs/014-the-materialised-column-and-a-rust-peer/  the materialised column taken apart, and a Rust peer
specs/015-the-row-group-count-is-a-knob/  the row-group count as a knob, and the parallelism cap
specs/016-the-per-row-group-cost/  the per-row-group cost: the mechanism measured, and removed
```

**One predicate, six implementations, one answer.** The comparison kernel is the project. There
is a plain Rust oracle, a C++ scalar reference compiled two ways, a C++ NEON/AVX2 path, a Rust
intrinsics path on the same two ISAs, and a Rust `simd128` path for the browser — all agreeing bit
for bit, and the differential test suite holds every one of them to the oracle's mask, bit for
bit, on random inputs and on the cases that are easy to get wrong: a NULL row never matches
(including under `!=`), a NaN never matches (including under `!=`), the tail block, the empty
input, the bits past the end.

---

## Running it

**Prerequisites**: a Rust toolchain (`rustup`), `just` (`cargo install just`), and Python 3 for the
two scripts under `scripts/`. `just` is the single entry point — every recipe in the `justfile` is
the documented way to do a thing.

```bash
# 1. The whole story, in one command, in about a minute.
just demo

# 2. Build and check. This is the whole gate.
just check

# 2a. Compile the x86-64 half of the kernel from this ARM64 machine.
#     The AVX2 path only builds on x86-64, so without this it rots
#     silently. It rotted silently once already.
just check-x86

# 3. Generate the dataset. Deterministic: the same (scale factor, seed) gives
#    byte-identical output, and the manifest records what it was.
just dataset                      # 6,000,000 rows, 231.6 MiB, about 2.8 seconds

# 4. Run a query and read the plan output.
just query                        # the TPC-H Q6 predicate set
just query-lazy                   # the same, reading only what the plan needs

# 5. Measure the kernel, four ways.
just bench

# 6. Measure the engine against DuckDB, Polars and DataFusion.
just peer-build               # the Rust peer: a separate project, built once
just compare

# 7. Verify the AVX2 path's correctness, under Rosetta 2.
#    Correctness only -- the benchmark refuses to measure with the
#    override in force, because emulated timings mean nothing.
just verify-avx2

# 8. Build the browser module and serve the page.
just web                     # then open the URL it prints
```

`just check-wasm` is the build gate for the browser's half of the kernel: it compiles
`columnar-kernels` for `wasm32-unknown-unknown` with the `simd128` feature, so the browser's
path cannot rot silently — the way the AVX2 path rotted once already.

`just compare` needs `DuckDB` on `PATH` or in `tools/bin/`, the `polars` Python package installed,
and the Rust peer built (`just peer-build`, which puts it in `tools/bin/` next to DuckDB). It says
so and skips the missing comparator rather than failing quietly — and it cross-checks every answer
against the engine's own, because a comparison where the two sides computed different values is not
a benchmark.

---

## What is measured, and what is not

| | status |
|---|---|
| The comparison kernel, on AArch64 / NEON | **measured**, six campaigns of eight iterations, spread published |
| The comparison kernel, on x86-64 / AVX2 | **measured** — the kernel A/B on GitHub-hosted x86-64 runners, **ratios only**, on a named CPU, with the machine published and never compared with the ARM64 numbers. Its **correctness** is verified twice over: under Rosetta 2 on this ARM64 Mac, and on **native x86-64 hardware**: [`PERF.md` §1b](PERF.md#1b-the-kernel-on-x86-64-measured-natively) |
| The engine's filter, project and aggregate | **measured**, against DuckDB 2.0.0 and Polars 1.36.1 |
| The engine's **arithmetic aggregate** | **measured** — the real TPC-H Q6, `sum(l_extendedprice * (1 - l_discount))`, against the same SQL on the same file |
| The engine's Parquet reader | **measured**, in two modes: eager (all sixteen columns) and lazy (two of sixteen, with the predicates pushed in) |
| The **read** under several thread counts | **measured** — the engine and both incumbents, at the same thread counts, on the same file: [`PERF.md` §2e](PERF.md#2e-the-thread-count-sweep) |
| The **query** under several thread counts | **measured** — the same sweep with the data already in memory, which is where the operators are the measurement: [`PERF.md` §2f](PERF.md#2f-the-resident-sweep-where-the-operators-are-the-measurement) |
| Multithreading of the **operators** | **measured** — the filter and the projection are parallel, and the answer does not depend on the thread count. The **aggregate** is not parallelised and never will be: `SUM` is not associative |
| The **eager** path's decode | **measured** — the eager path splits the file's row groups across the workers, and the merged dataset is bit-identical to the serial one's. It gets the same derived batch rule the lazy path got: [`PERF.md` §2g](PERF.md#2g-the-load-sweep--where-the-engines-biggest-number-was) |
| The **lazy** path's scan, decomposed | **measured** — Arrow's own decode, the engine's kernel, and the mask are separated, and the decomposition is in the tree as `crates/columnar-engine/examples/lazy_scan_decomposition.rs`: [ADR-0015](docs/adr/0015-the-engine-keeps-the-mask.md) |
| The comparison kernel, **in a browser** | **measured**, on a named browser and a named machine, with the host's load — and never compared with the native number: [`PERF.md` §1a](PERF.md#1a-the-kernel-in-a-browser) |
| The engine against a **Rust peer** | **measured** — `DataFusion` 49.0.2 reading the same Parquet through the same Arrow stack, in the same harness, at the same thread count. It is a **separate cargo project** under `tools/`, so the engine's dependency graph does not move: [`PERF.md` §2](PERF.md#2-the-engine-against-duckdb-polars-and-datafusion) |
| The **materialised column**, taken apart | **measured** — the column the query materialises is **61%** of the real Q6, and its cost is **bytes**: Arrow decodes it in **34.26 ms** and the parquet crate in **36.76 ms**. The counterfactual — the same column written *with* a dictionary — is 30% bigger and 12% slower: [`PERF.md` §3h](PERF.md#3h-the-materialised-column-and-what-it-is-made-of) |
| The query, under several **dataset sizes** | **measured** — the real Q6 at 6, 12 and 24 million rows, with every incumbent at every size. The cost per million rows is **flat**: 9.6, 9.5, 9.7: [`PERF.md` §3i](PERF.md#3i-the-curve-over-the-size-of-the-data) |
| The **row-group count**, as a knob | **measured** — the row-group size is a generator flag now, and the engine's parallelism cap was measured on files with six, twelve and twenty-four row groups. The cap is `min(threads, row_groups)`, exactly. The engine used to pay about **0.83 ms a row group** that DuckDB, Polars and DataFusion did not pay, so the file with the most parallelism was the slowest in absolute terms; **spec 016 measured the mechanism and removed it** — the reader's batch size is derived from the file's row groups now, and the engine's own scan costs **0.02 ms a row group**, flat across the three files: [`PERF.md` §3j](PERF.md#3j-the-row-group-count-and-what-it-costs) |

The x86-64 row is the important one, and it has three halves.

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
timing taken there would measure the translator rather than any hardware.

**And now there is a third half: the measurement itself.** The repository is public, so
GitHub-hosted runners are free, and `bench-avx2.yml` runs the kernel A/B on a **native x86-64**
machine. The AVX2 path is now executed by real hardware for the first time in the project's life,
checked against the oracle on that hardware, and its ratio published — as a **ratio**, with the
CPU named, because GitHub's runners are a fleet and the ratio turned out to be a property of the
**CPU** as much as of the kernel. The absolute figures are not published: they describe a shared
runner, and this repository does not publish a number it cannot reproduce.

Finding this also exposed a defect worth naming: **the AVX2 Rust path had never been compiled by
anyone.** It had two type errors in it. It is now gated by `just check-x86` and by CI, so that
class of rot cannot recur. See
[ADR-0009](docs/adr/0009-avx2-without-x86-hardware.md) and
[ADR-0024](docs/adr/0024-the-repository-goes-public.md).

---

## Governance

This repository has a constitution, and it is not decoration.

- [`constitution.md`](constitution.md) — eight numbered principles, the quality gates, and the
  governance rules.
- [`docs/adr/`](docs/adr/) — every decision that cost something, including the ones that cost a
  published number: an ADR here can retire a figure this file publishes.
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
| Specs | `specs/001-columnar-core/` — R1..R5, with the out-of-scope table · `specs/002-lazy-columnar-scan/` — R1..R8, the lazy scan and the pushdown · `specs/003-kernel-pushdown/` — R1..R8, the pushdown running the engine's kernel · `specs/004-performance-gates/` — R1..R8, the regression and budget gates · `specs/005-parallel-decode/` — R1..R10, the parallel read and the thread-count sweep · `specs/006-arithmetic-aggregate/` — R1..R10, the arithmetic aggregate and the real Q6 · `specs/007-parallel-operators/` — R1..R6, the parallel filter and projection · `specs/008-parallel-eager-decode/` — R1..R6, the parallel eager decode and the batch size it inherited · `specs/009-the-engine-keeps-the-mask/` — R1..R8, the lazy scan's decomposition and the mask the engine keeps · `specs/010-fuse-the-projection-into-the-aggregate/` — R1..R9, the global aggregate reading the source in place · `specs/011-the-scan-filters-only-what-it-keeps/` — R1..R6, the scan compacting the survivors once, and the batch size it inherited · `specs/012-the-scan-does-not-expand-the-predicates-column/` — the scan's own decode and the expansion of its RLE indices, both measured and refused · `specs/013-the-kernel-in-a-browser/` — R1..R6, the third SIMD path, the page that checks it, and the number it reproduces · `specs/014-the-materialised-column-and-a-rust-peer/` — R1..R4, the materialised column taken apart, the curve over the size of the data, and a Rust peer beside the incumbents · `specs/015-the-row-group-count-is-a-knob/` — R1..R4, the row-group count as a knob, the parallelism cap measured on three files, and the per-row-group cost the incumbents do not pay · `specs/016-the-per-row-group-cost/` — R1..R4, that cost's mechanism measured, the reader's batch size derived from the file, and the defect removed · `specs/017-the-repository-goes-public/` — R1..R5, the licence, the contribution guide, three files out of the history, and the x86-64 measurement the public repository made possible |
| Increment | storage → filter → project → evaluate → aggregate (the TPC-H Q6 path), with two ways to read a file: eager and lazy |
| Licence | Apache-2.0. The C++ baseline in `crates/columnar-kernels/src/` is this repository's own work and carries the same licence, and there is no vendored third-party source in the tree. [`CONTRIBUTING.md`](CONTRIBUTING.md) is the method, written for a stranger. |
| Tests | 269, all green, including the differential kernel suite, the lazy-scan differential suite, the expression suite, the parallel-operator suite, the parallel-eager suite, the fused-aggregate suite, the performance-gate suite, the browser's column suite, the RLE decoder's suite and the x86-64 cross-check |
| Lint | `clippy --workspace --all-targets` with `all` + `pedantic` as errors; zero warnings |
| CI | fmt, clippy (ubuntu + macos), test, the differential job, an x86-64 cross-compile check, `cargo deny`, a no-committed-dataset check, the published-figures check, and the two benchmark runners. **The repository is public**, so the runners are free — but the account's own Actions spending limit still applies to every job, and when it is exhausted *every* job fails at once with a billing message rather than a test failure. `just check` covers all of it locally and does not. |
| Gate | `just perf-gate` — a Mann-Whitney test against a stored baseline, with a measured practical threshold. It can also say *inconclusive*, which is not a pass. |
| Next | **The next question is the last of the per-row-group cost.** On the two-predicate shape it is gone — the engine's own scan costs **0.02 ms a row group** and is flat across the three files. On the **three**-predicate shape there is about **0.5 ms a row group** left, and the two shapes differ by exactly one thing: the third predicate's column, `l_quantity`, which is dictionary-encoded with 4,901 values a row group. Whether that is the crate building that dictionary per row group, or the engine comparing its distinct values per row group, is **not separated by the measurement**, and it is written down as the next question rather than guessed at. Before that: the cold path's one remaining gap is `filter-range`, and it is now **0.1 ms** — the lazy path is 16.5 ms against Polars' 16.4 ms, and the two spreads overlap. The scan that holds it is **Parquet decode**, and the decode was measured **three** times and **every time it was refused**: owning the decode buys nothing (the two shapes came out equal to a tenth of a millisecond), and neither does **not expanding the predicate's column**. The file's columns *are* dictionary-encoded, and the expansion of six million RLE indices into six million values was measured at **1.79 ms** — 46% of the decode of the predicate's column, and **4.1% of the query**. The mask the hypothesis would build instead costs **1.73 ms**, which is **0.42 ms** more than the vectorised predicate costs today once the two are netted off, and the parquet crate does not expose the indices at all, so capturing the expansion means owning the Parquet reading: **the net is 1.37 ms.** And the **materialised** column — the query's real cost, **61%** of it — was taken apart too: it is **bytes**, Arrow decodes it in 34.26 ms and the parquet crate in 36.76 ms, so there is nothing there to win either. After that: the morsel driver (still worth ~0 ms in the cold path, because the pushdown leaves the operators nothing to do), and the **x86-64 absolute figures**, which need a **pinned** x86-64 runner — the `avx2-self-hosted` job is already wired for one, and it is the only job whose absolute figures would be worth publishing |
