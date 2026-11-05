# Columnar Engine

A vectorised columnar query engine in Rust, with a measurement harness that is willing to say the
engine loses.

This is **Project 02** of the portfolio: a columnar engine that stores columns in its own
dictionary-aware format, filters them with a hand-written SIMD kernel, and compares itself against
DuckDB and Polars under conditions it cannot choose for itself.

The first increment is storage → filter → project → aggregate: the TPC-H Q6 path. **The engine now
joins two tables and sorts.** The pipeline is `scan → filter → project → [join] → [sort] →
aggregate`: the join is an inner equi-join executed as a **hash join**, with the smaller table as
the build side and the larger one staying on the lazy path, and the sort keeps a **bounded heap**
when the query has a limit. Both are measured against the same incumbents, on the same two files,
with the same SQL handed to every system.

**Where the project stands** is in [`STATUS.md`](STATUS.md): what is built, what is measured, what
the increments found, and what is still open. It is a summary and not a second copy of the figures —
every number in it is the headline of a table this file and [`PERF.md`](PERF.md) publish in full, and
the same gate checks it against the committed reports.

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
figures are the medians of **six** campaigns, and they say `cpp-simd` is **2.44×** the scalar
reference and **1.48×** what clang's auto-vectoriser produces from the same loop — where the set
before the bump said **2.48×** and **1.47×**, and that movement is the host's, because the kernel
reads a synthetic column and never a file, so the dependency cannot be in it — this run said
2.37× and 1.65×. Both are true; the ratio is a timing and the machine is not isolated, the count of
matching rows is not a timing and does not move. The conditions, the medians and the spread are in
[`PERF.md`](PERF.md) §1.

**The engine's own plan.**

```
optimization order: predicate pushdown -> projection pushdown -> late materialization -> join -> sort -> aggregate
stage      rows in  rows out  elapsed
scan       6000000  315203    14.7 ms
project    315203   315203    0.0 ms
aggregate  315203   1         0.0 ms
materialised columns: (none)
read threads: 1
operator workers: 1 filter, 1 projection
columns read: l_discount, l_shipdate
pushed down: l_discount >= 0.04 and l_discount <= 0.07, l_shipdate >= '1994-01-01' and l_shipdate < '1995-01-01'
```

Six million rows in, 315,203 out. Two columns of sixteen read, four predicates pushed into the
reader, and **nothing materialised** — the projection stage costs 0.0 ms because the aggregate reads
the source in place.

**The comparison** comes last: the engine, DuckDB and Polars on the same file, resident and cold.
The table is below.

**And there is a browser version**, for a reader who has a link rather than a terminal:

**<https://oib98vz3b8zwj.space.minimax.io>** — open it and the kernel runs: the page rebuilds the
column, runs both implementations over it, compares their masks bit for bit, and only then
prints a number. There is nothing to click; the button under the title is there to run it a
second time, and it always answers — press it while a run is in flight and the run happens again
when the first one ends. It is the same build `just web-build` produces, and it names the commit
it was built from. To build and serve it yourself:

```bash
just web        # builds the wasm32 module and serves the page
```

The page runs the **engine's own kernel** — the same Rust crate, compiled to WebAssembly — over
the same column, and it is a **third SIMD path**: WebAssembly's `simd128`, beside NEON and AVX2.
It checks itself before it prints anything: the `simd128` mask is compared with the oracle's, bit
for bit, and **no throughput is printed if they differ**. The match count is the native
benchmark's — **12,726,115** — because the browser rebuilds the benchmark's column from a
histogram of the file's eleven distinct discount values.

The hosted page has no server behind it, so it says so rather than pretending otherwise: *"there
is no report server at this address — the numbers above are this browser's"*. The published
evidence is measured on the development machine, where the host's load average is a condition of
the measurement; the hosted page is the **proof**, not a new source of published numbers.

The browser's number is a browser's number: a different runtime, a different clock, a different
compiler. It is published in [`PERF.md`](PERF.md) §1a with the browser, the engine, the machine
and the host's load, and it is never compared with the native one.

---

## The headline

