# The project's status

**What this file is.** A summary of where the project stands: what is built, what is measured, what
the increments found, and what is still open. **It is not a second copy of the figures.** Every
number below is the headline of a table that [`PERF.md`](PERF.md) publishes in full, with its
spreads and its raw reports — and `scripts/check-published-figures.py` checks each one here against
the same committed reports it checks there. A figure here cannot drift away from the evidence
without the gate failing.

---

## Where the work is recorded

Every increment is a **spec** under [`specs/`](specs/), with its requirements (R1…Rn), its tasks and
the decision it took; the decisions are kept as **ADRs** under [`docs/adr/`](docs/adr/). The method
is in [`CONTRIBUTING.md`](CONTRIBUTING.md), and the rules the method answers to are in
[`constitution.md`](constitution.md). **31 specs**, **39 ADRs** and **29 defects** so far, and every
one of the specs but the last is closed.

---

## What the engine is

A vectorised columnar query engine in Rust, with a comparison kernel in C++ and Rust behind a thin
C ABI, and a measurement harness that is willing to say the engine loses.

The pipeline is `scan → filter → project → [join] → [sort] → aggregate`, over the TPC-H Q6 path,
plus an inner equi-join and a bounded-heap sort. The join's probe runs **inside the scan**, over
the reader's own batches; the sort compares its keys **in blocks** against the heap's own worst
key. There are two ways to read a file: eager (all sixteen columns) and lazy (two of sixteen, with
the predicates pushed in).

Apache-2.0. The C++ baseline in `crates/columnar-kernels/src/` is this repository's own work and
carries the same licence; there is no vendored third-party source in the tree.

---

## Where it stands

