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
[`constitution.md`](constitution.md). **49 specs**, **63 ADRs** and **29 defects** so far, and every one
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
| `q6` | **16.6 ms** | 32.0 ms | 190.0 ms | 35.7 ms | the engine wins |
| `q6-revenue` | **52.9 ms** | 67.0 ms | 193.3 ms | 111.5 ms | the engine wins |
| `join-orders` | **87.8 ms** | 89.0 ms | 216.6 ms | 163.1 ms | the engine is ahead on the median |
| `sort-topk` | 39.2 ms | **37.0 ms** | 1279.5 ms | 37.0 ms | DuckDB wins |
| `filter-range` | **14.6 ms** | 30.0 ms | 14.9 ms | 19.6 ms | the engine wins |

**Six of the eight cold queries in [`PERF.md`'s table](PERF.md#2b-cold--read-the-parquet-file-and-answer-in-one-go)
are the engine's**; the one it loses is named in the open questions below, and the sort is now DuckDB's.
The engine's eager path — all sixteen columns — takes **528.1 ms** for the same file, where the set this
increment replaces had **587.2 ms**, and is measured on its own in
[`PERF.md` §2g](PERF.md#2g-the-load-sweep--where-the-engines-biggest-number-was).

**What moved, and why.** **The default's rule changed, and the campaign was taken again because of it.** The
reader's default batch used to be a **row limit** — 125,000 rows whatever the read fetched — and it is **a byte
budget of 16 MiB** now, divided by what a row costs for the columns the read fetches, so the batch is
**width-dependent** ([§3ah](PERF.md#3ah-the-default-moved)). **And the old code was measured back to back with
the new one, three campaigns each, interleaved**, so the difference between the two is **the code's** and not the
host's. **And two of the five cold queries got slower.** The engine's cold `q6` is **16.6 ms** where the old
code's was **13.9 ms** — **2.7 ms slower** — and its `sort-topk` is **39.2 ms** where the old code's was
**37.0 ms**. **The reason is named**: the rule gives these shapes **1,000,000 rows** at one worker, and the
`q6`'s predicate set's scan curve **turns up at a whole row group** — its minimum is at **250,000 rows**, which
is what the rule gives at **four** workers — and the sort's stage is **2.34 ms** where it was **1.52 ms**. **The
other three did not move**: the `q6-revenue` is **52.9 ms** against **54.1 ms**, the `join-orders` **87.77 ms**
against **87.13 ms**, and the `filter-range` **14.64 ms** against **14.29 ms**. **The incumbents do not read the
file through this repository's Arrow**, so their movement is the host's and not the code's: DuckDB's `q6` is
**32.0 ms** in both sets, where the set this file published before had **35.0 ms**, Polars' **190.04 ms**
against **189.45 ms**, and the peer's **35.70 ms** against **35.15 ms**.

### Resident — the data is already in memory

| query | **engine** | duckdb | polars | datafusion | |
|---|---|---|---|---|---|
| `q6` | **9.3 ms** | 43.0 ms | 67.5 ms | 23.0 ms | the engine wins |
| `join-orders` | **24.0 ms** | 34.0 ms | 73.2 ms | 81.1 ms | the engine wins |
| `sort-topk` | **2.6 ms** | 3.0 ms | 1254.6 ms | 3.7 ms | the engine wins |
| `filter-range` | **6.4 ms** | 19.0 ms | 10.0 ms | 9.9 ms | the engine wins |

**The engine wins every query in this table**, against a Rust peer on the same Arrow stack. The
asymmetry is named: the incumbents have both tables in memory before their clock starts, while the
engine's resident path **reads the build side inside the clock**.

**What moved, and why.** **The resident path never decodes a file, so the default's rule cannot be in it**:
the movement is the **host's** and the engine's own variance. **And the interleaved set says the movement is
inside the noise**: the engine's `q6` is **9.33 ms** where the old code's was **9.06 ms**, and DuckDB's is
**43.0 ms** where it was **44.0 ms**, and Polars' **67.48 ms** against **67.63 ms**. **Against the older
published set the movement looks larger** — the engine's `q6` was **10.1 ms** there, and Polars' `join-orders`
**90.6 ms** — and that gap is the **host's**, which is why the incumbents are the yardstick.

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
| the engine's own probe, over the engine's column | 8.408 |
| the floor the probe cannot go under | 4.735 |
| the same lookup, written out by hand | 8.576 |
| **the remainder** — the engine's probe over the hand-written one | **−0.211** |
| the join's stage, cold | 20.863 |
| the join's stage, resident | 19.350 |
| the build side's whole — the read, the keys and the table | 11.881 |
| the stage's residual — the stage's own probe plus the build side's own gap | 7.393 |
| the context — the residual against the standalone probe | −0.928 |

**The remainder this table publishes is −0.211 ms, with a spread of 0.661 ms** — the noise covers zero, so
**the lookup's shape is free** still, and on the median **the engine's probe is the faster of the two**.
**And the figures moved, and every one of them is published old-and-new**: the engine's own probe was
**9.100 ms** and is **8.408 ms**, the floor was **4.948 ms** and is **4.735 ms**, the hand-written lookup was
**8.754 ms** and is **8.576 ms**, the stage was **27.493 ms** cold and is **20.863 ms**, **20.276 ms** resident
and is **19.350 ms**, the build side's whole was **12.568 ms** and is **11.881 ms**, the residual was
**7.686 ms** and is **7.393 ms**, and the context was **−1.354 ms** and is **−0.928 ms**. **And the default's
rule cannot be in it**: the probe never decodes a file — it probes a column in memory — and **the old code and
the new one measure the same thing**: the engine's own probe is **8.466 ms** in the old code's set and
**8.408 ms** in the new one, the floor **4.662 ms** against **4.735 ms**, and the remainder **−0.062 ms**
against **−0.211 ms**. The difference is the **host's** and the engine's own variance, and the two sets were
taken **in the same session**, interleaved. The remainder was **3.337 ms** before spec 029, and it held two
artefacts of the instrument rather than the engine's cost — the crate boundary, and a warm buffer where
the query probes a column the reader has just written.

**And the verdicts, with the yardstick each was made with.** The context's verdict is published in the
house's own sentences, and the yardstick it was made with is published beside it:
the verdict, on the cold path, is **unattributed**;
the verdict's state, on the cold path, is **sound**;
the cold path's yardstick is **0.495 ms** — the probe's own remainder's own, the largest leaf on
that path, where the set this increment replaces had **2.236 ms**;
the verdict, on the resident path, is **unattributed**;
the verdict's state, on the resident path, is **sound**;
the resident path's yardstick is **1.502 ms** — the probe's file state's own, the largest leaf on that
path, where the set this increment replaces had **1.324 ms**.
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
owning the Parquet decode, expanding the predicate's column, a bitmap in front of the table, and a
filter over the build side's keys — are refusals with the numbers that refused them, published as the
finding.

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
residual is 7.393 ms on the resident path, and the standalone probe of the same work is 8.408 ms. The
gap is **−0.928 ms**: inside the query, the probe accounts for *less* than it does in a quiet
process. On the cold path the same gap is **0.152 ms** — positive where the resident one is negative
— and it was **0.149 ms** on the old code, measured on this host back to back, and **−0.157 ms** on the
code this increment replaces, measured in the same session as this one.

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

**2. `filter-range`, cold: the two spreads overlap.** The engine's lazy path is **14.6 ms** against
Polars' **14.9 ms**, where the set this increment replaces had **15.8 ms** against **16.5 ms** — and the
engine's own spread is **14.36–14.78 ms** against Polars' **14.22–15.29 ms**, so the ordering is inside
the noise. **And the old code, measured in the same session, is the same shape**: the engine's
**14.29 ms** against Polars' **14.82 ms**. The scan that holds it is the Parquet decode, and the decode has been measured three times and
refused every time: owning the decode buys nothing, and neither does *not* expanding the predicate's
column — the expansion of six million RLE indices costs **1.77 ms** where the set the bump replaces had
**1.79 ms**, and the mask the alternative would build costs **1.75 ms** where it had **1.73 ms**, so the
hypothesis never pays for itself. **And the bump is in the decode, not in the expansion**: Arrow's own
decode of the column is **4.26 ms** now, where the set this increment replaces had **4.21 ms**, while the
expansion's own cost moved with the
host. **What would answer it:** a file with a page index, or a reader that owns the Parquet decode.
Neither is in scope of what the project has done so far.

**3. `join-orders`, cold: the parts are taken apart, and the engine's lookup is the faster one.** The
engine is **87.8 ms** against DuckDB's **89.0 ms** — **ahead on the median** — where the set this
increment replaces had the engine at **100.0 ms** against DuckDB's **94.0 ms**. **And the older sets are
published beside them**: the set spec 026 published had the engine at **96.7 ms** against DuckDB's
**95.0 ms**, and the code spec 021 replaced, measured on this host back to back, reads **97.3 ms**
against the same DuckDB's **92.0 ms**. **So the gap has been the host's**, and it closed on this host:
the engine's *old* code, re-measured in the same session as this one, reads **87.13 ms** against
DuckDB's **89.00 ms**. **Spec 046 took both
sides apart, in one session**, and the answer is not the one the file expected. The engine's **join** is
**14.625 ms** and DuckDB's is **7.706 ms** — the engine's join is **+6.898 ms** slower, and it is slower
in **every one of the eighteen iterations** — **and DuckDB's own join, over the same six million rows,
costs +51.422 ms more than its own filtered join and +44.190 ms more than the engine's.** DuckDB joins
**216,783** rows; the engine joins **6,000,000**. **The engine's lookup is the faster of the two, four
times over**, and the engine is behind because it looks up every row where DuckDB tests a bit. The filter
is worth **+45.697 ms** to DuckDB: it costs its scan **5.827 ms** and saves its join **51.422 ms**. **And
the rest of the engine's disadvantage is in the reads** — the build side's read **+0.862 ms**, the
projection **+0.452 ms**, the probe side's read at the noise — **and DuckDB's planning gives back 6.860
ms** that the engine's whole does not pay, which is why this host's paired gap is **+3.290 ms** — and the
whole is at the noise, with the engine ahead in **three** of the eighteen iterations. **The decomposition
closes to 0.0000 ms on every iteration.** **What would answer the next question:** what a filter over
the build side's keys costs the engine's scan and what it saves its join — DuckDB's own filter costs
**5.827 ms** over six million rows, and the engine's prize is the whole of its **14.625 ms** join less
what its filter would cost.

**Spec 047 measured that, and the answer is that the filter does not pay.** The ladder is the house's,
every rung is measured **in the iteration**, and the candidates are the engine's own probe **line for
line** — the engine's own probe is measured beside them as the control, **7.622 ms** against the
candidate's **7.967 ms** — and the probe side's column is **materialised afresh in every iteration**,
because a column held warm would not evict the table and would not be the shape the filter's case
depends on. **The prize is real and it is +6.981 ms**: the probe over the survivors alone costs
**0.906 ms** against the probe as it is at **7.967 ms**. **And it is unreachable, because the engine's
lookup is already the cheapest membership test there is.** The filter's own load over the same six
million rows is **5.222 ms** and the table's own load is **4.399 ms** — **both are one random access,
and the bitmap's test carries a shift and a mask on top of the access the byte does not.** The
direct-addressed table's byte **is** the membership test: the key's own range addresses it and it
consults nothing, so a filter can only be consulted **in addition** to it, and what it is consulted in
addition to is cheaper than it is. **The filter in front of the table is +2.318 ms — a loss — and the
filter compacting the rows, which keeps the rejected rows out of the probe's own work as well, is
+0.547 ms: a wash, with five of its twelve paired samples negative.** **No engine work changed**: the
filter is not built, the estimate does not move, and the engine's diff in the increment is empty.
**And the same test is worth seven times more in DuckDB** — its filter costs its scan
**5.827 ms** and saves its join **51.422 ms**, where the prize here is **6.981 ms** — **because it
replaces a hash join, and the engine has no hash join to pay for.** **What would answer the next question:** the engine's join part is
**14.625 ms**, its probe over six million rows is **7.967 ms** and its table's construction is
**0.672 ms** — **a filter cannot make the lookup cheaper, because the lookup is the membership test** —
**so the join's other work has to be found and named.** That is the next increment, and this one does
not answer it.

**Spec 048 found the join's other work, and it is the reader's context.** The engine's probe is the same
`probe_rows` in the two paths it runs in — the **ladder's path**, over a materialised column outside the
reader, and the **query's path**, inside the reader — and measured **in one session, back to back,
with the order alternated**, the query's path costs **+4.273 ms** more than the ladder's, in **24 of 24**
paired samples. **And the reader's batch size is what sets it.** In the set this increment replaces the
engine's own batch size was **125,000** rows, and a batch of **1,000,000** rows — one row group — was
**−4.926 ms** against it, faster in **24 of 24** paired samples; the curve was monotone, and the point in
between was **−1.438 ms** at **250,000** rows. **The query paid it too**:
the engine's join part is **15.823 ms** at the engine's own batch size and **9.641 ms** at a row group's
worth, and the paired difference against `DuckDB`'s join falls from **+8.083 ms** to **+1.898 ms** —
**with the scan flat, so the reader does the same work at every batch size.** **And the probe's cost is
the build table's residency**, which was worth **0.657 ms** and is measured: with the key column alone the
gap was **+1.834 ms**, so the value column's decode is more than half of the charge. **And the knob is a
knob**: `ParquetSource::with_reader_batch_rows` names a size in rows, the default did not move, and no
engine behaviour changed.

**And spec 051 moved the default, and the curve's own answer is now the default's.** The engine's own
batch size is **1,000,000** rows now — a whole row group — and **that is the minimum of the curve**: the
query's path costs **−0.159 ms** against the ladder's, where the set this increment replaces had
**+4.273 ms**, and the points below it are **+0.629 ms** at **250,000** rows, **+0.652 ms** at
**125,000** rows and **+0.816 ms** at **62,500** — where that set read **−1.438 ms** at **250,000**
and **+0.397 ms** at **62,500**, against the engine's own **125,000** rows. **And the old code, measured in the same session, sits at **+0.740 ms** with its own
batch of **125,000** rows** — so the movement in the probe's own figure is the code's, and it is the
code's improvement. **And the probe's cost is the build table's residency**, which is worth
**0.170 ms** now and was **0.657 ms**, and with the key column alone the gap is **−0.806 ms** where
it was **+1.834 ms**. **And the knob is still a knob**: `ParquetSource::with_reader_batch_rows` names
a size in rows, and it is still the measurement seam.

**And spec 049 priced the knob, and the price is what the rule has to answer.** A **counting global
allocator** measured the **peak** a read holds at three widths: the two predicate columns, the two
materialised ones, and the file's sixteen. **The bytes a batch holds scale with the width** — a whole row
group is **12.8 MiB** on a narrow query and **165.8 MiB** on a wide one — **and the knob is named in rows,
so it cannot see that.** **And the engine's own memory estimate did not model the batch at all**: it said
**1,500,016** bytes where the read holds **13,719,490**, and it said **1,549,500,016** where the read
holds **1,157,291,375** — and the **default budget refused the wide shape at every batch size**, because
the estimate did not depend on the batch size at all. **So the rule
is a byte budget, and the budget is 16 MiB** — bounded by the measurement from both sides, and it gives
every shape at least what it has today. **And the default did not move yet**: moving it moves **every**
published figure the batch size touches, and that was the next increment's work.

**And spec 050 gave the engine one honest statement of what a row costs, and made the estimate charge
the batch.** The statement is **8** bytes a row for a number, **4** for a dictionary's index and **24** for
a plain string's header, and **the signal is the schema's own Arrow type** — not the chunk's encodings,
which name `RLE_DICTIONARY` for a column that is not dictionary-encoded at all. **And it is checked
against the allocator's measurement**: **13.79** bytes a row against **13.39** on the narrow shape,
**16.25** against **16.03** on the two materialised columns, and **159.02** against **173.88** on the
file's sixteen — **and the file's own footer is 2.46× low** on the wide one. **So the estimate now says
**15,288,577** bytes where the read holds **13,719,522** *(was 13,719,490)* — **1.11× high where it was
9.15× low** — and, on the wide shape at the batch the row limit gave it, **975,512,992** where the read
holds **1,157,291,503** *(was 1,157,291,375)*. **And the
wide shape is still refused**, and now
for a reason that is not an accident: **the plan does not fit**.
**And what is still not explained:** why a shorter run of the probe costs more per row, when the table is
equally cold either way and the reader's work is the same.
[§3ac](PERF.md#3ac-the-join-orders-query-on-both-sides-taken-apart),
[§3ad](PERF.md#3ad-the-build-side-s-keys-as-a-filter),
[§3ae](PERF.md#3ae-the-probe-measured-twice) and
[§3af](PERF.md#3af-the-batch-sizes-price) have the tables.

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

**6. The byte budget's number — it is published, and it is open.** The default is **a byte budget of
16 MiB**, and the rule's output is published for every width and every worker count: the two narrow
shapes get **1,000,000** rows at one worker and **250,000** at four, and the file's sixteen columns get
**100,000** and **25,000** ([§3ah](PERF.md#3ah-the-default-moved)). **And the number is not settled by the
measurement, because the measurements disagree.** The `q6`'s predicate set's scan curve **turns up at a
whole row group** — its minimum is **13.740 ms** at **250,000** rows, and the rule's **1,000,000** is
**14.878 ms**, **8.3%** above it — and that is why the engine's cold `q6` is **16.6 ms** where the old
code's was **13.9 ms**, and its cold `sort-topk` **39.2 ms** where the old code's was **37.0 ms**. **And the
join's probe path says the opposite**: the engine's own batch is now a whole row group, and **that is the
minimum of the curve** — the query's path costs **−0.159 ms** against the ladder's, where the set this
increment replaces had **+4.273 ms**. **And a smaller budget is not free either**: at **4 MiB** the wide
shape's batch would be **20,000** rows, and the sweep's lowest rung is **62,500**. **And the scan's curve
is about to change again**, because the next increment makes the reader own the Parquet decode, and the
decode is what the batch's optimum turns on. **What would answer it:** the budget's number, measured with
the decode in place — **not** a smaller number chosen on a guess now.

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
  byte**. **The set costs nothing that can be claimed** — the rows out are **+30.155 ms** against the one file's
  *(was +1.133 ms)* and the write **+57.718 ms** *(was −24.333 ms)*, and the differences between the arms in
  every iteration — measured back to back, with the order alternating — straddle zero with a spread wider
  than either, on a host whose load average was 6.95, 7.0 and 6.76 — and the estimate does not move either:
  **975,512,992 bytes** for both *(was 1,549,500,016)*. **And the campaign was taken again**, because the
  engine's estimate changed and the estimate is one of this bullet's figures; the new figures are published and
  the old ones stand beside them. **And the round trip is the published file, byte for byte**: under the
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
  the caller owns the dataset there. **The rows out are six times faster**: **542.049 ms** against
  **3,338.468 ms**, and the copy that is left is **seventeen times smaller**: **164.513 ms** against
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
- **The `join-orders` query is taken apart on both sides, and the engine's lookup is the faster one.**
  Spec 046 measured **four arms in one session** — the engine's own plan output, `DuckDB`'s own profiler,
  `DuckDB` unprofiled, and `DuckDB` with its **join filter pushdown disabled** — with the two sides
  measured back to back in the same iteration and the order alternated. **The engine's join is 14.625 ms
  and `DuckDB`'s is 7.706 ms**, and the engine's is **+6.898 ms** slower in **every one of the eighteen
  iterations**. **And `DuckDB`'s own join, over the same six million rows, costs +51.422 ms more than its
  own filtered join and +44.190 ms more than the engine's**: `DuckDB` joins **216,783** rows and the engine
  joins **6,000,000**, so **the engine's lookup is four times the cheaper of the two**, and the engine is
  behind because it looks up every row where `DuckDB` tests a bit. **The filter is worth +45.697 ms** to
  `DuckDB` — it costs its scan **5.827 ms** and saves its join **51.422 ms** — and **the decomposition
  closes to 0.0000 ms on every iteration**. **No engine work changed**: the measurement decides what is
  built, and the next increment is the filter.
  [ADR-0058](docs/adr/0058-the-join-orders-query-on-both-sides-taken-apart.md) records it.
- **A filter over the build side's keys was measured, and it does not pay.** Spec 047 measured the two
  shapes a filter can take — **in front of the table**, and **compacting the rows** so that the survivors
  alone are probed — against the engine's own probe as the control, over a column materialised afresh in
  every iteration. **The prize is real: +6.981 ms**, the probe over the survivors alone against the
  probe as it is. **And it is unreachable.** The filter's own load is **5.222 ms** over six million rows
  and the table's own load is **4.399 ms**: **the direct-addressed table's byte is already the membership
  test** — the key's own range addresses it, and it consults nothing — **so a filter can only be
  consulted in addition to it, and what it is consulted in addition to is cheaper than it is.** The
  filter in front of the table is **+2.318 ms**, a loss; the filter compacting the rows is **+0.547 ms**,
  a wash. **No engine work changed.** **And the same test is worth seven times more in
  DuckDB** — its filter costs its scan **5.827 ms** and saves its join **51.422 ms**, where the prize
  here is **6.981 ms** — **because it replaces a hash join, and the engine has no hash join to pay for.**
  [ADR-0059](docs/adr/0059-the-build-side-s-keys-as-a-filter.md) records it.
- **The reader's batch size is a knob now, and the engine's default is the wrong end of the curve.**
  Spec 048 measured the engine's own probe in the **two paths it runs in** — over a materialised column
  outside the reader, and inside the reader — **in one session, back to back, with the order
  alternated**, and the query's path costs **+4.273 ms** more than the ladder's, in **24 of 24** paired
  samples. **And the batch size is what sets it**: the engine's own **125,000** rows against a row
  group's **1,000,000** is **−4.926 ms**, faster in **24 of 24** paired samples, and the curve is
  monotone — **−1.438 ms** at **250,000** rows, **+0.397 ms** at **62,500**. **The query pays it too**
  — the engine's join part is **15.823 ms** at the
  engine's own batch size and **9.641 ms** at a row group's worth, and the paired difference against
  `DuckDB` falls from **+8.083 ms** to **+1.898 ms**, **with the scan flat**. **And the default did not
  move**: `ParquetSource::with_reader_batch_rows` names a size in rows, and **no engine behaviour
  changed**. **What is not done:** whether the default *should* move, what a batch of a million rows
  costs in memory, and **why a shorter run of the probe costs more per row** when the table is equally
  cold either way and the reader's work is the same.
  [§3ae](PERF.md#3ae-the-probe-measured-twice) has the tables, and
  [ADR-0060](docs/adr/0060-the-probe-measured-twice.md) records it.
- **The batch size's price is measured, and the rule is a byte budget.** Spec 049 measured the **peak** a read
  holds — with a **counting global allocator**, at three widths — and the **time** on both shapes in one session,
  back to back, with the order alternated. **The bytes a batch holds scale with the query's width**: a whole row
  group is **12.8 MiB** on a narrow query and **165.8 MiB** on a wide one, **and the knob is named in rows, so it
  cannot see that**. **And the engine's memory estimate did not model the batch**: it said **1,500,016** bytes where the read
  holds **13,719,490**, and **1,549,500,016** where the read holds **1,157,291,375** — and the **default budget
  refused the wide shape at every batch size**. **So the rule is a byte budget, and the budget is 16 MiB**,
  bounded by the measurement from both sides and giving every shape at least what it has today.
  **What is not done:** the default does not move yet — moving it moves **every** published figure the batch size
  touches, and that is the next increment's work.
  [§3af](PERF.md#3af-the-batch-sizes-price) has the tables, and
  [ADR-0061](docs/adr/0061-the-batch-sizes-price.md) records it.
- **The engine now says what a row costs, and the estimate charges the batch.** Spec 050 gave the engine **one**
  statement of what a row costs — **8** bytes a row for a number, **4** for a dictionary's index and **24** for a
  plain string's header, **and the value's own bytes on top** — and **the signal is the schema's own Arrow type**,
  not the chunk's encodings, which name `RLE_DICTIONARY` for a column that is not dictionary-encoded at all.
  **And it is checked against the allocator's measurement**: **13.79** bytes a row against **13.39** on the narrow
  shape, **16.25** against **16.03** on the two materialised columns, and **159.02** against **173.88** on the
  file's sixteen — **and the file's own footer is 2.46× low** on the wide one. **So the estimate now says
  **15,288,577** bytes where the read holds **13,719,522** *(was 13,719,490)* — **1.11× high where it was
  9.15× low** — and **975,512,992** where the read holds **1,157,291,503** *(was 1,157,291,375)*, **and the
  budget's verdict on a plan now depends on the reader's batch size**, which it did not before.
  **What is not done:** the default still does not move — that is the next increment — and **the plain string's
  copy is not fixed**: the conversion copies every string out of the Arrow batch, one allocation a row, and the
  dictionary path borrows instead. **That is the wide shape's gap**, and the fix wants its own measurement.
  [§3ag](PERF.md#3ag-the-width-statement-paid) has the tables, and
  [ADR-0062](docs/adr/0062-the-width-statement.md) records it.
- **The default moved, and it cost the two queries that read a narrow shape.** Spec 051 made the reader's
  default **a byte budget of 16 MiB**, divided by what a row costs for the columns the read fetches, and **the
  rate is in eighths of a byte** — the width statement charges the validity bit an eighth, and a whole-byte rate
  was eight times the truth and cost the materialised shape half its batch. **And the whole campaign was taken
  again**, because the batch size touches a great many figures, with the old code measured **back to back and
  interleaved** so that the difference between the two sets is the code's. **And the engine's cold `q6` is
  16.6 ms** where the old code's was **13.9 ms**, and its cold `sort-topk` **39.2 ms** against **37.0 ms**,
  while `DuckDB`'s `q6` was flat at **32.0 ms** and the machine's own floor moved **1.6%**. **The reason is
  named**: the rule gives the `q6`'s predicate set **1,000,000** rows at one worker, and that shape's scan
  curve turns up at a whole row group — its minimum is at **250,000** rows, which is what the rule gives at
  **four** workers. **And the join's probe path says the opposite**: the engine's own batch is now a whole row
  group, and **that is the minimum of that curve**.
  **What is not done:** **the byte budget's number** — a smaller budget would move the wide shape's batch below
  anything measured, and the scan's curve is about to change again when the reader owns the Parquet decode — and
  **the plain string's copy**, which is still not fixed.
  [§3ah](PERF.md#3ah-the-default-moved) has the tables, and
  [ADR-0063](docs/adr/0063-the-default-moves-to-the-byte-budget.md) records it.
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