> **The comparison kernel on the Q6 predicate set: 7.8 G rows/s, 2.44× faster than a genuinely
> scalar reference and 1.48× faster than what clang's auto-vectoriser produces from the same
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
scan           574.4           0.1      2.0      1.1      1.3
filter-numeric 668.4           4.2     24.0     47.7     17.0
filter-string  640.4          10.0     24.0    166.9     25.9
filter-range   585.4          15.8     33.0     16.5     21.8
q6             587.2          14.9     35.0    201.8     38.5
q6-revenue     596.1          59.1     70.0    201.7    118.2
join-orders    591.3         100.0     94.0    225.7    175.3
sort-topk      561.1          39.0     39.0   1468.9     41.3
```

**The lazy path is 39.4× faster than the eager one on `q6`**, and it beats Polars on every cold
query — `filter-range` included, where it was the one loss before the bump. It beats DuckDB on
`filter-numeric` — 4.2 ms against 24.0 ms, 5.7× — and on `filter-string`, and on `scan`, and on
`q6-revenue`, and on `filter-range`, and on `q6`. On `q6` it is **2.3× ahead**, where the set this
file published before had **2.1×**, and it used to be 2.6× behind. The gap was never pruning:
DuckDB prunes nothing on this file either (its own plan says `Row Groups Scanned: 6 / 6`, and the
file has no page index). It was the engine's own mask, on its way to the reader as a **list of 2.78
million row ranges**, applied one range at a time. Measured and attributed in
[`PERF.md`](PERF.md), not glossed.

**And the two queries that were the engine's first work on two tables are published as they are.**
The engine's cold `join-orders` — `lineitem` joined to `orders` on the order key, with a date range
on the build side, aggregated over the whole join — is **100.0 ms** (98.7–106.2 ms over the three
campaigns), against DuckDB's **94.0 ms** (92.0–100.0 ms), Polars' **225.7 ms** (220.8–238.7 ms) and
the `DataFusion` peer's **175.3 ms** (171.4–184.9 ms): the engine is **6.0 ms behind** DuckDB,
**1.8× ahead of the peer**, and **2.3× ahead of Polars**. The set this file published before had the
engine at **97.6 ms** against DuckDB's **91.0 ms** — **6.6 ms behind** — and DuckDB's own cold join
moved **91.0 ms** to **94.0 ms** in the same sessions while the engine's moved **97.6 ms** to
**100.0 ms**: **the gap is 0.6 ms smaller than it was, and the whole of that is DuckDB's own
movement.** The engine's cold `sort-topk` — the ten largest `l_extendedprice` values — is **39.0 ms**
(38.8–56.6 ms) against DuckDB's **39.0 ms** (38.0–42.0 ms), the peer's **41.3 ms** (40.0–42.3 ms)
and Polars' **1468.9 ms** (1394.5–1529.4 ms): the engine is **level with DuckDB** — where the set
this file published before had it **1.04× behind** — **2.3 ms behind the peer**, and **38× ahead of
Polars**. One thread, the same two files, the same SQL handed to every system, and the answers
cross-checked.

**The join's number moved because the code moved, and the code's share is measured, not asserted.**
The join's probe was taken apart by an instrument committed in the tree —
`crates/columnar-engine/examples/scan_cost_probe.rs` — and every candidate it measures is checked
against the engine's own output, row for row, before it is timed. It found that the probe was
copying **96 MB** of rows to look up **216,345** keys in them, and that **96.4%** of what it copied
was thrown away: the probe now runs **inside the scan**, over the reader's batches, and only the
rows it matches are ever copied.

**And the newest increment took the sort apart.** The engine's cold `sort-topk` was **40.1 ms**
against DuckDB's **37.0 ms** — a gap of **3.1 ms**, the largest one left — and the sort had never been
measured against its own parts. The instrument committed in the tree
(`crates/columnar-engine/examples/sort_cost_probe.rs`) runs the engine's own plan through the engine's
own entry point, takes the sort's stage from the plan output and measures the parts on the same
column and the same rows, checking the engine's ten rows against its own before it times anything.
The answer was that the sort's own work was a **comparison made a row at a time**: the floor it was
measured against — **3.6 ms**, a plain read — was a *scalar* read, and the same column compared in
blocks costs **0.96 ms**. The sort now compares its keys in blocks against the heap's own worst key,
and its stage fell from **4.18 ms** to **1.10 ms**: the engine's cold `sort-topk` was **37.2 ms**
against DuckDB's **39.0 ms** — **ahead of it, for the first time**.

**And the increment after that took the join's stage apart.** The join's stage is the engine's own work
on the query where the engine is level — **24.2 ms** of the cold `join-orders`, a quarter of it — and it
had never been measured against its own parts. The instrument committed in the tree
(`crates/columnar-engine/examples/join_cost_probe.rs`) runs the engine's own plan through the engine's own
entry point, takes the join's stage from the plan output on **both** paths, and measures the parts on the
same data, the same rows and the same table, checking the engine's **216,783** rows against its own, in
order, before it times anything. The answer was that the stage is **the build side's read** — **11.83
ms**, more than half of it, and Arrow's decode, which has been measured three times and refused every
time — and the **probe**, **10.05 ms**, and that the probe pays **3.10 ms** more than the same lookup
written out by hand, because the engine's per-row lookup consults the **overflow map**, which is **empty**
for this data. The two paths also disagreed: the stage reported `216783` rows in on one and `6000000` on
the other for the same probe, so the stage now reports the rows the probe **consumed**, and the two paths
say the same thing.

**To separate both from the machine, the old code was re-measured on the same host, back to back,
in its own three campaigns.** The join's cold figure is **89.8 ms** in the set this one replaces,
**95.9 ms** for the old code on this host, and **96.7 ms** for the new code; the sort's is
**40.1 ms**, **42.6 ms** and **37.2 ms** — while the six queries the increment did not touch moved by
**0% to +6.3%** in the cold lazy path, every one of them against the sort. The engine's own plan
output reads it a stage at a time: the sort's stage went from **5.2 ms** to **2.2 ms** on the same
host, and the account closes to **0.01 ms** on every query and on both paths — where it closed to
**0.02 ms** when the selection that holds every row became the range, and where the old code left
**4.7 ms** and **4.3 ms** unnamed. The decomposition, the control and the
negative result are in [`PERF.md` §2h](PERF.md#2h-the-hot-paths-taken-apart),
[`PERF.md` §3l](PERF.md#3l-the-scan-taken-apart-and-the-scan-that-feeds-the-join),
[`PERF.md` §3m](PERF.md#3m-the-time-the-plan-output-does-not-name) and
[`PERF.md` §3n](PERF.md#3n-the-sort-taken-apart) and
[`PERF.md` §3o](PERF.md#3o-the-joins-stage-taken-apart).

**The join's resident figure is published now, and the asymmetry is named.** In resident mode the
incumbents hold both tables in memory before the clock starts and the engine's resident path reads
the build side **inside** it — so the engine does more work inside its own clock than they do
inside theirs. Spec 020 withheld the figure for that reason; it is published now because the
asymmetry works **against** the engine and the engine wins anyway: **26.0 ms** against DuckDB's
**40.0 ms**, the peer's **91.1 ms** and Polars' **90.6 ms**. Those moved with the machine and not
with the dependency — the resident path never decodes a file, so the bump cannot be in them — and
the set this file published before had the engine at **26.6 ms** against DuckDB's **35.0 ms**, the
peer's **85.8 ms** and Polars' **78.9 ms**. The reason, and the raw number, are in
[`PERF.md` §2a](PERF.md#2a-resident--the-data-is-already-in-memory).

**The set before this one moved when the campaign was re-run, and the reason was the machine.** The
engine's cold `q6` went from **15.7 ms** to **15.4 ms**; the real Q6 from **62.4 ms** to **58.9 ms**;
`filter-numeric` from **5.0 ms** to **4.5 ms**; `filter-string` from **9.2 ms** to **9.4 ms**;
`filter-range` from **15.6 ms** to **16.3 ms**; the engine's resident `q6` from **9.5 ms** to
**9.4 ms**; the engine's resident `filter-range` from **6.5 ms** to **6.6 ms**; DuckDB's cold
`q6` from **33.0 ms** to **32.0 ms**; DuckDB's cold `q6-revenue` from **68.0 ms** to
**67.0 ms**; DuckDB's resident `filter-range` from **19.0 ms** to **19.0 ms**; Polars' resident
`filter-range` from **11.3 ms** to **10.8 ms**; and the `DataFusion` peer's cold `q6-revenue` from
**115.2 ms** to **112.6 ms**. The six queries that were already here did not change their code
path in this increment: what changed is the join's lookup, which no longer consults a map that
holds nothing, and the campaign was re-run on the same shared machine. The eager load was
**556.3 ms** and became **549.9 ms**, and DuckDB's load moved the other way, **1355.0 ms → 1756.0 ms** —
the drift is in both directions and in both systems. Every system was measured in the same three
campaigns, on the same file, at the same thread count, so the comparisons stay like-for-like even
where the absolute numbers moved.

**And the figures moved again when the dependency moved, and this time the reason is the
dependency.** `parquet` and `arrow` went from **54.3.1** to **58.4.1**, Arrow's decode changed with
them, and the dataset was regenerated, so every figure above was measured again — three campaigns
each, on the same host, at the same thread count. **On the cold path the reason is the bump**,
because the file is read through the new Arrow: the engine's cold `q6` was **15.4 ms** and is
**14.9 ms**, the eager `q6` was **552.3 ms** and is **587.2 ms**, the cold `q6-revenue` was
**58.9 ms** and is **59.1 ms**, and the cold `filter-range` was **16.3 ms** and is **15.8 ms**.
**On the resident path the bump cannot be in it at all** — the data is already in memory, so Arrow
never decodes a file there, and the movement is the host's and the engine's own variance: the
resident `q6` was **9.4 ms** and is **10.1 ms**, the resident `filter-range` was **6.6 ms** and is
**7.7 ms**, and the resident `join-orders` was **26.6 ms** and is **26.0 ms**. **The incumbents are
the yardstick**, because they do not read the file through this repository's Arrow: DuckDB's cold
`q6` was **32.0 ms** and is **35.0 ms**, Polars' cold `q6` was **190.5 ms** and is **201.8 ms**, and
DuckDB's resident `filter-range` was **19.0 ms** and is **20.0 ms**. Every system was measured in
the same three campaigns, on the same file, so the comparisons stay like-for-like even where the
absolute numbers moved.

**`q6-revenue` is TPC-H Q6 itself** — `sum(l_extendedprice * (1 - l_discount))` — and the engine
runs it: 59.1 ms cold against DuckDB's 70.0 ms, and 17.0 ms resident against DuckDB's 51.0 ms.
The arithmetic is **0.1 ms of the 59.1 ms**, because it is evaluated over the 110,915 rows that
survived the filter and not over the 6,000,000 rows of the file. The query's cost is reading
`l_extendedprice`, not computing with it. See
[ADR-0012](docs/adr/0012-arithmetic-expression.md).

**And there is now a Rust peer to compare against.** `DataFusion` 49.0.2 reads the same Parquet
through its own Arrow stack, answers the same SQL, and is pinned to the same thread count:
**118.2 ms** against the engine's **59.1 ms** on the real Q6, cold — where the set this file
published before had **112.6 ms** against **58.9 ms**. The engine is **2.00×** faster than a Rust
engine on its own stack: the engine's own cold number moved with the bump, and the peer's moved
with the host, because the peer does not read the file through this repository's Arrow. And the
peer is **an instrument, not a dependency** — a separate cargo project under `tools/`, with its own
committed lock file, so the engine's published dependency graph does not move. See
[ADR-0021](docs/adr/0021-the-materialised-column-and-a-rust-peer.md).

**And the query the engine used to lose, it now wins.** `filter-range` — `sum(l_quantity)` over a
two-predicate range — was the one loss in the table: 14.6 ms resident against Polars' 11.5 ms in
the set that found it, because the projection materialised a column that the aggregate only
streamed over. The aggregate now folds the source **where it lies**, over the filter's selection,
and the projection materialises nothing at all: **7.7 ms resident against Polars' 13.3 ms and
DuckDB's 20.0 ms** — where the set this file published before had **6.6 ms** against **10.8 ms** and
**19.0 ms**, and the resident path never decodes a file, so that movement is the host's and the
engine's own variance — and the projection stage is 0.0 ms. See
[ADR-0016](docs/adr/0016-the-aggregate-reads-in-place.md).

**And the scan itself got faster, because it was copying rows nobody asked for.** The lazy scan
used to filter the **whole** batch — including the column the predicates read and the query does
not materialise — and then copy the survivors a **second** time out of the filtered batch. It now
writes the survivors straight out of the reader's array into the engine's own column: one copy, of
the values that are kept. The lazy path's batch size was swept for the lazy shape as well, and it
was **inherited** rather than measured; spec 016 then found that the right answer was not a number
at all but the file's own row-group divisor, and the batch size is derived from the file now. The
whole lazy path moved: `q6` **14.9 ms**, `q6-revenue` **59.1 ms**, `filter-string` **10.0 ms**,
`filter-range` **15.8 ms** — the figures the current set carries. See
[ADR-0017](docs/adr/0017-the-scan-filters-only-what-it-keeps.md) and
[ADR-0023](docs/adr/0023-the-per-row-group-cost.md).

**And the engine now has threads too.** The read splits the file's row groups across the workers
and merges them back in the file's order, so the answer does not depend on the scheduler. On the
identical query, with the incumbents at the identical thread count:

```
$ just sweep
threads   workers  filter  proj      engine   engine-scan  engine-ops     duckdb     polars  datafusion   engine x    duckdb x
---------------------------------------------------------------
      1        1       1     1      14.6          14.6          0.0       33.0      197.2         35.4       1.00x      1.00x
      2        2       1     1       7.5           7.5          0.0       21.0       99.1         27.7       1.95x      1.57x
      4        4       1     1       5.4           5.4          0.0       14.0       66.6         16.6       2.71x      2.36x
      6        6       1     1       4.1           4.1          0.0       10.0       37.2         16.4       3.57x      3.30x
