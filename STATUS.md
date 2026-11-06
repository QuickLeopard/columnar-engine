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
[`constitution.md`](constitution.md). **43 specs**, **57 ADRs** and **29 defects** so far, and every one
of the specs but the one in flight is closed. **And the status has a page**:
[`docs/status.html`](docs/status.html) is this file rendered as one screen — the headline, the tree's
shape, the gates, the increments and the open questions — and it is **generated** and **gated**, so it
cannot go stale.

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

The tree reads Parquet through the **`parquet` crate, 58.4.1**, and Arrow through **`arrow`,
58.4.1** — one line in the workspace's shared dependency table, so the version cannot drift between
crates. The bump from **54.3.1** is the increment spec 041 is, and it was measured before it was
published: [`PERF.md` §3x](PERF.md#3x-the-bump-and-the-figures-it-moved) and
[ADR-0050](docs/adr/0050-the-bump-and-the-exemption-that-named-nothing.md). The toolchain is
**Rust 1.97.1**.

---

## Where it stands

**Against DuckDB 2.0.0, Polars 1.36.1 and DataFusion 49.0.2, on the same files, at the same thread
count, with every answer cross-checked against the engine's own.** The headline tables are pinned to
**one thread** — the like-for-like single-threaded comparison. The thread-count sweeps are in
[`PERF.md` §2e–§2g](PERF.md#2-the-engine-against-duckdb-polars-and-datafusion).

### Cold — the file is read and the query answered in one go

| query | **engine** | duckdb | polars | datafusion | |
|---|---|---|---|---|---|
| `q6` | **14.9 ms** | 35.0 ms | 201.8 ms | 38.5 ms | the engine wins |
| `q6-revenue` | **59.1 ms** | 70.0 ms | 201.7 ms | 118.2 ms | the engine wins |
| `join-orders` | 100.0 ms | **94.0 ms** | 225.7 ms | 175.3 ms | DuckDB wins |
| `sort-topk` | **39.0 ms** | 39.0 ms | 1468.9 ms | 41.3 ms | level with DuckDB |
| `filter-range` | **15.8 ms** | 33.0 ms | 16.5 ms | 21.8 ms | the engine wins |

**Six of the eight cold queries in [`PERF.md`'s table](PERF.md#2b-cold--read-the-parquet-file-and-answer-in-one-go)
are the engine's**; the one it loses is named in the open questions below, and the sort is level with
DuckDB. The engine's eager path — all sixteen columns — takes **587.2 ms** for the same file, where
the set the bump replaces had **552.3 ms**, and is measured on its own in
[`PERF.md` §2g](PERF.md#2g-the-load-sweep--where-the-engines-biggest-number-was).

**What moved, and why.** The engine's cold path reads the file through this repository's Arrow, and
**the bump changed Arrow's decode**: the engine's `q6` moved from **15.4 ms** to **14.9 ms** and its
`filter-range` from **16.3 ms** to **15.8 ms**, which is the decode getting faster. **The incumbents do
not read the file through this repository's Arrow**, so **their movement is the host's** — DuckDB's `q6`
moved from **32.0 ms** to **35.0 ms**, Polars' from **190.5 ms** to **201.8 ms** and the peer's from
**34.8 ms** to **38.5 ms** — and they are the yardstick the host's movement is read against. **And the
host's movement is in the engine's own figures too**: its `join-orders` rose from **97.6 ms** to
**100.0 ms**, and the host did change between the sets — the OS moved from macOS 27.0.1 to 27.2.

### Resident — the data is already in memory

| query | **engine** | duckdb | polars | datafusion | |
|---|---|---|---|---|---|
| `q6` | **10.1 ms** | 48.0 ms | 75.8 ms | 25.5 ms | the engine wins |
| `join-orders` | **26.0 ms** | 40.0 ms | 90.6 ms | 91.1 ms | the engine wins |
| `sort-topk` | **3.1 ms** | 4.0 ms | 1564.0 ms | 4.5 ms | the engine wins |
| `filter-range` | **7.7 ms** | 20.0 ms | 13.3 ms | 12.1 ms | the engine wins |

**The engine wins every query in this table**, against a Rust peer on the same Arrow stack. The
asymmetry is named: the incumbents have both tables in memory before their clock starts, while the
engine's resident path **reads the build side inside the clock**.

**What moved, and why.** **The resident path never decodes a file, so the bump cannot be in it**: the
movement is the **host's** and the engine's own variance. The engine's `q6` moved from **9.4 ms** to
**10.1 ms** and its `filter-range` from **6.6 ms** to **7.7 ms**, while **the incumbents moved with
it** — Polars' `q6` rose from **67.4 ms** to **75.8 ms** and its `join-orders` from **78.9 ms** to
**90.6 ms**, DuckDB's `q6` from **45.0 ms** to **48.0 ms** and its `join-orders` from **35.0 ms** to
**40.0 ms** — and they are the yardstick.

### The load — the file into memory

| threads | **engine** | duckdb | polars | datafusion | |
|---|---|---|---|---|---|
| 1 | **546.8 ms** | 1228.0 ms | 1019.4 ms | 328.9 ms | ahead of DuckDB and Polars, behind the peer |
| 6 | **216.5 ms** | 223.0 ms | 190.3 ms | 119.7 ms | ahead of DuckDB, behind Polars and the peer |
| 14 | **211.3 ms** | 219.0 ms | 180.0 ms | 70.6 ms | ahead of DuckDB, behind Polars and the peer |

**This was the engine's worst figure** — once the largest number the repository had published — and
it is now ahead of DuckDB and Polars at one thread, and at six ahead of DuckDB and behind Polars.
The engine's worker count is capped at the file's six row groups, exactly `min(threads, row_groups)`,
and the peer reads the same file a different way.

**What moved, and why.** The whole table fell: DuckDB's one-thread load moved from **1275.0 ms** to
**1228.0 ms**, Polars' from **1069.8 ms** to **1019.4 ms** and the peer's from **362.3 ms** to
**328.9 ms**, and the engine's own from **576.8 ms** to **546.8 ms**. **The incumbents do not read the
file through this repository's Arrow, so their movement is the host's**; the engine's own figure reads
the file through it, so **the bump's decode is in the engine's figure as well**, and this table cannot
separate the two.

### The join's probe, taken apart

The probe is the join's hot loop: six million rows, one a probe row, against a table that is one
byte a key. The instrument measures it against its own **floor** — the table's own load, which no
lookup can go under — and against the same lookup written out by hand over the same rows.

| | ms |
|---|---|
| the engine's own probe, over the engine's column | 9.100 |
| the floor the probe cannot go under | 4.948 |
| the same lookup, written out by hand | 8.754 |
| **the remainder** — the engine's probe over the hand-written one | **0.436** |
| the join's stage, cold | 27.493 |
| the join's stage, resident | 20.276 |
| the build side's whole — the read, the keys and the table | 12.568 |
| the stage's residual — the stage's own probe plus the build side's own gap | 7.686 |
| the context — the residual against the standalone probe | −1.354 |

**The remainder this table publishes is 0.436 ms, with a spread of 0.943 ms** — inside the noise, and
**the lookup's shape is free** still. The figures moved against the set the bump replaces, and every
one of them is published old-and-new: the engine's own probe was **8.860 ms** and is **9.100 ms**, the
floor was **6.147 ms** and is **4.948 ms**, the hand-written lookup was **8.953 ms** and is **8.754 ms**,
the stage was **24.690 ms** cold and is **27.493 ms**, **19.431 ms** resident and is **20.276 ms**, the
build side's whole was **12.347 ms** and is **12.568 ms**, the residual was **7.187 ms** and is
**7.686 ms**, and the context was **−1.581 ms** and is **−1.354 ms**. **The engine's own code did not
change, and the probe never decodes a file — it probes a column in memory — so the bump cannot be in
it**: the difference is the **host's** and the engine's own variance, and the two sets were not taken
in the same session: this one ran on a host whose load average was **14.74–15.35**, the set the bump
replaces on one at **3.66–4.34**. The remainder was **3.337 ms** before spec 029, and it held two
artefacts of the instrument rather than the engine's cost — the crate boundary, and a warm buffer where
the query probes a column the reader has just written.

**And the verdicts, with the yardstick each was made with.** The context's verdict is published in the
house's own sentences, and the yardstick it was made with is published beside it:
the verdict, on the cold path, is **unattributed**;
the verdict's state, on the cold path, is **sound**;
the cold path's yardstick is **2.236 ms** — the probe's own remainder's own, the largest leaf on
that path;
the verdict, on the resident path, is **unattributed**;
the verdict's state, on the resident path, is **sound**;
the resident path's yardstick is **1.324 ms** — the probe's file state's own, the largest leaf on that
path.
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
movement between them and this set is the **host's** and the **bump's** — the incumbents moved with it,
and the old code measures the same as the new one.
[`PERF.md` §3p](PERF.md#3p-the-joins-stage-measured-again) has the table, and
[§2i](PERF.md#2i-the-same-code-measured-again) has the query's.

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

**1. The context — it is decomposed, and in this set the verdict is unattributed.** The join's stage's
residual is 7.686 ms on the resident path, and the standalone probe of the same work is 9.100 ms. The
gap is **−1.354 ms**: inside the query, the probe accounts for *less* than it does in a quiet
process. On the cold path the same gap is **1.382 ms** — positive where the resident one is negative
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
published and is the yardstick every part is judged against. The verdict is **unattributed** on both
paths, and both yardsticks are **sound**: the resident path's file state is **−1.208 ms** against a
yardstick of **1.324 ms**, and its own spread of **1.354 ms** is wider than that yardstick, so no
mechanism can be claimed there. **And that is the host's, not the bump's** — the probe never decodes a
file: where the set the bump replaces had the file state at **−1.238 ms** against a yardstick of
**0.487 ms** and a spread of **0.390 ms**, outside it, the verdict was **the memory's**.
The context's own spread moved with it — **0.418 ms** to **0.356 ms** to **0.184 ms** on the resident
path and **2.211 ms** to **0.667 ms** to **2.271 ms** on the cold one — and it is published old-and-new
with the reason: **the host's**, and the sets were not taken in the same session.
**What would answer it:** whether the file state is the whole of it — the same mechanism measured on the
cold path, where the verdict is still unattributed. **And spec 036 built the instrument that would** —
the **file-state floor**, the build side's read through the engine's own reader, measured in the
iteration right after the machine's own measure, so that the two are back to back in the same
iteration — **and the file's state did not move on the day the set was taken**: the verdict is still
**unattributed** on the cold path, and only the open's gap's yardstick widened, from **0.258 ms** to
**0.361 ms**. **So the question stands, and the instrument that would settle it is in place.** It is
written down as [`spec 036`](specs/036-the-file-state-floor/spec.md) and in
[ADR-0042](docs/adr/0042-the-file-state-floor.md).

**2. `filter-range`, cold: the two spreads overlap.** The engine's lazy path is **15.8 ms** against
Polars' **16.5 ms**, where the set the bump replaces had **16.3 ms** against **15.2 ms** — and the
engine's own spread is **15.1–19.2 ms** against Polars' **16.4–16.7 ms**, so the ordering is inside the
noise. The scan that holds it is the Parquet decode, and the decode has been measured three times and
refused every time: owning the decode buys nothing, and neither does *not* expanding the predicate's
column — the expansion of six million RLE indices costs **1.77 ms** where the set the bump replaces had
**1.79 ms**, and the mask the alternative would build costs **1.75 ms** where it had **1.73 ms**, so the
hypothesis never pays for itself. **And the bump is in the decode, not in the expansion**: Arrow's own
decode of the column fell from **4.13 ms** to **3.34 ms**, while the expansion's own cost moved with the
host. **What would answer it:** a file with a page index, or a reader that owns the Parquet decode.
Neither is in scope of what the project has done so far.

**3. `join-orders`, cold: DuckDB is ahead.** The engine is **100.0 ms** against DuckDB's **94.0 ms**,
where the set the bump replaces had **97.6 ms** against **91.0 ms**: the gap is **0.6 ms** smaller than
it was, and the whole of that is DuckDB's own movement. The published analysis says the gap is
**DuckDB's own figure that moved**: DuckDB's cold join fell from 95.0 ms while the engine's rose from
96.7 ms, and the engine's *old* code, re-measured on this host in the same session, reads 97.3 ms
against the same DuckDB's 92.0 ms — so the gap is there for the code spec 026 shipped as well. The
engine's stage was taken apart in spec 026 and the answer was that it is the **build side's read** —
11.716 ms of the 21.731 ms cold stage — and Arrow's decode of `orders.parquet`, which has been
measured three times and refused every time. **What would answer it:** the parts, not the total.

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
  of the engine that no figure had ever measured. The write of the published file is **1679.362 ms**, one
  row group costs **279.785 ms**, and the **commit** — the data file's `fsync`, the manifest written and
  `fsync`ed, the `rename`, the directory's `fsync` — is **8.493 ms**, which is **0.005** of the write:
  **the protocol's shape is not constrained by the flush on this host**, so the store needs no write-ahead
  log and no rollback to be affordable. **And the dictionary stays on**: the same rows written plain take
  **1.55×** the bytes and read back **2.47×** slower. **And the batch size is a property of the bytes**:
  at **65,536** rows the probe's file **is** the published file, byte for byte, and at the engine's own
  **125,000** it differs **from byte 7** — the writer cuts its mini-batches, and the dictionary's fallback
  with them, at the caller's batch boundary. **And the write costs 3.22× a full read**, so *write once,
  read many* pays for itself after about four full reads. **And what moved here is the host's**: the write
  of the published file was **1844.195 ms** and is **1679.362 ms**, the row group **306.230 ms** and is
  **279.785 ms**, the commit **7.651 ms** and is **8.493 ms** — **0.004** of the write and is **0.005** —
  and the plain encoding read back **2.60×** slower and is **2.47×**.
  [§3s](PERF.md#3s-the-write-side) has the tables, and
  [ADR-0043](docs/adr/0043-the-write-side.md) records the decision.
- **The store's half is built, and it writes the published file byte for byte.** Spec 038 built
  `columnar-store`: a **writer**, a **manifest**, a **commit** and a **catalog**. A table is a **set of
  files** — a commit appends one and never modifies an old one — and the **manifest** is the table's
  truth. The commit is the **five steps** spec 037 measured: the data file durable, the manifest written,
  durable and renamed over the old one, the directory durable. **And there is no write-ahead log**: a
  commit is a rename, and the rename is the answer. **And the batch size is a rule now, not a surprise**:
  the grid is **65,536**, the store **checks** it, and a batch off the grid is refused with an error —
  because a file that comes out differently is a caller that batched differently, and the store would rather
  say so than write a file whose bytes nobody can predict. **And the acceptance is the strongest one
  available**: fed the published file's own rows, the store wrote **242,859,461** bytes — **the published
  file, byte for byte** — in every campaign. **And the published file's own bytes moved with the bump**:
  it was **242,857,841** bytes and is **242,859,461**, the writer's output changed with Arrow, and the store
  writes the file that is published now, byte for byte, as before. [§3t](PERF.md#3t-the-stores-own-write) has
  the tables, and [ADR-0044](docs/adr/0044-the-stores-half.md) records the decision.
- **The file boundary's cost is measured, and it is free.** The store writes a table as **many** files and
  the engine's only source is **one**, so what the boundary costs had to be measured before the engine was
  taught to read the set. **The experiment was the published file's own six row groups** -- the same rows as
  the published file -- written as six files and read **both ways**, through the engine's own reader, in the
  same campaign: **70.151 ms** for the one file and **68.267 ms** for the six, and the difference is
  **−1.416 ms**, inside the one file's own spread. **And what moved here is the bump's and the host's** —
  the engine reads the file through this repository's Arrow, and the host moved with it: the one file was
  **66.003 ms** and is **70.151 ms**, the six **64.114 ms** and is **68.267 ms**, and the difference
  **−1.152 ms** and is **−1.416 ms**. **So the file boundary costs nothing measurable**, and
  the rows are the same -- compared **row by row, in order**, on the key column. **And so the source's case
  is correctness, not performance**: the naive way to read a store's table is **wrong** for a filter, an
  aggregate and a join, and a source that reads the set as one table is what makes it right -- so it has to
  be **correct, and free**.
  [§3u](PERF.md#3u-the-file-boundarys-cost) has the tables, and
  [ADR-0045](docs/adr/0045-the-file-boundarys-cost.md) records the decision.
- **The engine's side of the glue is built, and the source is free.** Spec 040 gave the engine a source
  over a **set** of files: `FileSetSource` is **one new name** on the surface, `execute_on` takes a
  `ParquetSource` exactly as it did, and the set reads the files **as one table** — the metadata merged,
  the statistics the table's, the predicate reaching **every** file, the rows in the files' order. **And
  it is free**: the set's read is **−0.991 ms** against the one file's — negative, and inside the one file's
  own spread — and in parallel **−0.019 ms** against **35.879 ms**, a share of **−0.001**. **And the set
  the bump replaces had it at **−0.599 ms** against **35.123 ms**, a share of **0.004** — the same cause
  as the boundary's: the bump's decode and the host's movement.** **And the way it
  is free is the way that matters**: the files' readers are **chained** and decoded in **one pass**, so the
  rows are copied **once**; the first implementation merged the files' chunks instead, and the second copy
  was worth **8.747 ms**, **13%** of the read. **And the answers are the engine's**: the set's rows are the
  one file's, compared **row by row, in order**, and the same filtered plan counts the same **3,184,637**
  rows over the file and over the set. [§3v](PERF.md#3v-the-source-over-a-set-of-files) has the tables, and
  [ADR-0046](docs/adr/0046-the-source-over-a-set-of-files.md) records the decision.
- **A published sentence was wrong about this engine, and it is corrected where it was published.** Specs
  039 and 040 said the pushdown **prunes row groups by the file's statistics**, and that a row group which
  cannot survive is not read. **The engine does neither**: ADR-0010 accepted that cost — "the engine does
  not prune, and does not pretend to" — and the statistics are used in two places, and **both of them are
  estimates, not reads**. The requirement is now what was true of this engine all along: the statistics a
  set publishes are the **table's** — the range is the **union** of the files' ranges — and the engine's own
  predicate reaches **every** file. It is corrected in both specs and in ADR-0045, and ADR-0046 records it.
  **The measurement is not touched by the correction.**
- **The engine's rows reach a store's table, and the file comes out the same.** The store's claim was
  measured through Arrow alone; spec 042 measured it through the **engine's** rows, which are the engine's
  own vocabulary. **The conversion is exact**: the store's conversion, under the published file's own
  schema, writes the published file **byte for byte** — 242,859,461 bytes, no differing byte — so the
  engine's rows are the file's rows. **And the one thing the rows cannot carry is a column's
  nullability**: the engine says which rows are NULL and not whether a column may be, and the Parquet
  writer makes that difference visible in the bytes. So the store's rule is stated rather than
  discovered — **a column that holds no NULL is written REQUIRED** — and the file it writes from the
  engine's rows is 242,854,029 bytes, first difference at 20,566,725, and **every byte of the difference
  is the schema**. The conversion costs **149.809 ms** against a **1,826.588 ms** write. The engine gained
  the way to hand a plan's rows out (`execute_rows`, `execute_rows_on`), and **a plan with an aggregation
  is refused**, because the aggregate folds the rows and there is nothing left to write. The **grid is
  the store's** and the engine does not learn it.
  [ADR-0054](docs/adr/0054-the-engines-rows-reach-a-stores-table.md) records it.
- **The set's rows are handed out, and the round trip is the file, byte for byte.** `FileSetSource::execute_rows` is
  the set's own door to its rows — the same path `execute_rows_on` takes for one file — so **a store's table
  can now be read by the engine and written back**, and the glue is three lines: the paths go to the set, the
  set's rows come out, the store writes them. **The identity is exact**: the same **6,000,000** rows come out
  of one file and out of six, and **the two files the two round trips write are the same file, byte for
  byte**. **The set costs nothing that can be claimed** — the rows out are **+1.133 ms** against the one file's
  *(was +6.425 ms)* and the write **−24.333 ms** *(was +34.659 ms)*, and the differences between the arms in
  every iteration — measured back to back, with the order alternating — straddle zero with a spread wider
  than either, on a host whose load average was 5.88, 6.69 and 7.22 — and the estimate does not move either:
  **1,549,500,016 bytes** for both. **And the round trip is the published file, byte for byte**: under the
  file's own schema it writes **242,859,461** bytes and **no differing byte** — the Parquet schema is the
  same and the Arrow schema is the same, because the rows keep the dictionary — while **the store's own rule
  still differs**, at byte **20,566,725**, because a column that holds no NULL is written REQUIRED. **No
  public break**: the surface does not move, so the crates can be published with this in them.
  [ADR-0055](docs/adr/0055-the-sets-rows-are-handed-out.md) and
  [ADR-0056](docs/adr/0056-the-rows-are-handed-out-as-they-lie.md) record it.
- **The status is a page now, and the page cannot go stale.** `docs/status.html` is the project's status
  as one screen: the headline comparisons, the tree's shape, the gates, the increments and the open
  questions. **It is generated** — `scripts/build-dashboard.py` reads `STATUS.md`, `PERF.md` and the
  specs and writes it — **so it holds no figure of its own**: every number on it is rendered from the
  document that publishes it, the way the figures guard already insists for the prose. **And it cannot
  go stale**: the same script with `--check` rebuilds the page and fails when it differs, so a change
  that moves the status and does not rebuild the page does not merge. ADR-0057 records it.
- **The rows path does not copy the table any more, and the round trip is the file.** The rows the engine
  hands out are **the rows that were read** when no row was removed and nothing reordered them: the engine's
  own chunks, with their own encodings, and **the gather is gone for a plan that filters nothing**. **The lazy
  path moves the columns out of the reader** — no copy at all — and **the eager path copies them**, because
  the caller owns the dataset there. **The rows out are six times faster**: **549.167 ms** against
  **3,338.468 ms**, and the copy that is left is **eighteen times smaller**: **154.858 ms** against
  **2,823.164 ms**. **And the shape is the plan's**: the rows come out with **7 dictionary columns** and
  **9 plain ones**, the encoding says which shape the caller got, and the store takes both. **The gather
  stays for every plan that needs it** — a filter that removed rows in the engine, a join and a sort — and
  §3z's plan's rows are gathered still. [ADR-0056](docs/adr/0056-the-rows-are-handed-out-as-they-lie.md)
  records it.
- **A published figure was wrong, and it is corrected where it was published.** §3aa published the gather as
  **2,292.710 ms** — the difference between the eager rows and the eager read — and as **81%** of what the
  eager path spends handing the rows out. **But the eager rows measurement did not include the read**: the
  probe measures the decode and the rows **separately**, so the gather was the whole of the eager rows,
  **2,823.164 ms**, and **84.2%** of the eager path's **3,353.618 ms**. **The claim was right** — the gather
  was most of the cost, and on a plan that filters nothing the copy bought nothing — **and the figures were
  wrong**. ADR-0056 records the correction, and the guard checks the figure it quotes against the **before**
  reports, so the correction quotes the reports and not a memory of them.
- **The store's table can be on the build side of a join, and on the probe side.** `JoinSpec.build_paths`
  is a list of paths: one path is one file, exactly as it was, and several are read as one table — so the
  build side is now the **source's** shape and not the plan's, and it is named rather than left to be
  discovered. **The change to the plan's own shape is one public break**, and it was made while the crates
  were still unpublished, so it cost nothing. [ADR-0053](docs/adr/0053-the-build-side-takes-a-set.md)
  records it.
- **The store's crate is publishable, and the publish is the user's.** The engine can now read the store's
  tables, and `Table::paths` hands it the list of paths it takes, so the store's own surface has its shape.
  **And the crate could not even be packaged** until now: its dev-dependency on the generator carried a
  version, and the generator is not on crates.io, so `cargo package` failed with *"no matching package named
  `columnar-testdata` found"* — a **path-only** dependency is dropped from the published manifest and a
  versioned one is not. **And the two reasons ADR-0044 gave for holding the store back were wrong**: the
  store does **not** depend on the core (it depended on nothing in this workspace then), and the increment
  that was named is done. **The publish order is `columnar-core`, `columnar-kernels`, `columnar-store`,
  `columnar-engine`** — the graph decides what it can and the list breaks the rest — and
  `cargo publish --dry-run -p columnar-store` passes. **The publish itself needs the account and the
  token**, which the repository does not have.
- **The composition is a test, and the edge it needs is test-only.** The store's table, read by the
  engine, as one table — that claim was checked **by hand**, and the hand check was **wrong the first
  time**: it compared the set against one file, and the set holds both, so the counts differ by two.
  The code was right and the assertion was wrong, and the wrong assertion was the only place the
  composition was ever checked. **So it is in the suite now**: the store's own tests write two
  commits, take the table's paths, open the set with the engine's source, and say that the set holds
  the table's rows **row by row, in the files' order**, that the engine's predicate reaches **every**
  file, and that the **sum** over the set is the files' sums added up. **And the tests have teeth**:
  with the set reading one file instead of the table, all three fail. **And the edge the test needs
  is a dev-dependency, path-only** — so the store's *library* depends on the core and on nothing else in
  this workspace, the engine still never depends on the store, and the published manifest says so.
  [ADR-0048](docs/adr/0048-the-composition-is-a-test.md) records it.
- **The links are a gate now.** A link in a document is a claim about the tree, and **no other gate
  reads a sentence's target** — the figures guard checks numbers, the page guard checks the browser's
  promises — so a link could rot in place for increments before anyone looked, and **eighteen did**:
  the ADRs' links to their specs, the older specs' links to each other, and PERF.md's link to STATUS.md.
  They are repaired where they were published, and **`scripts/check-links.py` fails the build** when a
  document's link does not resolve — a link to another host is left alone, because it is a claim about
  another host.
- **The four crates are ready to publish, and the publish is a separate act.** Spec 034 turned the
  research harness into a library: the surface is decided and written down (**46 names**, closed by the
  rule that every type in a public signature is on the list), the crates package **in the order
  their dependencies decide**, each carries its own README and licence, the research modules are
  reachable and undocumented, the enums that will grow are `#[non_exhaustive]`, and the embedder test
  imports **only** the facade. **What is left is the publish itself**: it needs a crates.io account and a
  token, it makes the code public, and it is **not** an act this repository can take on its own. The
  four names are **free** — `columnar-core`, `columnar-kernels`, `columnar-store` and `columnar-engine`
  are all unregistered — and the order is **core, then kernels, then store, then engine**, with the
  commands in [`docs/embedding.md`](docs/embedding.md). Until it is published, `cargo package` succeeds
  for `columnar-core` and for the store, and **waits** for its sibling for the other two, and the check
  says which sibling each of them waits for.
- **The comparison campaigns were taken again, and the published set stands.** Spec 034 changed no engine
  work, so the set the file publishes keeps its name and its figures, and the increment's re-measurement
  is published **beside** it: the same code on this host, and the code the increment replaces, measured
  back to back, both trees clean. **The code's share of every difference is nil** — the join's cold stage
  differs by **−0.089 ms** against a drift of **1.237 ms** — and the movement between the published set
  and this one is the **host's** and the **bump's**: the incumbents, which the increment cannot have
  touched, moved with it.
  [§2i](PERF.md#2i-the-same-code-measured-again) and [§3p](PERF.md#3p-the-joins-stage-measured-again) have
  the tables.
- **A hostile Parquet file could ask for four gigabytes before a byte of payload was read — and the
  sentence that said so was wrong about where, and wrong about the fix.** The reader speaks the Thrift
  **compact** protocol, whose `read_bytes` allocates the declared length eagerly, bounded by a u32.
  **And the measurement says the hole is in the file's page headers, not in its metadata**: the
  footer is read through the reader's own *slice* protocol, which refuses a declared length larger
  than the bytes that remain **before** it allocates — so a hostile footer asks for **8 KiB**, the
  same as an honest file's — while the **page headers**, read one after another as the reader walks a
  row group, go through the *streaming* protocol, and there the declared length is allocated before a
  byte of it is read. **The probe writes a real file with one hand-written page header and reads it
  through the engine's own reader**: before the bump, the largest single allocation the reader asked
  for was **the declared length, exactly** — 1,048,576 at 1 MiB, 67,108,864 at 64 MiB, 1,073,741,824 at
  1 GiB and **4,294,967,295 at 4 GiB**, a share of **1.0** at every length, from a **25-byte** header —
  and **the cost was address space, not time**: the read took **0.089 ms**, less than the honest file's
  own **0.300 ms**. **And on the tree this file ships, the reader asks for what an honest file costs**:
  **8,192** bytes at every declared length — a share of **0.0078125** at 1 MiB, **0.0001221** at
  64 MiB, **0.0000076** at 1 GiB and **0.0000019** at 4 GiB — and the 4 GiB read takes **0.044 ms**,
  less than the honest file's own **0.095 ms**. **And the fix the sentence named would not have closed
  it**: `thrift` ≥ 0.23 refuses an over-long length only if the caller asks it to, the default asks for
  nothing, and `parquet` asks for nothing either. **What closes it is `parquet`'s own reader, from
  58.4.1**, which does not read the page statistics by default — so the declared length is never parsed
  at all. **The
  sentence is corrected where it was published** — in [`spec 034`](specs/034-the-engine-as-a-library/spec.md)'s
  R10, here and in `deny.toml` — and [ADR-0049](docs/adr/0049-the-readers-four-gigabytes.md) records
  the correction. The figure is in [§3w](PERF.md#3w-the-readers-four-gigabytes).
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
