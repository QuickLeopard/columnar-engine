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
[`constitution.md`](constitution.md). **36 specs**, **43 ADRs** and **29 defects** so far, and every one
of the specs but the last is closed.

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

**And the same probe, measured again on spec 034's tree.** The probe was re-run on the tree that
increment ships and — **back to back, on this host** — on the tree it replaces: the stage's cold path
reads **18.366 ms** against the old code's **18.455 ms**, a difference of **−0.089 ms** against a drift
of **1.237 ms**, and the floor reads **5.376 ms** against **5.361 ms**. **The figures above stand**: the
movement between them and this set is the **host's** — the incumbents moved with it, and the old code
measures the same as the new one. [`PERF.md` §3p](PERF.md#3p-the-joins-stage-measured-again) has the
table, and [§2i](PERF.md#2i-the-same-code-measured-again) has the query's.

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
cold path, where the verdict is still unattributed. **And spec 036 built the instrument that would** —
the **file-state floor**, the build side's read through the engine's own reader, measured in the
iteration right after the machine's own measure, so that the two are back to back in the same
iteration — **and the file's state did not move on the day the set was taken**: the verdict is still
**unattributed** on the cold path, and only the open's gap's yardstick widened, from **0.258 ms** to
**0.361 ms**. **So the question stands, and the instrument that would settle it is in place.** It is
written down as [`spec 036`](specs/036-the-file-state-floor/spec.md) and in
[ADR-0042](docs/adr/0042-the-file-state-floor.md).

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

- **The drift's own parts are taken apart, and the answer is that the yardstick is not a property of the
  code.** Spec 035 decomposed the drift into its parts — the **machine's own movement** (the floor),
  the **file's state** and the **engine's own variance** — and it closes exactly: the reconciliation is
  **0.000000 ms** on both paths and on both sets. **And which of them is largest is a property of the
  host's state on the day, not of the code.** On the busy host the published set was taken on, the cold
  path's movement is the file's state (**+1.179 ms**) and the build side's gap (**+1.054 ms**) against
  the machine's own **+0.205 ms** — and the build side's read's own series shows three excursions above
  22 ms in one campaign, against about **11.7 ms** in the others. On the quiet host the same read never
  breaks away — **10.316 ms**, **10.952 ms**, **10.657 ms**, every iteration within a millisecond — and
  the machine's own **+0.301 ms** is larger than the cold path's whole movement. **And the floor's
  validity flips with it**: sound on the resident path of one set, not on the cold path of the same set,
  and the other way round on the other set. **And how much of the engine's own cost moves under the
  measurement**: **18.72%** of the cold path's cost and **1.75%** of the resident path's in the
  published set, **6.76%** and **4.52%** in the increment's — so the engine's cost moves by about two
  per cent on the path that holds the data and by about a fifth on the path that reads the file.
  [§3q](PERF.md#3q-the-drifts-own-parts) has the ladder.
- **The file-state floor is built, and the sentence that named it is corrected.** Spec 036 moved both
  floors into the iteration: the **file's own read** — the build side's read through the engine's own
  reader, with the engine's own predicates — is now the **file-state floor**, measured right after the
  **machine's own measure**, and the two are **8.565 ms**, **8.871 ms** and **8.793 ms** apart in the
  three campaigns. **And the correction**: PERF.md §3q and ADR-0041 said the floor was measured *in the
  same iteration, back to back, with the order alternating*; the **sides** were, and the floor was
  measured in its own block and paired with the leaves by index, so the machine it took out was the
  machine's movement **in another block**. It is corrected in §3q, where it was published, and
  [ADR-0042](docs/adr/0042-the-file-state-floor.md) records it. **And the yardstick now sees the file's
  state as well as the machine's** — the machine's drift, the file-state floor's drift, and the larger of
  them: **0.258 ms** and **0.361 ms** on this day, and only the open's gap's yardstick changes, from
  **0.258 ms** to **0.361 ms**. **And the attribution is published as it came out**: the excursions did
  not recur — over **nine more campaigns** the file's own read's largest spread within a campaign was
  **1.805 ms** and the machine's own movement's was **1.842 ms** — so the published set's excursion at
  **38.640 ms** is not re-attributed, and what the increment makes true is that **the next time the
  file's state moves, the two will be measured apart, in the same iteration.**
  [§3r](PERF.md#3r-the-file-state-floor) has the tables.
- **The write side is measured, and the store's shape is decided by figures.** Spec 037 priced the half
  of the engine that no figure had ever measured. The write of the published file is **1844.195 ms**, one
  row group costs **306.230 ms**, and the **commit** — the data file's `fsync`, the manifest written and
  `fsync`ed, the `rename`, the directory's `fsync` — is **7.651 ms**, which is **0.004** of the write:
  **the protocol's shape is not constrained by the flush on this host**, so the store needs no write-ahead
  log and no rollback to be affordable. **And the dictionary stays on**: the same rows written plain take
  **1.55×** the bytes and read back **2.60×** slower. **And the batch size is a property of the bytes**:
  at **65,536** rows the probe's file **is** the published file, byte for byte, and at the engine's own
  **125,000** it differs **from byte 7** — the writer cuts its mini-batches, and the dictionary's fallback
  with them, at the caller's batch boundary. **And the write costs 3.36× a full read**, so *write once,
  read many* pays for itself after about four full reads.
  [§3s](PERF.md#3s-the-write-side) has the tables, and
  [ADR-0043](docs/adr/0043-the-write-side.md) records the decision.
- **The measurement track's question is answered, and the next increment is the store's own code.** What
  is left besides it is the work the repository cannot take on its own: **the library's publish** — it
  needs a crates.io account and a token, and it makes the code public — and the **`parquet` bump**, whose
  target is known and whose own measurement has to be taken because Arrow's decode is what the published
  figures measure. Both are written down below, and neither is a spec yet.
- **The three crates are ready to publish, and the publish is a separate act.** Spec 034 turned the
  research harness into a library: the surface is decided and written down (**45 names**, closed by the
  rule that every type in a public signature is on the list), the three crates package **in the order
  their dependencies decide**, each carries its own README and licence, the research modules are
  reachable and undocumented, the enums that will grow are `#[non_exhaustive]`, and the embedder test
  imports **only** the facade. **What is left is the publish itself**: it needs a crates.io account and a
  token, it makes the code public, and it is **not** an act this repository can take on its own. The
  three names are **free** — `columnar-core`, `columnar-kernels` and `columnar-engine` are all
  unregistered — and the order is **core, then kernels, then engine**, with the commands in
  [`docs/embedding.md`](docs/embedding.md). Until it is published, `cargo package` succeeds for
  `columnar-core` and **waits** for its sibling for the other two, and the check says which sibling each
  of them waits for.
- **The comparison campaigns were taken again, and the published set stands.** Spec 034 changed no engine
  work, so the set the file publishes keeps its name and its figures, and the increment's re-measurement
  is published **beside** it: the same code on this host, and the code the increment replaces, measured
  back to back, both trees clean. **The code's share of every difference is nil** — the join's cold stage
  differs by **−0.089 ms** against a drift of **1.237 ms** — and the movement between the published set
  and this one is the **host's**: the incumbents, which the increment cannot have touched, moved with it.
  [§2i](PERF.md#2i-the-same-code-measured-again) and [§3p](PERF.md#3p-the-joins-stage-measured-again) have
  the tables.
- **A hostile Parquet file can ask for four gigabytes before a byte of payload is read.** The reader
  speaks the Thrift **compact** protocol, whose `read_bytes` allocates the declared length eagerly,
  bounded by a u32 — so a file's metadata can declare a four-gigabyte string. The engine does not fix
  it: the fix is upstream (`thrift` ≥ 0.23), and `parquet` 54 pins `thrift = "0.17"`, so the fix is a
  **`parquet` bump** — and a `parquet` bump moves the published figures, because Arrow's decode is
  measured. **And the target is known, not guessed**: `parquet` **58.4.1** takes **`thrift` ^0.23**, and
  **59.0.0** and **60.0.0** drop `thrift` altogether — but every one of them takes `arrow` with it
  (**58.4.1**, **59.0.0**, **60.0.0**), and Arrow's decode is what is measured, so the bump is its own
  increment with its own figures. **It is named in `deny.toml` with the reason**, and it is written down
  as [`spec 034`](specs/034-the-engine-as-a-library/spec.md)'s R10 rather than left in the reader.
- **The x86-64 absolute figures** need a pinned x86-64 runner. The job is wired and has not run.
- **The CI runners are contended.** The runs sit queued for a long time, and the account's Actions
  spending limit applies to every job — when it is exhausted, *every* job fails at once with a
  billing message rather than a test failure. The published figures are therefore taken on this
  machine and not on CI, and every report carries the host's load average.
- **The official TPC-H SF-1 file.** The generator produces a TPC-H-*shaped* file with approximated
  distributions, UTF-8 dates and a seeded PRNG. The manifest says so, and the constitution names it
  as a boundary.
- **Window functions, decimal, dates, nested types, GPU.** Out of scope of spec 001, and still out.
  The hash join and the sort *were* in that list and are built now — spec 020.