```

**The engine gets 3.57× from six workers, against DuckDB's 3.30× from six, and it is ahead at every
thread count.** The engine's curve is steeper because its query is almost entirely scan, and the
scan is the part that parallelises. The parallelism is capped by the file's **six row groups**, so
the curve is flat past six — the ceiling is the file's, not the machine's. See
[ADR-0011](docs/adr/0011-parallel-decode.md).

**And that ceiling is now a knob, and it has been measured.** The row-group size is a generator
flag, and files with **twelve** and **twenty-four** row groups were swept beside the six-row-group
one, in one session, with every incumbent at every thread count. The worker count is
`min(threads, row_groups)` at every point — **6**, **12** and **14** workers at fourteen threads —
and the engine's speedup no longer rises with it — it is **3.65×**, **2.80×** and **3.00×**
— because the cost that used to rise with the row-group count is gone.

**And the cost the more parallel file used to carry is gone.** Spec 015 found that the engine's
one-thread figure rose from 19.7 ms to 34.6 ms across the three files while DuckDB's, Polars' and
DataFusion's **did not move at all** — they read the same bytes. The engine was paying about
**0.83 ms a row group** that the other readers did not pay, so the more parallel file was the
slower one, and the mechanism was not measured yet. **Spec 016 measured it and removed it**: the
reader's batch size was a constant that divided no row group's row count, so every row group ended
on a **partial** batch, and a partial batch costs 1.32 ms. The batch size is derived from the file
now, and the engine's one-thread figure is **14.6, 14.4 and 15.9 ms** across the three files —
within 9% of each other, against the 19.7 ms to 34.6 ms spread spec 015 found. See
[ADR-0022](docs/adr/0022-the-parallelism-above-the-row-group-count.md) and
[ADR-0023](docs/adr/0023-the-per-row-group-cost.md).

**The operators are parallel too now, and that is where the engine is furthest ahead.** The filter
and the projection split their work across the workers; the answer does not depend on the thread
count, and that is checked row by row rather than asserted. Measured resident, with the incumbents
at the identical thread count:

```
$ just sweep mode=resident query=q6
threads   workers  filter  proj      engine   engine-scan  engine-ops     duckdb     polars  datafusion   engine x    duckdb x
---------------------------------------------------------------
      1        1       1     1       6.7           0.0          6.6       46.0       68.6         24.5       1.00x      1.00x
      2        2       2     1       3.8           0.0          3.8       25.0       35.3         15.0       1.75x      1.84x
      4        4       4     1       2.5           0.0          2.5       13.0       18.0          9.6       2.62x      3.54x
      6        6       6     1       2.1           0.0          2.1       10.0       12.3          7.3       3.18x      4.60x
      8        6       8     1       1.9           0.0          1.9        8.0       10.2          6.4       3.47x      5.75x
     14        6      14     1       1.8           0.0          1.8        7.0        9.5          6.0       3.64x      6.57x