**Against DuckDB 2.0.0, Polars 1.36.1 and DataFusion 49.0.2, on the same files, at the same thread
count, with every answer cross-checked against the engine's own.** The headline tables are pinned to
**one thread** — the like-for-like single-threaded comparison. The thread-count sweeps are in
[`PERF.md` §2e–§2g](PERF.md#2-the-engine-against-duckdb-polars-and-datafusion).

### Cold — the file is read and the query answered in one go

| query | **engine** | duckdb | polars | datafusion | |
|---|---|---|---|---|---|
| `q6` | **15.4 ms** | 32.0 ms | 190.5 ms | 34.8 ms | the engine wins |
| `q6-revenue` | **58.9 ms** | 67.0 ms | 198.7 ms | 112.6 ms | the engine wins |
| `join-orders` | 97.6 ms | **91.0 ms** | 219.1 ms | 168.8 ms | DuckDB wins |
| `sort-topk` | 39.5 ms | **38.0 ms** | 1398.0 ms | 39.8 ms | DuckDB wins |
| `filter-range` | 16.3 ms | 31.0 ms | **15.2 ms** | 20.7 ms | Polars wins |

**Six of the eight cold queries in [`PERF.md`'s table](PERF.md#2b-cold--read-the-parquet-file-and-answer-in-one-go)
are the engine's**; the two it loses are named in the open questions below. The engine's eager path —
all sixteen columns — takes 552.3 ms for the same file, and is measured on its own in
[`PERF.md` §2g](PERF.md#2g-the-load-sweep--where-the-engines-biggest-number-was).

### Resident — the data is already in memory

| query | **engine** | duckdb | polars | datafusion | |
|---|---|---|---|---|---|
| `q6` | **9.4 ms** | 45.0 ms | 67.4 ms | 23.8 ms | the engine wins |
| `join-orders` | **26.6 ms** | 35.0 ms | 78.9 ms | 85.8 ms | the engine wins |
| `sort-topk` | **2.7 ms** | 3.0 ms | 1356.8 ms | 3.8 ms | the engine wins |
| `filter-range` | **6.6 ms** | 19.0 ms | 10.8 ms | 10.1 ms | the engine wins |

**The engine wins every query in this table**, against a Rust peer on the same Arrow stack. The
asymmetry is named: the incumbents have both tables in memory before their clock starts, while the
engine's resident path **reads the build side inside the clock**.

### The load — the file into memory

| threads | **engine** | duckdb | polars | datafusion | |
|---|---|---|---|---|---|
| 1 | **576.8 ms** | 1275.0 ms | 1069.8 ms | 362.3 ms | ahead of DuckDB and Polars, behind the peer |
| 6 | 251.0 ms | **239.0 ms** | 193.5 ms | 127.7 ms | behind all three |
| 14 | 243.9 ms | **235.0 ms** | 204.8 ms | 83.5 ms | behind all three |

**This was the engine's worst figure** — once the largest number the repository had published — and
it is now ahead of DuckDB and Polars at one thread and behind them at six. The engine's worker
count is capped at the file's six row groups, exactly `min(threads, row_groups)`, and the peer reads
the same file a different way.

### The join's probe, taken apart

The probe is the join's hot loop: six million rows, one a probe row, against a table that is one
byte a key. The instrument measures it against its own **floor** — the table's own load, which no
lookup can go under — and against the same lookup written out by hand over the same rows.

| | ms |
|---|---|
| the engine's own probe, over the engine's column | 8.860 |
| the floor the probe cannot go under | 6.147 |
| the same lookup, written out by hand | 8.953 |
| **the remainder** — the engine's probe over the hand-written one | **−0.092** |
| the join's stage, cold | 24.690 |
| the join's stage, resident | 19.431 |
| the build side's whole — the read, the keys and the table | 12.347 |
| the stage's residual — the stage's own probe plus the build side's own gap | 7.187 |
| the context — the residual against the standalone probe | −1.581 |

**The remainder this table publishes is −0.092 ms, with a spread of 0.178 ms** — inside the noise, and
**the lookup's shape is free** still. The figures moved against spec 030's set, and every one of them is
published old-and-new: the engine's own probe was **9.080 ms** and is **8.860 ms**, the floor was
**6.282 ms** and is **6.147 ms**, the hand-written lookup was **10.695 ms** and is **8.953 ms**, the stage
was **25.446 ms** cold and is **24.690 ms**, **20.368 ms** resident and is **19.431 ms**, the build side's
whole was **12.605 ms** and is **12.347 ms**, the residual was **7.386 ms** and is **7.187 ms**, and the
context was **−1.565 ms** and is **−1.581 ms**. **The code did not change** — the difference is the
**instrument's** and the **host's**, and the two sets were not taken in the same session: this one ran on a
host whose load average was **3.66–4.34**, spec 030's on one at **6.80–7.36**. The remainder was
**3.337 ms** before spec 029, and it held two artefacts of the instrument rather than the engine's cost —
the crate boundary, and a warm buffer where the query probes a column the reader has just written.

**And the verdicts, with the yardstick each was made with.** The context's verdict is published in the
house's own sentences, and the yardstick it was made with is published beside it:
the verdict, on the cold path, is **unattributed**;
the verdict's state, on the cold path, is **sound**;
the cold path's yardstick is **7.860 ms** — the probe's row-number offset's own, the largest leaf on
that path;
the verdict, on the resident path, is **the memory's**;
the verdict's state, on the resident path, is **sound**;
the resident path's yardstick is **0.487 ms** — the probe's file state's own, the largest leaf on that
path, and the one that counts.
**The legacy band verdict is kept beside it, annotated not sound**: a band of **0.418 ms** against the
machine's own **0.936 ms** was never a yardstick.

**And the instrument itself changed, because the drift was inside the difference.** The two sides of every
difference are now measured **in the same iteration, back to back**, and the **order** between them is
**alternated** between the iterations, because a difference of two series measured a campaign apart is a
difference of two **drifting** series. The decomposition, the drift, the method and the verdicts are in
[`PERF.md` §3o](PERF.md#3o-the-joins-stage-taken-apart), in
[`spec 033`](specs/033-the-drift-taken-apart/spec.md) and in
[ADR-0039](docs/adr/0039-the-drift-taken-apart.md); the context's own decomposition is in
[ADR-0038](docs/adr/0038-the-context-taken-apart.md).

---

## What the increments found

**29 defects**, all found by measurement, each recorded with its before-and-after numbers in the
ADRs and in the commit history. The list is
[`PERF.md` §4](PERF.md#4-the-defects-the-increments-found).

By kind:

- **Work done per batch, per predicate or per dictionary, where the shape of the problem does not
  need it.** The reader decoded each predicate column *twice*, once per predicate; the Arrow-to-chunk
  conversion copied the dictionary's distinct values per batch.
- **A total that was never attributed.** The scan's time was not split into the reader's decode and
  the comparison, and the comparison turned out to be the larger half.
- **A wrong answer published as a win.** The sort's bounded heap kept the best row instead of the
  worst, so the query returned *a* sum — a wrong one.
- **A rule with two owners.** Five of the join's rules had two owners each, and the stage reported
  two different things for the same work.
- **An estimate that was not an upper bound.** The join's build side was charged the file's whole
  row count for a table bounded by its distinct keys.
- **An instrument that measured the wrong thing.** The engine's lookup is a private method and the
  instrument is another crate, so it was not inlined there; the instrument paid a call a probe row
  that the query does not pay.

And the ones the measurement **refused** — the direct-addressed table's own shape, the probe's hash,
owning the Parquet decode, expanding the predicate's column, a bitmap in front of the table — are
refusals with the numbers that refused them, published as the finding.

---

## The method that holds it together

- **Measure first.** Every claim about the engine's cost comes from a committed report in
  `bench-results/`, taken as the median of at least three campaigns, with the host's load recorded.
- **One owner per figure.** A number is published in the section that measures it, and the guard
  checks it **there**, against the reports — not anywhere in the document, where an unrelated number
  could satisfy the check by accident.
- **Publish what moved, old and new, with the reason.** Every figure that changed is published
  beside the figure it replaced, and the reason is in the text.
- **A superseded set is kept under its own name.** The reports a comparison was measured from are
  preserved beside the new ones, so the old figure stays derivable from a committed report.
- **The instruments are in the tree.** `crates/columnar-engine/examples/` holds the probes that took
  the hot paths apart, and each candidate is checked against the engine's own output, row for row,
  before anything is timed.
- **A negative result is a result.** The refusal and the numbers that refused it are published.

---

## The open questions, and what is not done yet

### The questions the engine has not answered

**1. The context — it is decomposed, and on the resident path the memory answers it.** The join's stage's
residual is 7.187 ms on the resident path, and the standalone probe of the same work is 8.860 ms. The
gap is **−1.581 ms**: inside the query, the probe accounts for *less* than it does in a quiet
process. On the cold path the same gap is **0.199 ms** — positive where the resident one is negative
— and **0.149 ms** on the old code, measured in the same session.

**Spec 030 took it apart, and the answer was that it cannot be attributed.** The join stage's plan
output now reports its **parts**, so the build side's in-query cost is the engine's own account of
itself and not an inference; the decomposition closes **exactly** — the reconciliation is **0.000 ms**
on both paths — and the verdict was **unattributed** on both. The context was **real** on the resident
path, outside its own spread of **0.418 ms**, but **every** part's own spread was wider than that band,
so no mechanism could be claimed; on the cold path the context was inside its own spread of **2.211 ms**,
so it was at the noise there too. The parts were measured, they closed, and the band was the host's.

**Spec 033 took the drift apart, and the answer changed: on the resident path the verdict is the
memory's.** The band those parts were judged against was the **context's own spread** — a **difference
of two drifting series** — and it could be narrower than the machine's own movement. So the two sides of
every difference are now measured **in the same iteration, back to back**, with the **order alternated**
between the iterations, and the **drift** — the machine's own variation, measured on both sides — is
published and is the yardstick every part is judged against. The verdict is **unattributed** on the cold
path and **the memory's** on the resident path, and both yardsticks are **sound**: the resident path's
file state is **−1.238 ms** against a yardstick of **0.487 ms**, outside its own spread of **0.390 ms**.
The context's own spread came down with it — **0.418 ms** to **0.356 ms** on the resident path and
**2.211 ms** to **0.667 ms** on the cold one — and it is published old-and-new with the reason.
**What would answer it:** whether the file state is the whole of it — the same mechanism measured on the
cold path, where the verdict is still unattributed. It is written down as
[`spec 033`](specs/033-the-drift-taken-apart/spec.md), which is closed, and in
[ADR-0039](docs/adr/0039-the-drift-taken-apart.md).

**2. `filter-range`, cold: Polars is ahead.** The engine's lazy path is 16.3 ms against Polars'
15.2 ms, and the two spreads do not overlap. The scan that holds it is the Parquet decode, and the
decode has been measured three times and refused every time: owning the decode buys nothing, and
neither does *not* expanding the predicate's column — the expansion of six million RLE indices costs
1.79 ms and the mask the alternative would build costs 1.73 ms, so the hypothesis never pays for
itself. **What would answer it:** a file with a page index, or a reader that owns the Parquet
decode. Neither is in scope of what the project has done so far.

**3. `join-orders`, cold: DuckDB is ahead.** The engine is 97.6 ms against DuckDB's 91.0 ms, and
the published analysis says the gap is **DuckDB's own figure that moved**: DuckDB's cold join fell
from 95.0 ms while the engine's rose from 96.7 ms, and the engine's *old* code, re-measured on this
host in the same session, reads 97.3 ms against the same DuckDB's 92.0 ms — so the gap is there for
the code spec 026 shipped as well. The engine's stage was taken apart in spec 026 and the answer
was that it is the **build side's read** — 11.716 ms of the 21.731 ms cold stage — and Arrow's
decode of `orders.parquet`, which has been measured three times and refused every time.
**What would answer it:** the parts, not the total.

**4. The morsel driver.** The pushdown leaves the operators nothing to do on the cold path, so a
morsel driver has nothing to drive there. It has not been built, and nothing measured so far says
it would pay.

**5. The x86-64 absolute figures.** The kernel's AVX2 path's **correctness** is verified twice over
— under Rosetta 2 on this ARM64 Mac, with the row counter proving the instructions ran, and on
**native x86-64 hardware**, where the differential suite passes and the mask matches the oracle's
bit for bit. Its **performance** is published as **ratios, per processor**, because the hosted
runners are a fleet. The **absolute** figures are not published: they describe a shared runner, and
this repository does not publish a number it cannot reproduce. The `avx2-self-hosted` job is wired
for a pinned x86-64 runner; it has not run on one.

### The work the project has not done

- **The next question is the drift itself, and it is written down here and in the README's Next row.**
  Spec 033 is written down and closed, and it leaves one question: the **drift** is now the yardstick
  every part is judged against, and **it has never been taken apart**. The machine's own floor moved
  **0.362 ms** across one session's three campaigns, but the join stage moved **4.731 ms** on the cold
  path and the build side's read of the build file alone moved **10.330 ms** — so **the yardstick is as
  wide as the engine's own cold read makes it**, and that is why the cold path's parts cannot be told
  apart. **The resolution that would be needed is a cold-path drift below 0.878 ms** — the yardstick the
  build side's gap is judged against — and that is **twelve times** narrower than the read's own drift.
  **The next question is which of the drift's parts is the engine's, and how much of the engine's own
  cost moves under the measurement.** It is **not** a spec yet: nothing has been measured for it,
  and its instrument is not built.
- **The x86-64 absolute figures** need a pinned x86-64 runner. The job is wired and has not run.
- **The CI runners are contended.** The runs sit queued for a long time, and the account's Actions
  spending limit applies to every job — when it is exhausted, *every* job fails at once with a
  billing message rather than a test failure. The published figures are therefore taken on this
  machine and not on CI, and every report carries the host's load average.
- **The comparison campaigns were not taken again.** Spec 029's change does not touch the query's
  path, so no published query figure moves, and the comparison set in [`PERF.md` §2](PERF.md#2-the-engine-against-duckdb-polars-and-datafusion)
  is spec 028's, unchanged.
- **The official TPC-H SF-1 file.** The generator produces a TPC-H-*shaped* file with approximated
  distributions, UTF-8 dates and a seeded PRNG. The manifest says so, and the constitution names it
  as a boundary.
- **Window functions, decimal, dates, nested types, GPU.** Out of scope of spec 001, and still out.
  The hash join and the sort *were* in that list and are built now — spec 020.
