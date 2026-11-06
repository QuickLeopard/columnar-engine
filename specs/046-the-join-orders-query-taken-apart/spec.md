# Spec 046 — The join-orders query, on both sides, taken apart

**Status**: done
**Started**: 2026-11-06
**Finished**: 2026-11-06
**Predecessor**: [spec 045 — the status dashboard](../045-the-status-dashboard/spec.md)

**CI green on `b7efb2f`** -- run 42241899945, sixteen of sixteen jobs.

## The problem

**The engine is behind `DuckDB` on the cold `join-orders` query, and the file says the answer is "the
parts, not the total".**

The engine's cold `join-orders` is **100.0 ms** against `DuckDB`'s **94.0 ms** — **6.0 ms** apart — while
the peer's is **175.3 ms** and `Polars`' **225.7 ms**; the set the bump replaces read **97.6 ms**,
**91.0 ms**, **168.8 ms** and **219.1 ms**. The gap is **0.6 ms** smaller than it was, and the whole of
that is `DuckDB`'s own movement.

**And the engine's own stage was taken apart, and the comparison never was.** Spec 026 took the join's
stage apart: **29.5 ms** of the cold query, **26.4 ms** in the set the bump replaces — **29.5%** of it,
and all of it the engine's own work — and the answer was that it is the **build side's read**,
**11.716 ms** of the **21.731 ms** cold stage, and `Arrow`'s decode of `orders.parquet`, which has been
measured three times and refused every time.

**But every part of that analysis was set against `DuckDB`'s *whole*.** The engine's stage is a part.
`DuckDB`'s figure is a total. A part cannot be compared with a whole, so the comparison the file has
carried since spec 026 is not a comparison at all — it is the engine's own account of itself next to
somebody else's number, and the reader is left to guess which part of the engine's account the
difference lives in.

### The question this increment asks

**Not "how much is the gap" — the gap is published and it is 6.0 ms. The question is: which part of
the query holds it?**

The answer has to be in **parts**, because:

- a part can be named on **both** sides, and a total can only be compared with a total;
- a part can be **turned off** and measured again, which is the only way a mechanism is established
  rather than guessed;
- a part can be **counted** — in rows — and the row counts are often the mechanism.

### The hypothesis, and it is a hypothesis

**`DuckDB` does not join faster than this engine. It joins fewer rows.**

`DuckDB` pushes the build side's keys into the probe side's scan as a **filter** — its own
`join filter pushdown`, which turns the build side's key set into a perfect-hash test the scan applies
to every row of the probe side — so the rows that reach the join are the ones that can match, and not
the six million the probe side holds. This engine has no such filter: its probe runs over **every** row
the reader hands it.

**If that is the mechanism, then it is visible three ways**, and this increment measures all three:

1. **the parts** — the two sides' join parts, against the two sides' totals, with the two sides' build
   sides and probe sides beside them;
2. **the rows** — how many rows each side's join actually sees;
3. **the counterfactual** — the same measurement with `DuckDB`'s filter pushdown **turned off**, which
   changes exactly one thing and therefore names the mechanism.

**And the third is the one that decides.** If the mechanism is the filter, then `DuckDB` without it
joins **six million** rows, and its join part becomes the size this engine's join part already is. If
the mechanism is something else, the counterfactual will show it.

### What this increment is not

**It is not a change to the engine.** The measurement comes first, and what it finds decides what is
built. A change built before the parts are known would be a guess with a benchmark attached.

## The requirements

### R1 — Both sides are taken apart, in one session, and each side's parts are that system's own account

- The engine's parts come from **the engine's own plan output**: the stage table, and the join report's
  own parts — the build side's whole, the probe, and the build side's open.
- `DuckDB`'s parts come from **`DuckDB`'s own profiler**: `PRAGMA enable_profiling='json'`, whose
  operator tree carries a timing per operator — the probe side's table scan, the build side's table
  scan, the hash join, the projection and the aggregate.