```

**The engine is 6.9× faster than DuckDB at one thread and 3.9× faster at fourteen.** The sweep's
`workers` column is the read's, and in the resident mode it is the file's cap — six row groups —
rather than a count of anything that ran; the operators' own worker count is not capped by it, so
the engine keeps going where the read stops, and the `filter` column says how far. The **aggregate**
is not parallelised and never will be: `SUM` is not associative, and an answer that depends on the
block boundaries is not an answer.

**The engine's own curve is flatter than DuckDB's here, and the comparison still holds.** From
fourteen workers the engine gets **3.64×** and DuckDB gets **6.57×** — a resident query is
operators, not scan, and DuckDB's operators scale better. The engine is still ahead in absolute
terms at every thread count, by **6.9×** at one thread and **3.9×** at fourteen, and the answer
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
the engine was behind both incumbents. It is **574.4 ms** in this set, where the set before it had
**549.8 ms**; the load sweep below is a separate campaign and puts the same measurement at
**546.8 ms**. At one thread the engine is **2.2× ahead** of DuckDB on the same file:

```
$ just sweep mode=load
threads   workers        engine      duckdb     polars    datafusion   engine x
---------------------------------------------------------------
      1        1       546.8     1228.0     1019.4        328.9       1.00x
      2        2       379.2      601.0      509.0        224.9       1.44x
      4        4       292.2      410.0      343.8        120.7       1.87x
      6        6       216.5      223.0      190.3        119.7       2.53x
      8        6       216.9      220.0      181.5         69.8       2.52x
     14        6       211.3      219.0      180.0         70.6       2.59x
