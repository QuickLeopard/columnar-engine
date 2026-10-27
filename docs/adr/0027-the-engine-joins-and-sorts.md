# ADR-0027: The engine joins and sorts

**Status**: accepted
**Date**: 2026-10-27
**Reviewers**: TBD

## Context

Spec 001 listed what the first increment would not build. Six of the seven entries are decisions
with a reason behind them — a SQL parser is "a month of work that produces no evidence", a decimal
type would be "a claim, not evidence", GPU acceleration is "CPU-only by design", nested types are
"not present in the TPC-H schema", morsel-driven parallelism belongs to spec 005.

One entry is different:

> **Hash join and sort** — The Q6 path needs neither. They are in the "full engine" increment.

That is a promise, and the project is now the thing that made it. The engine's operator set is
filter, project and hash aggregate; the pipeline is pinned at four stages, `scan -> filter ->
project -> aggregate`; and every published query reads **one table**. A reader who wants to know
whether this engine can join has to read a spec from the project's first week to find out that it
cannot.

The shape of the answer is not a free choice, though. Three things constrain it, and they are the
same three constraints the rest of the engine is built on.

1. **The probe side stays lazy.** The engine's whole read is predicated: the predicates are
   pushed into the scan and the rows that fail them never reach the operators behind it (ADR-0015).
   A join that forced the probe side to be materialised first would give that up for the sake of
   one query.
2. **The build side is materialised, and that is unavoidable.** A hash table is a hash table: the
   rows on the right side have to be in memory to be looked up. The only question is *which* side
   is the build side, and the answer is the one that is smaller — the `orders` table, not the
   six-million-row `lineitem`.
3. **The answers must stay checkable.** The comparison's rule is "the same answer": the engine's
   result is cross-checked against `DuckDB`'s, `Polars`'s and the `DataFusion` peer's, with a
   relative tolerance of 1e-9, and a disagreement is a warning in the report. A query whose answer
   is a *set* of groups needs a set comparison across four engines — a different piece of
   machinery, and one that would arrive untested on the same day as the join.

## Decision

**The engine joins and sorts, and the two operators are checked against brute-force references.**

1. **The join is an inner equi-join, executed as a hash join.** The build side is the right table:
   it is read, its own predicates are evaluated **before** the table is built, and the surviving
   rows are hashed on the key. The probe side is the left table, and it stays on the lazy path with
   its predicates pushed into the scan. A NULL key never matches, on either side — SQL's rule.
2. **The sort is a stage, and a limit makes it a bounded heap.** `ORDER BY <column> [DESC] [LIMIT
   n]` keeps a heap of `n` rows when there is a limit — the published query asks for ten rows out
   of six million — and sorts everything when there is not.
3. **The pipeline becomes `scan -> filter -> project -> [join] -> [sort] -> aggregate`**, and the
   pinned optimisation order constant is extended to match and printed at the top of every plan
   output. A plan with a sort and an aggregate aggregates over **the sorted rows**: the order of
   the stages is part of the plan.
4. **Both operators are checked against a reference implementation, row for row.** The references
   are deliberately slow and obviously correct: a nested-loop join and a plain sort. A hash join
   that is only checked against itself is a hash join that can be wrong.
5. **The published queries aggregate over the whole join and over the sorted rows**, so their
   answers are single numbers and the harness's existing cross-check applies unchanged.
6. **The incumbents get the second table as well.** `DuckDB` reads both Parquet files by path in
   cold mode; `Polars` and the `DataFusion` peer register both tables under their names in resident
   mode. The harness's "how does this system name the table it reads" decision becomes a decision
   about a *list* of tables.
7. **The dataset grows a second table, and the generator stays pure.** `columnar-testdata`
   generates TPC-H's `orders` at SF-1 beside `lineitem`, with `o_orderkey` drawn from the same
   key space as `l_orderkey`, so the join has a real selectivity. The manifest records the key
   space and the match rate. The generator remains a pure function of `(scale_factor, seed,
   null_rate)`, and generating `orders` does not perturb a single `lineitem` row.

## Consequences

**Easier.** The engine can answer the question a reader is most likely to ask about it, and the
question is answered with evidence rather than with a citation to an out-of-scope table. The
published comparison grows from six single-table queries to eight, of which one joins and one
sorts — so the published numbers now describe a query engine rather than a filter.

**Harder.** The plan is no longer a four-stage pipeline with a fixed meaning, and the executor has
three paths — eager, lazy and lazy-with-a-join — that must agree on the answer. The comparison
harness now has to hand a second relation to four systems, and a driver that cannot register it
fails the whole comparison rather than one query.

**Given up.** This increment publishes **no grouped join** and **no window function**, because
neither is in the promise and both would need a row-set comparison to be measurable. The engine
also still has no SQL parser, so the join is expressed as a typed plan, not as text — the same
trade spec 001 made, and for the same reason.

## Measurement

**The operators.** The join and the sort are each checked against a brute-force reference in a
differential test: the same rows, in the same order, value for value. The join's reference is a
nested loop over both sides; the sort's is a plain sort of everything that survives the filter.
The tests are in the tree and they run in CI.

**The queries.** The two new queries are published in `PERF.md` beside the six existing ones, as the
median of three full `just compare` campaigns with the spread, derived by `scripts/medians.py` and
checked by `scripts/check-published-figures.py`. Every incumbent answers the same SQL over the same
two files, and the answers are cross-checked against the engine's with the harness's 1e-9 relative
tolerance.

| | the engine, cold | duckdb | polars | datafusion |
|---|---|---|---|---|
| `join-orders` | **216.1 ms** | 97.0 ms | 231.6 ms | 177.3 ms |
| `sort-topk` | **61.6 ms** | 40.0 ms | 1460.6 ms | 41.3 ms |

**The engine's first hash join is behind `DuckDB` and behind the `DataFusion` peer, and ahead of
`Polars`.** Its top-k sort is behind `DuckDB` and the peer, and 24× ahead of `Polars`. That is what
the measurement says, and it is published as it is. The join's **resident** figure is not published
at all: in resident mode the incumbents have both tables in memory before the clock starts, while
the engine's resident path reads the build side inside the clock, so the two numbers would not be
the same measurement. The raw report carries the engine's 138.1 ms; it is named in the prose instead
of published next to a number measured differently.

**The defect the increment found.** The first implementation probed with `std`'s default hasher,
`SipHash`, which is built to be resistant to collision attacks -- a property a query engine's hash
table has no use for, and a cost it does, because the probe is the join's hot loop and it runs once
for every row of the probe side. With a hash written for the job, the join stage went from **219.9 ms
to 117.7 ms** and the whole lazy query from **318.9 ms to 199.5 ms**, on the same file, in the same
session, with the same answer. The numbers are in §4 of `PERF.md` with the rest of the defects the
increments found.

**The cost of the decision, stated.** The join's build side is materialised, so the join reads the
`orders` file whole before it probes anything. That is the price of a hash join, and the published
cold figure is the number that says what it costs next to the incumbents' -- which do the same thing
with their own build sides. The build side's memory is also counted in the plan's estimate, which
reserves the file's whole row count because the filter's selectivity is not known before the read:
220.3 MiB of a 256 MiB budget at SF-1. It fits, and at a larger scale factor it would be refused
before any work was done -- a limit, not a claim.

## Confidence

**high** — the two operators are new code, but the checks on them are not novel: a brute-force
reference is the same technique the kernel's six implementations are held to, and the queries are
measured by the same harness that measured the six before them.