- Each side is measured **the way the published comparison measures it**: the engine's cold path over
  the file, `DuckDB`'s cold path over the file, one thread each, the plan cache disabled.
- The two sides are measured **back to back in the same iteration**, and the **order is alternated**
  between the iterations, so the machine's drift cannot land on one side.
- The iteration count is **even**, for the reason spec 026 gave: a machine that drifts while an
  iteration runs gives the first measurement a different price from the second, and an odd count would
  leave the two orders' groups different in size.

### R2 — The parts are named the same way on both sides, and each part says what it is

| part | the engine | `DuckDB` |
|---|---|---|
| **the probe side's read** | the scan stage | the probe side's table scan |
| **the build side's read** | the build side's whole | the build side's table scan |
| **the build side's open** | the build side's open | *no counterpart — it is inside `DuckDB`'s scan* |
| **the join** | the probe | the hash join |
| **the projection** | the project stage | the projection |
| **the aggregate** | the aggregate stage | the ungrouped aggregate |
| **the whole** | the query's elapsed | the query's total time |

- The engine's **table's construction** is charged to the engine's build side and to `DuckDB`'s hash
  join, and the difference is **named**, not averaged away.
- The engine's build side's **open** has no counterpart on `DuckDB`'s side, and its row says so rather
  than leaving a gap in the table.

### R3 — The mechanism is measured, not assumed: the filter is turned off, and the parts are measured again

- `DuckDB` is measured **twice**: once as it runs, and once with its **join filter pushdown disabled**
  — a change of exactly one thing.
- The counterfactual's **row count** is published beside its parts, because the rows are the mechanism.
- If the counterfactual does **not** move the join part, the hypothesis is **refused**, and the file
  says so.

### R4 — The answers do not change, and they are checked before anything is timed

- Both sides' answers are checked against the **published** value of the cold `join-orders` query —
  the same value the published campaigns recorded — **before** anything is timed.
- A probe that measured a different query would be a measurement of nothing, so the check is first.

### R5 — The bytes are accounted for

- `DuckDB`'s own **bytes read** are published, from its own profile.
- The engine's bytes are **not** measured by this increment, and the file says so: the engine's read is
  named by the **columns** it reports, and the file's own column sizes are the file's, not a
  measurement of the engine's read.
- The two sides' **row counts** are published on both sides, because a row count is what makes two
  measurements comparable.

### R6 — The figures are published, old-and-new, and the guard follows the new sets

- The parts are published in `PERF.md` under a **new** section, with **every figure that moved**
  published **old-and-new** and the reason.
- The raw reports are **committed**, and `scripts/medians.py` derives the published table from them, and
  `scripts/check-published-figures.py` checks the published figures against them.
- The published totals — the engine's **100.0 ms** and `DuckDB`'s **94.0 ms** — are **not** re-derived
  by this increment: they are the comparison harness's figures, taken by a different campaign, and the
  reason is recorded.

### R7 — The open question is answered in the open, whether the answer is a win or a loss

- `STATUS.md`'s last section — the open questions — is where the question this increment answers is
  rewritten, and the **next** question is written there too.
- If the finding is that this engine loses the query for a reason it can fix, the fix is **named** and
  the next increment is written down. **The finding is not the fix**, and the file does not pretend it
  is.

## The scope

- **In**: the cold `join-orders` query, on the engine's lazy path, against `DuckDB`'s cold path, one
  thread each, over the published file and the published build file.
- **In**: the parts of both sides, the rows both sides join, and `DuckDB`'s counterfactual.
- **Out**: the **resident** path. The question is about the cold query, and a second path would be a
  second question.
- **Out**: `Polars` and the `DataFusion` peer. The engine is behind **one** incumbent on this query, and
  the peer is **175.3 ms** away and `Polars` **225.7 ms** away — the question is not about them.
- **Out**: any **change to the engine**. The measurement decides that, and the decision belongs to the
  increment that follows.