```

**At six, eight and fourteen threads the engine is 1–4% ahead of DuckDB**, because the engine's read
is capped by the file's six row groups and DuckDB's is not — a reversal of the set the `parquet`
bump replaces, where DuckDB was 4–5% ahead at six and fourteen. One thread is the like-for-like
comparison, and there the engine is 2.2× ahead.

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
now **14.9 ms**, against DuckDB's **35.0 ms** on the same file at the same thread count: the engine
is **2.3× ahead** on the headline query, where it was 2.6× behind. The first cause was not the decode
and not the comparison — it was the pushdown's mask, handed to the reader as a **row filter**,
which parquet-rs turns into a list of **2,775,138 row ranges** for 2,178,861 surviving rows and
applies one range at a time. Reading a column through that list costs 50.0 ms; without it, 7.8 ms.
The engine keeps the mask now — one bit per row — and applies it itself. See
[ADR-0015](docs/adr/0015-the-engine-keeps-the-mask.md). The second and third were the scan's own
shape: it was copying the rows it discards and the column nobody asked for, and its batch size was
a constant measured for a scan shape that has since changed three times. See
[ADR-0017](docs/adr/0017-the-scan-filters-only-what-it-keeps.md). The fourth was the batch size
itself: the constant became a **divisor of the file's row-group row count**, which is **21% faster**
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
  columnar-testdata  a seeded, reproducible TPC-H-shaped `lineitem` and `orders` generator
  columnar-engine    filter -> project -> [join] -> [sort] -> aggregate, with late
                     materialisation
  columnar-store     the write side: a writer, a manifest, a commit and a catalog --
                     the files the engine reads, and the truth about which ones
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
specs/017-the-repository-goes-public/  the licence, the contribution guide, and the public
                     x86-64 runner
specs/018-the-page-on-a-static-host/  the page, published on a static host
specs/019-the-button-always-answers/  the button that never disables itself
specs/020-the-engine-joins-and-sorts/  the hash join and the sort, measured against the same
                     incumbents, and the defect the increment found
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

**The versions this tree uses.** The engine reads Parquet through the **`parquet` crate, 58.4.1**,
and Arrow through **`arrow`, 58.4.1** — one line in the workspace's shared dependency table, so the
version cannot drift between crates. The bump to 58.4.1 is the smallest one that closes the
hostile-file hole the repository measured, and it is measured: [`PERF.md` §3w](PERF.md#3w-the-readers-four-gigabytes)
and [ADR-0049](docs/adr/0049-the-readers-four-gigabytes.md). The toolchain is **Rust 1.97.1**,
the version CI pins to.

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
| The project's status | [`STATUS.md`](STATUS.md) — the summary of where the project stands: what is built, what is measured, what the increments found, and **what is still open**. Every figure in it is checked against the committed reports by the same gate that checks this file's. |
| The comparison kernel, on AArch64 / NEON | **measured**, six campaigns of eight iterations, spread published |
| The comparison kernel, on x86-64 / AVX2 | **measured** — the kernel A/B on GitHub-hosted x86-64 runners, **ratios only**, on a named CPU, with the machine published and never compared with the ARM64 numbers. Its **correctness** is verified twice over: under Rosetta 2 on this ARM64 Mac, and on **native x86-64 hardware**: [`PERF.md` §1b](PERF.md#1b-the-kernel-on-x86-64-measured-natively) |
| The engine's filter, project and aggregate | **measured**, against DuckDB 2.0.0 and Polars 1.36.1 |
| The engine's **arithmetic aggregate** | **measured** — the real TPC-H Q6, `sum(l_extendedprice * (1 - l_discount))`, against the same SQL on the same file |
| The engine's **hash join** and **sort** | **measured** — an inner equi-join executed as a hash join, with `orders` as the build side and `lineitem` staying on the lazy path, and a sort that keeps a bounded heap when the query has a limit. Both are published against the same three incumbents, on the same two files, with the same SQL and the answers cross-checked: [`PERF.md` §2](PERF.md#2-the-engine-against-duckdb-polars-and-datafusion) |
| The join's probe and the sort, **taken apart** | **measured** — a committed instrument, `crates/columnar-engine/examples/operator_probe.rs`, decomposes both operators and **checks every candidate against the engine's own output, row for row, before it is timed**. The join's probe went from **102.7 ms** to **9.0 ms** and the engine's bounded sort from **13.3 ms** to **3.7 ms**, and a **bitmap in front of the table was measured and rejected**. The old code was re-measured on the same host, so the code's share of the published tables is measured rather than asserted: [`PERF.md` §2h](PERF.md#2h-the-hot-paths-taken-apart) |
| The engine's Parquet reader | **measured**, in two modes: eager (all sixteen columns) and lazy (two of sixteen, with the predicates pushed in) |
| The **read** under several thread counts | **measured** — the engine and both incumbents, at the same thread counts, on the same file: [`PERF.md` §2e](PERF.md#2e-the-thread-count-sweep) |
| The **query** under several thread counts | **measured** — the same sweep with the data already in memory, which is where the operators are the measurement: [`PERF.md` §2f](PERF.md#2f-the-resident-sweep-where-the-operators-are-the-measurement) |
| Multithreading of the **operators** | **measured** — the filter and the projection are parallel, and the answer does not depend on the thread count. The **aggregate** is not parallelised and never will be: `SUM` is not associative |
| The **eager** path's decode | **measured** — the eager path splits the file's row groups across the workers, and the merged dataset is bit-identical to the serial one's. It gets the same derived batch rule the lazy path got: [`PERF.md` §2g](PERF.md#2g-the-load-sweep--where-the-engines-biggest-number-was) |
| The **lazy** path's scan, decomposed | **measured** — Arrow's own decode, the engine's kernel, and the mask are separated, and the decomposition is in the tree as `crates/columnar-engine/examples/lazy_scan_decomposition.rs`: [ADR-0015](docs/adr/0015-the-engine-keeps-the-mask.md) |
| The comparison kernel, **in a browser** | **measured**, on a named browser and a named machine, with the host's load — and never compared with the native number: [`PERF.md` §1a](PERF.md#1a-the-kernel-in-a-browser) |
| The engine against a **Rust peer** | **measured** — `DataFusion` 49.0.2 reading the same Parquet through its own Arrow stack, in the same harness, at the same thread count. It is a **separate cargo project** under `tools/`, so the engine's dependency graph does not move: [`PERF.md` §2](PERF.md#2-the-engine-against-duckdb-polars-and-datafusion) |
| The **materialised column**, taken apart | **measured** — the column the query materialises is **61%** of the real Q6, and its cost is **bytes**: Arrow decodes it in **32.84 ms** and the parquet crate in **36.63 ms**. The counterfactual — the same column written *with* a dictionary — is 30% bigger and 7% slower: [`PERF.md` §3h](PERF.md#3h-the-materialised-column-and-what-it-is-made-of) |
| The query, under several **dataset sizes** | **measured** — the real Q6 at 6, 12 and 24 million rows, with every incumbent at every size. The cost per million rows is **flat**: **9.7**, **9.5** and **9.7** — the six-million-row figure is the set this file publishes now, and the two larger files were not re-measured after the bump: [`PERF.md` §3i](PERF.md#3i-the-curve-over-the-size-of-the-data) |
| The **row-group count**, as a knob | **measured** — the row-group size is a generator flag now, and the engine's parallelism cap was measured on files with six, twelve and twenty-four row groups. The cap is `min(threads, row_groups)`, exactly. The engine used to pay about **0.83 ms a row group** that DuckDB, Polars and DataFusion did not pay, so the file with the most parallelism was the slowest in absolute terms; **spec 016 measured the mechanism and removed it** — the reader's batch size is derived from the file's row groups now, and the engine's own scan costs **0.04 ms a row group**, flat across the three files: [`PERF.md` §3j](PERF.md#3j-the-row-group-count-and-what-it-costs) |

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
| Specs | `specs/001-columnar-core/` — R1..R5, with the out-of-scope table · `specs/002-lazy-columnar-scan/` — R1..R8, the lazy scan and the pushdown · `specs/003-kernel-pushdown/` — R1..R8, the pushdown running the engine's kernel · `specs/004-performance-gates/` — R1..R8, the regression and budget gates · `specs/005-parallel-decode/` — R1..R10, the parallel read and the thread-count sweep · `specs/006-arithmetic-aggregate/` — R1..R10, the arithmetic aggregate and the real Q6 · `specs/007-parallel-operators/` — R1..R6, the parallel filter and projection · `specs/008-parallel-eager-decode/` — R1..R6, the parallel eager decode and the batch size it inherited · `specs/009-the-engine-keeps-the-mask/` — R1..R8, the lazy scan's decomposition and the mask the engine keeps · `specs/010-fuse-the-projection-into-the-aggregate/` — R1..R9, the global aggregate reading the source in place · `specs/011-the-scan-filters-only-what-it-keeps/` — R1..R6, the scan compacting the survivors once, and the batch size it inherited · `specs/012-the-scan-does-not-expand-the-predicates-column/` — the scan's own decode and the expansion of its RLE indices, both measured and refused · `specs/013-the-kernel-in-a-browser/` — R1..R6, the third SIMD path, the page that checks it, and the number it reproduces · `specs/014-the-materialised-column-and-a-rust-peer/` — R1..R4, the materialised column taken apart, the curve over the size of the data, and a Rust peer beside the incumbents · `specs/015-the-row-group-count-is-a-knob/` — R1..R4, the row-group count as a knob, the parallelism cap measured on three files, and the per-row-group cost the incumbents do not pay · `specs/016-the-per-row-group-cost/` — R1..R4, that cost's mechanism measured, the reader's batch size derived from the file, and the defect removed · `specs/017-the-repository-goes-public/` — R1..R5, the licence, the contribution guide, three files out of the history, and the x86-64 measurement the public repository made possible · `specs/018-the-page-on-a-static-host/` — the page, published on a static host · `specs/019-the-button-always-answers/` — the button that never disables itself · `specs/020-the-engine-joins-and-sorts/` — R1..R6, the hash join and the sort, the two queries they published, and the defect the increment found · `specs/021-the-hot-paths-taken-apart/` — R1..R6, the join's probe and the sort taken apart by a committed instrument, the hot paths rewritten, the estimates made honest and tested, and the negative result published · `specs/022-the-dictionary-counted/` — R1..R5, the dictionary counted, the alternative closed, and the residual attributed to the crate · `specs/023-the-scan-that-feeds-the-join/` — R1..R5, the join's scan taken apart, the probe moved inside the scan, and the copy of the rows the join discards removed · `specs/024-the-time-the-plan-output-does-not-name/` — R1..R5, the plan output's own account of the query, the selection's expansion measured and refused, and the rows handed to the join and the sort as the range they are · `specs/025-the-sort-taken-apart/` — R1..R4, the sort's stage taken apart against the engine's own output, the floor that was a scalar read replaced by the comparison made in blocks, and the scan that feeds the sort measured and refused · `specs/026-the-join-stage-taken-apart/` — R1..R9, the join's stage taken apart against the engine's own output on both paths, the two paths' difference named, five rules of the join given one owner each, the build side's estimate made an upper bound, the join's helpers given the pipeline's context, and the two documents that said the opposite of the code corrected · `specs/027-the-plan-decomposed/` — R1..R8, the plan's two faces split into two modules, the mapping made one type and its panic made an error, the scan's decision given a module, the pipeline's ten parameters made a context, the stages given one shape, the estimates one home, and the crate root reduced to the surface · `specs/028-the-probes-remainder/` — R1..R7, the probe's **3.095 ms** remainder, the overflow map that is consulted once a probe row and holds nothing for this data, the map's share measured at **0.020 ms** and the rest named as the lookup's shape · `specs/029-the-lookup-shape-and-the-stage-that-moved/` — R1..R8, the lookup's shape measured part by part — each part priced by a candidate that differs from the plain one in exactly one thing — the resident stage's movement taken with the **floor** beside it, and the documents corrected where they named two costs the fast path does not pay · `specs/030-the-context-taken-apart/` — R1..R8, the context between the join stage's own probe and the same probe measured outside the query, decomposed into named gaps — the build side's, the probe's, the clock's bracket, the memory's state — each priced by a candidate that differs in exactly one thing, with the build side's in-query cost measured directly rather than inferred by subtraction, and the verdict published in one of five words |
| Increment | storage → filter → project → evaluate → aggregate (the TPC-H Q6 path), with two ways to read a file: eager and lazy — and now a **hash join** and a **sort**, with the pipeline `scan → filter → project → [join] → [sort] → aggregate`, and a join whose probe runs **inside** the scan, and a selection that holds every row **is** the range, and a sort that compares its keys **in blocks** against the heap's own worst key |
| Licence | Apache-2.0. The C++ baseline in `crates/columnar-kernels/src/` is this repository's own work and carries the same licence, and there is no vendored third-party source in the tree. [`CONTRIBUTING.md`](CONTRIBUTING.md) is the method, written for a stranger. |
| Tests | 375, all green, including the differential kernel suite, the lazy-scan differential suite, the expression suite, the parallel-operator suite, the parallel-eager suite, the fused-aggregate suite, the performance-gate suite, the browser's column suite, the RLE decoder's suite, the memory-estimate property tests, the attribution suite, the sort's suite, the join's key-source suite and the x86-64 cross-check |
| Lint | `clippy --workspace --all-targets` with `all` + `pedantic` as errors; zero warnings |
| CI | fmt, clippy (ubuntu + macos), test, the differential job, an x86-64 cross-compile check, `cargo deny`, a no-committed-dataset check, the published-figures check, and the two benchmark runners. **The repository is public**, so the runners are free — but the account's own Actions spending limit still applies to every job, and when it is exhausted *every* job fails at once with a billing message rather than a test failure. `just check` covers all of it locally and does not. |
| Gate | `just perf-gate` — a Mann-Whitney test against a stored baseline, with a measured practical threshold. It can also say *inconclusive*, which is not a pass. |
| Next | **The next question was the probe's remainder — the part of it that is **not** the map — and the **resident** path's stage, which moved and could not say why** — it is `specs/029-the-lookup-shape-and-the-stage-that-moved/`, and it is **answered**. **The map was never the remainder**, and spec 028 measured it: the candidate **with** the overflow map's consult costs **7.106 ms** against the candidate without it at **7.157 ms**, so the map's share is **0.020 ms** of a **3.337 ms** remainder, each set's own three campaigns judged against **its own** candidate spread — **−0.221 ms … 0.406 ms** against **0.709 ms** in the old set and **−0.278 ms … 0.071 ms** against **0.168 ms** in the new one — so the share is **at or below the noise** in both. **And spec 029 measured the rest of it, part by part** — the chain, the width, the state through `&self`, the map's `Option`, the engine's own loop and the key source, each priced by a candidate that differs in exactly one thing: the remainder is **0.109 ms**, with a spread of **1.083 ms**, and the engine's own probe (**8.395 ms**) is as fast as the hand-written candidate (**8.315 ms**). **The lookup's shape is free, and the lookup does not change.** The published **3.337 ms** held two artefacts of the instrument, not the engine's cost: the **crate boundary** (**+1.332 ms** — the engine's lookup is a private method and the instrument is another crate, so it was not inlined there and the instrument paid a call a probe row the query does not pay) and the **buffer** (**1.158 ms** — the candidate read a warm column while the engine's probe read one the reader had just written). The lookup is now marked `#[inline]`, which changes nothing the query does, and every rung materialises its column afresh. **And the resident stage's movement was the host's**: measured **in the same minutes**, three campaigns each, interleaved, the two codes' resident stages are **19.108 ms** and **18.976 ms** — **0.132 ms** apart against a spread of **0.568 ms** — and the **floor**, the instrument's own measure of what a memory-bound loop costs on this host, is **5.754 ms** against **5.690 ms**; in spec 028's session the stage moved **2.309 ms** and the floor **1.514 ms**, in the same direction and the same order, and here **neither moves**. **The next question was the context**: the stage's residual and the standalone probe of the same work are **1.069 ms** apart on the resident path, and the **cold** gap changes sign between the two code sets — **+0.494 ms** on the code that ships against **−1.440 ms** on the code it replaces. It is written down as `specs/030-the-context-taken-apart/`, and it is **answered**: the join stage's plan output now reports its **parts**, so the build side's in-query cost is the engine's own account of itself and not an inference, and the decomposition closes **exactly** — the reconciliation is **0.000 ms** on both paths. **And the verdict was unattributed on both.** Measured against the context's own spread: the context on the resident path was **−1.565 ms** and the resident path's spread was **0.418 ms**, so the context was a real thing there — but every part's own spread was wider than that band, and no mechanism could be claimed; the context on the cold path was **0.433 ms** and the cold path's spread was **2.211 ms**, so on that path the context was at the noise too. **The next question was the resolution**: the parts are measured, they close, and the band is the host's — what would make the parts separable. It is written down as `specs/033-the-drift-taken-apart/`, and it is **answered**: the band the parts are judged against is the context's own spread, which is a **difference of two drifting series** and can be narrower than the machine's own movement, so the **drift** had to be measured before the parts could be judged. **And it is**: the two sides of every difference are now measured **in the same iteration, back to back**, with the **order alternated** between the iterations, and the **drift** — the machine's own variation, measured on both sides — is the yardstick every part is judged against, and the context on the cold path is **1.382 ms** and the cold path's spread is **2.271 ms**, and the cold path's yardstick is **2.236 ms** — the probe's own remainder's own, the largest leaf on that path — so the verdict, on the cold path, is **unattributed** and the verdict's state, on the cold path, is **sound**. And the context on the resident path is **−1.354 ms** and the resident path's spread is **0.184 ms**, and the resident path's yardstick is **1.324 ms** — the probe's file state's own, the largest leaf on that path — so the verdict, on the resident path, is **unattributed** and the verdict's state, on the resident path, is **sound**: the probe's **file state**, **−1.208 ms**, its own spread of **1.354 ms** wider than its own yardstick of **1.324 ms**, so no part counts and no mechanism can be claimed. **The set the bump replaces read differently**: the context on the cold path was **0.199 ms** with a spread of **0.667 ms** and a yardstick of **7.860 ms**, the context on the resident path was **−1.581 ms** with a spread of **0.356 ms** and a yardstick of **0.487 ms**, and the resident path's verdict was **the memory's**, because the file state was then outside its own spread of **0.390 ms**. The cold path reads the file through the new Arrow, so the bump can be in that movement; the resident path never decodes a file, so it cannot be in it, and that movement is the host's and the engine's own variance. **The legacy band verdict is kept beside it, annotated not sound** — a band of **0.418 ms** against the machine's own **0.936 ms** was never a yardstick. **And the next question is the drift itself**, which is now the yardstick for everything and has never been taken apart: the machine's own floor moved **0.362 ms** across one session's three campaigns, but the join stage moved **4.731 ms** on the cold path and the build side's read of the build file alone moved **10.330 ms** — so **the yardstick is as wide as the engine's own cold read makes it**, and that is why the cold path's parts cannot be told apart. **Which of the drift's parts is the engine's, and how much of the engine's own cost moves under the measurement?** **And the next increment is not that question: it is the engine, as a library.** The engine half of a database already exists — it runs a typed plan over Parquet, and it is measured against DuckDB and Polars — but **the library does not ship**: `cargo package` fails outright, because the three crates have to be published in the order their dependencies decide; the packaged crate carries the repository's root README instead of its own; the public surface is the research harness's, with the research modules public and no `prelude`; the join's build side is a `PathBuf`, and there is no table and no name; the enums that will grow are not `#[non_exhaustive]`; and all **fourteen** measurement probes would ship to crates.io. It is written down as `specs/034-the-engine-as-a-library/`, with its requirements, its tasks and its plan's shape, and **nothing has been implemented for it yet**. **The drift's question waits one increment, and the reason is recorded in that spec's scope**: this increment changes no engine work, so every published figure must stand unchanged. The join's stage was taken apart in spec 026 and the answer was that it is **the build side's read** — **11.83 ms**, more than half of it, and Arrow's decode of `orders.parquet`, which has been measured three times and refused every time — and the **probe**, **10.05 ms** over six million rows. The probe paid **3.10 ms** more than the same lookup written out by hand over the same rows and the same table, and the engine's own per-row lookup named a **candidate** for it: it consulted the **overflow map** once a probe row, and the map is **empty** for this data, because every key's count fits in its byte. **That candidate has been measured, and it was not the reason**: the map's share is **0.020 ms**, and the remainder is the lookup's shape. **The five questions that were here before are answered.** **The join's stage** was the last of them: it was taken apart against the engine's own output, on both paths, in one session — and the two paths disagreed, reporting `216783` rows in on one and `6000000` on the other for the same probe, so the stage now reports the rows the probe **consumed** and the two paths say `6000000 → 216783` alike. The stage's difference is **1.52 ms** here and **2.9 ms** in the published campaigns, and it is in the **probe**: the build side's read is the same call on both paths. The **review** found five rules of the join with **two owners each**, an estimate that was **not an upper bound**, helpers left behind by the pipeline's context, and two documents that said the opposite of the code; all ten fixes are in the tree, and the **overflow map's** cost is the one thing the measurement found that the engine can still have. **And the four questions that were here before are answered.** **The sort's gap** was the last of them: the sort's stage went from **4.18 ms** to **1.10 ms** cold and from **4.30 ms** to **1.03 ms** resident, and the engine's cold `sort-topk` from **42.6 ms** to **37.2 ms** against DuckDB's **39.0 ms** — **ahead of DuckDB on this query for the first time** — because the sort's own work was a comparison made a row at a time, and the same comparison made in blocks costs **0.96 ms** against the operator's old **3.23 ms**. The floor the sort was measured against — **3.6 ms**, a plain read — was a *scalar* read, and it was never a floor: it was the cost of counting the values one at a time, and the sort was counting them one at a time too. The **scan that feeds the sort** was measured and **refused**: the top-k over the reader's own arrays costs **6.3 ms** on top of the decode, against the stage's **4.18 ms** for the whole of it. **And the three before that are answered.** The **plan output** now accounts for the whole query: the instrument committed in the tree (`query_cost_probe.rs`) found that the join and the sort asked the filter's selection for **six million row numbers** — **24 MB** — before either stage's clock started, so the stage table left **4.7 ms** of the join's query and **4.3 ms** of the sort's **outside every stage**. A selection that holds every row is **the range** now, the expansion is **gone rather than renamed**, and the account closes to **0.02 ms** on every query and on both paths. The **dictionary** is counted: the crate hands back a **new** dictionary array for every batch — **0 of 23** consecutive batches share one, checked by pointer identity — so "compare the distinct values once a row group" is impossible with this reader, and the kept shape is **18.3× slower** than the expansion it replaces; the **0.14 ms a row group** left on the three-predicate shape is therefore the **crate's** read, at **0.07 ms a row group**, and the engine's own comparison is **flat** at −0.01 ms a row group. And the **join's gap to DuckDB** was 7 ms and was **level** in the set spec 026 published — **96.7 ms** against **95.0 ms**, inside their spreads — because the scan feeds the join, the probe runs inside the scan over the reader's batches before anything is copied, and **96.4%** of the copy the join used to make was thrown away. In the set this file publishes now the join is **6.0 ms behind** — **100.0 ms** against **94.0 ms** — and **that gap is DuckDB's own figure that moved**: DuckDB's cold join rose from **91.0 ms** to **94.0 ms** across the two sets while the engine's rose from **97.6 ms** to **100.0 ms**, so the gap is **0.6 ms smaller** than it was and the whole of that is DuckDB's own movement; and the engine's **old code, measured on this same host in this same session**, reads **95.0 ms** against the same DuckDB's **90.0 ms**, so the gap is there for the code spec 026 shipped as well. The increment that closed the sort's gap did not touch the join: the join's stage was **24.2 ms** in the set spec 026 published against **25.7 ms** for the old code's on that host, and the query moved by **+0.8%** — the machine's. What is left of the cold path's one other gap is `filter-range` — the lazy path is 15.8 ms against Polars' 16.5 ms, and the two spreads (15.1–19.2 ms against 16.4–16.7 ms) overlap, so the engine is **0.7 ms** ahead here where the set this file published before had Polars **1.1 ms** ahead; the **resident** path still wins it, **7.7 ms** against Polars' **13.3 ms**. The scan that holds it is **Parquet decode**, and the decode was measured **three** times and **every time it was refused**: owning the decode buys nothing (the two shapes came out equal to a tenth of a millisecond), and neither does **not expanding the predicate's column**. The file's columns *are* dictionary-encoded, and the expansion of six million RLE indices into six million values was measured at **1.79 ms** — 46% of the decode of the predicate's column, and **4.1% of the query**. The mask the hypothesis would build instead costs **1.73 ms**, which is **0.42 ms** more than the vectorised predicate costs today once the two are netted off, and the parquet crate does not expose the indices at all, so capturing the expansion means owning the Parquet reading: **the net is 1.37 ms.** And the **materialised** column — the query's real cost, **61%** of it — was taken apart too: it is **bytes**, Arrow decodes it in 32.84 ms and the parquet crate in 36.63 ms, so there is nothing there to win either. After that: the morsel driver (still worth ~0 ms in the cold path, because the pushdown leaves the operators nothing to do), and the **x86-64 absolute figures**, which need a **pinned** x86-64 runner — the `avx2-self-hosted` job is already wired for one, and it is the only job whose absolute figures would be worth publishing. **And the increment after that was the engine, as a library — it is written down as `specs/034-the-engine-as-a-library/`, and it is answered.** The research harness now ships as a library: the surface is decided and written down — **45 names**, closed by the rule that every type in a public signature is on the list — the three crates package **in the order their dependencies decide** (`columnar-core`, then `columnar-kernels`, then `columnar-engine`), each carries its own README and its own licence, the research modules are reachable and undocumented, the enums that will grow are `#[non_exhaustive]`, and the embedder test imports **only** the facade. **And the figures did not move**: the campaigns were re-run on the tree the increment ships and — **back to back, on this host** — on the tree it replaces, both trees clean, and **the code's share of every difference is nil**. The join's cold stage reads **18.366 ms** against the old code's **18.455 ms** — a difference of **−0.089 ms** against a drift of **1.237 ms** — and the cold `join-orders` query reads **82.0 ms** against **82.5 ms**. The movement between the published set and this one is the **host's**: DuckDB's `join-orders` cold moved **91.0 ms** to **85.0 ms** in the same sessions, and the old code measures the same as the new one. **What is left of that increment is the publish itself**: the three names are free, and `cargo publish` needs a crates.io account and a token. After that: the **drift's own parts**, which waits one increment still — the reason is recorded in that spec's scope — and the **x86-64 absolute figures**, which need a **pinned** x86-64 runner **And the increment after that was the drift's own parts — it is written down as `specs/035-the-drifts-own-parts/`, and it is answered.** A **drift** is a range, and a range is not additive, so the decomposition is made of **movements** — the difference between two campaigns — and it closes exactly: the reconciliation is **0.000000 ms** on both paths and on both sets. **And the answer is that the drift has three parts — the machine's own movement, the file's state and the engine's own variance — and which of them is largest is a property of the host's state on the day, not of the code**: on the busy host the published set was taken on, the cold path's movement is the file's state (**+1.179 ms**) and the build side's gap (**+1.054 ms**) against the machine's own **+0.205 ms**, and the build side's read's own series shows three excursions above 22 ms in one campaign; on the quiet host the same read never breaks away and the machine's own **+0.301 ms** is larger than the cold path's whole movement. **And how much of the engine's own cost moves under the measurement**: **18.72%** of the cold path's cost and **1.75%** of the resident path's in the published set — so the engine's cost moves by about two per cent on the path that holds the data, and by about a fifth on the path that reads the file. **And the increment after that was the file-state floor — it is written down as `specs/036-the-file-state-floor/`, and it is answered.** The **file's own read** — the build side's read through the engine's own reader, with the engine's own predicates — is now the **file-state floor**, measured **in the iteration**, right after the **machine's own measure**, and the two are **8.565 ms**, **8.871 ms** and **8.793 ms** apart. **And a published sentence is corrected where it was published**: PERF.md §3q and ADR-0041 said the floor was measured *in the same iteration, back to back, with the order alternating*; the **sides** were, and the floor was measured in its own block and paired by index, so the machine it took out was the machine's movement **in another block**. **And the yardstick now sees the file's state as well as the machine's**: the machine's drift, the file-state floor's drift, and the larger of them — **0.258 ms** and **0.361 ms** on this day, and only the open's gap's yardstick changes, from **0.258 ms** to **0.361 ms**. **And the attribution is published as it came out**: the excursions did not recur — over **nine more campaigns** the file's own read's largest spread within a campaign was **1.805 ms** against the machine's own movement's **1.842 ms** — so the published set's excursion at **38.640 ms** is not re-attributed, and what the increment makes true is that **the next time the file's state moves, the two will be measured apart, in the same iteration.** After that: the **library's publish**, which needs a crates.io account, and the **`parquet` bump**, whose target version exists today. **And the increment after that was the write side — it is written down as `specs/037-the-write-side/`, and it is answered.** Every published figure in this repository was a figure about **reading**: nothing in the tree wrote a table, and an embedded store is made of a **write path**, a **commit protocol** and a **catalog that survives the process**. **And the write side is now measured.** The write of the published file is **1844.195 ms**, one row group costs **306.230 ms**, and the **commit** — the data file's `fsync`, the manifest written and `fsync`ed, the `rename`, the directory's `fsync` — is **7.651 ms**, which is **0.004** of the write: **the protocol's shape is not constrained by the flush on this host**, and the store needs no write-ahead log and no rollback to be affordable. **And the dictionary stays on**: the same rows written plain take **1.55×** the bytes and read back **2.60×** slower. **And the batch size is a property of the bytes**: at **65,536** rows the probe's file **is** the published file, byte for byte, and at the engine's own **125,000** it differs **from byte 7** — the writer cuts its mini-batches, and the dictionary's fallback with them, at the caller's batch boundary. **And the write costs 3.36× a full read**, so *write once, read many* pays for itself after about four full reads. After that: **the store's own code**, and the work the repository cannot take on its own — **the library's publish**, which needs a crates.io account, and the **`parquet` bump**, whose target version exists today. **And the increment after that was the store's half — it is written down as `specs/038-the-stores-half/`, and it is built.** `columnar-store` is a **writer**, a **manifest**, a **commit** and a **catalog**: a table is a **set of files**, a commit appends one and never modifies an old one, and the **manifest** is the table's truth. The commit is the **five steps** spec 037 measured — the data file durable, the manifest written, durable and renamed over the old one, the directory durable — and **there is no write-ahead log**: a commit is a rename, and the rename is the answer. **And the batch size is a rule now, not a surprise**: the grid is **65,536**, the store **checks** it, and a batch off the grid is refused with an error. **And the acceptance is the strongest one available**: fed the published file's own rows, the store wrote **242,857,841** bytes — **the published file, byte for byte** — in every campaign. The crate sits **below** the engine: its **library** does not depend on `columnar-engine`, so the engine's read path cannot move because of anything here. After that: **the engine's side of the glue** — the source that reads a table which is **many** files, which the engine cannot do yet — and, beside it, the work the repository cannot take on its own: **the library's publish**, which needs a crates.io account, and the **`parquet` bump**, whose target version exists today. **And the increment after that measured the file boundary's cost — it is written down as `specs/039-the-engine-reads-a-stores-table/`, and the boundary is free.** The store writes a table as **many** files and the engine reads **one**, so what the boundary costs had to be measured before the engine was taught to read the set. **The experiment was the published file's own six row groups** — the same rows as the published file — written as six files and read **both ways**: **66.003 ms** for the one file and **64.114 ms** for the six, a difference of **−1.152 ms** inside the one file's own spread, and the rows the same, compared **row by row, in order**. **So the file boundary costs nothing measurable**, and **the source's case is correctness, not performance** — the naive per-file read is wrong for a filter, an aggregate and a join, so a source that reads the set as one table has to be **correct, and free**. **And the source itself is built — it is written down as `specs/040-the-source-that-reads-a-set-of-files/`, and it is answered.** The engine gained **`FileSetSource`**, **one new name** on the surface, and `execute_on` takes a `ParquetSource` exactly as it did. The set reads the files **as one table**: the metadata merged, the statistics the **table's** — the range is the **union** of the files' ranges — the engine's own predicate reaching **every** file, and the rows in the files' order. **And it is free**: the set's read is **−0.599 ms** against the one file's, negative and inside the one file's own spread, and in parallel **0.133 ms** against **35.123 ms**, a share of **0.004**. **And the way it is free is the way that matters**: the files' readers are **chained** and decoded in **one pass**, so the rows are copied **once** — the first implementation merged the files' chunks instead, and the second copy was worth **8.747 ms**, **13%** of the read. **And the answers are the engine's**: the set's rows are the one file's, compared **row by row, in order**, and the same filtered plan counts the same **3,184,637** rows over the file and over the set. **And a published sentence was wrong and is corrected where it was published**: specs 039 and 040 said the pushdown **prunes row groups by the file's statistics**, and it does not — ADR-0010 accepted that cost, and the statistics are used in two places, and **both of them are estimates, not reads**. **And the store's half is whole, and publishable**: `Table::paths` is the glue — the list of paths the engine's source takes — and the crate **packages** now, after two things nobody had checked: its dev-dependency on the generator had to become **path-only**, because a versioned dependency has to resolve on crates.io and the generator is not there, and the package carried **no licence file**. **And ADR-0044's reason for holding the store back was wrong twice over**: the store does **not** depend on the core — its **library** depends on nothing in this workspace — and the increment that was named did not change its API at all. **The four crates are ready to publish**, in the order **core, then kernels, then store, then engine**. **And the composition is a test**: the store's table, read by the engine, as one table — the claim the store's half rests on — was checked **by hand**, and **the hand check was wrong the first time**, so it is in the suite now: the store's own tests write two commits, take the table's paths, open the set with the engine's source, and say that the set holds the table's rows **row by row, in the files' order** and that the engine's predicate reaches **every** file. **And the edge that test needs is a dev-edge, path-only** — the engine never depends on the store, and the store's library still depends on nothing in this workspace. **And the increment after that measured what a hostile file asks of the reader — it is written down as `specs/041-the-readers-four-gigabytes/`, and the answer is that the four gigabytes are in the file's **page headers**, not in its metadata.** The probe writes a real file with one hand-written page header whose statistics field declares a length, and reads it through the engine's own reader with a counting global allocator around it: **the largest single allocation the reader asks for is the declared length, exactly** — 4,294,967,295 at 4 GiB, out of a 25-byte header — and **the cost is address space, not time**. **And the published sentence is corrected where it was published**: the metadata path is bounded, and what closes the hole is `parquet`'s own reader, which from 58.4.1 does not read the page statistics by default. After that: the work the repository cannot take on its own — **the publish**, which needs a crates.io account and a token |