- **Out**: the **comparison campaigns** (`just compare`). This increment does not re-take them, and the
  reason is recorded in R6.
- **Out**: `DuckDB`'s **planner**, its optimiser and its `io` layer as subjects. The profiler reports
  them; they are not the parts the query is made of.

## The instrument

**`crates/columnar-bench/src/join_parts.rs`**, driven by `just probe-join-parts`.

The crate is the right home for it because the crate is where the engine and the incumbents are
measured **by one harness**: `columnar-bench` already drives `DuckDB` as a subprocess against the same
Parquet file the engine reads, and a comparison whose two sides are measured by two harnesses is a
comparison of two harnesses.

**The four arms**, in one iteration:

| arm | what it is | what it yields |
|---|---|---|
| **the engine** | the engine's own `join-orders` plan over the lazy path | the stage table, the join's parts, the whole |
| **`DuckDB`, profiled** | the same SQL, with `DuckDB`'s own profiler on | the operator tree's timings, the whole, the bytes read |
| **`DuckDB`, unprofiled** | the same SQL, the way the comparison harness runs it | the whole, with no parts |
| **`DuckDB`, the filter off** | the profiled arm with the join filter pushdown disabled | the parts, and the rows that reached the join |

The **unprofiled** arm is what makes the profiled arm honest: the profiler is not free, and the whole
the parts sum to is not the whole the comparison harness measures. Both wholes are published, and the
difference between them is the profiler's price.

**The answer check** (R4) runs first, on both sides, before anything is timed.

**The report** carries, per iteration and per arm: every part's time, every row count, the whole, the
bytes read, the host's load average, and the machine's spec. It carries **the order the arms ran in**,
because the order is what takes the machine's drift out of the difference.

## The measurement

- **Three campaigns**, each of **six iterations**, the arms **interleaved** within an iteration.
- The **median** of each part across the campaigns is the published figure, and the **spread** is
  published beside it wherever it is wide enough to change a reader's conclusion.
- Every part's **own spread** is published, because a part is judged against its own spread and not
  against the query's.
- The **raw reports are committed** under `bench-results/`, and the published table is derived from
  them by `scripts/medians.py`.

### The outcome

**The hypothesis is confirmed, and it is published with its numbers in
[§3ac](../../PERF.md#3ac-the-join-orders-query-on-both-sides-taken-apart).**

The engine's **lookup is the cheaper of the two, row for row** — `DuckDB`'s own join, over the same six
million rows, costs far more than the engine's — and the engine is behind because it looks up **every**
row where `DuckDB` tests a bit over **216,783** of them. **The next increment is the filter**, and its
prize is now a number rather than a hope.

**The figures live in the section that publishes them**, and they are not repeated here: one owner per
figure, and the guard checks them there.

## The references

- [ADR-0058](../../docs/adr/0058-the-join-orders-query-on-both-sides-taken-apart.md) — the decision: both sides are
  taken apart, in one session, and the mechanism is measured with a counterfactual.
- [STATUS.md](../../STATUS.md) — the open question this increment answers, and the house's single
  account of the state of the work.
- [PERF.md §3o](../../PERF.md#3o-the-joins-stage-taken-apart) — the join's stage, taken apart: the
  engine's own parts, and the analysis this increment puts against `DuckDB`'s.
- [PERF.md §2b](../../PERF.md#2b-cold--read-the-parquet-file-and-answer-in-one-go) — the cold
  comparison, and the totals this increment does not re-derive.
- [constitution.md](../../constitution.md), principle **IV — Evidence Over Claims**: a number is a
  claim about a measurement, and a claim without its evidence is not published.
- [constitution.md](../../constitution.md), principle **V — The Comparison Is Reproducible by a
  Stranger**: a stranger clones the repository, runs the gate, and gets the same verdict — so both
  sides are measured by one harness, in the same session, against the same file.
