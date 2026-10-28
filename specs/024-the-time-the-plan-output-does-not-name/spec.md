# Spec 024 — The time the plan output does not name

**Status**: done
**Started**: 2026-10-28
**Finished**: 2026-10-28
**ADR**: [ADR-0031](../../docs/adr/0031-the-time-the-plan-output-does-not-name.md) — written from the instrument's measurement
**Predecessor**: [023-the-scan-that-feeds-the-join](../023-the-scan-that-feeds-the-join/spec.md)

## The problem

The engine's plan output is the engine's own account of its work: a stage table, the rows each
stage was handed and produced, and the time each stage took. **On six of the eight published
queries that account is complete.** The stages add up to the query's time to within a fifth of a
millisecond.

The resident path, the median of three campaigns on this host, from
`bench-results/compare-spec023-run{1,2,3}.json` — the set this spec replaces:

| query | the query | the stages | **what the stages do not name** |
|---|---|---|---|
| `scan` | 0.0 ms | 0.0 ms | 0.0 ms |
| `filter-numeric` | 2.5 ms | 2.4 ms | 0.1 ms |
| `filter-string` | 3.6 ms | 3.5 ms | 0.1 ms |
| `filter-range` | 6.7 ms | 6.5 ms | 0.2 ms |
| `q6` | 9.9 ms | 9.9 ms | 0.0 ms |
| `q6-revenue` | 14.9 ms | 14.9 ms | 0.0 ms |
| **`join-orders`** | **29.9 ms** | **25.1 ms** | **4.8 ms** |
| **`sort-topk`** | **11.1 ms** | **5.8 ms** | **5.3 ms** |

The two queries that name a **sort** and a **join** leave time that the plan output does not, and
the plan output does not say what it is. The cold path says the same thing about the sort and
nothing about the join: `sort-topk` is **45.6 ms** against **41.6 ms** of stages — **4.0 ms**
unnamed — while `join-orders` is **91.8 ms** against **91.5 ms**.

**And the sort is the engine's largest remaining gap.** The engine's cold `sort-topk` is
**45.6 ms** against DuckDB's **38.0 ms**, and the gap is not where the plan output says it is: the
plan output says the scan is **35.7 ms** and the sort stage **6.0 ms**. The scan is Arrow's decode,
which has been measured three times and refused every time. So the engine's own work on that
query is the sort's **6.0 ms** and the **4.0 ms** the plan output does not name — **10.0 ms**, and
not one microsecond of it is a decode.

**What the code says the time is.** One thing the join and the sort do that the other six do not:
they ask the filter's selection for its **row numbers** (`selection.rows()`), before either
stage's clock starts. A plan with no filter has the selection "every row", so that is **6,000,000
row numbers** — **24 MB** — built outside every stage, and handed to the join or the sort as a
`Vec<u32>`. The six queries that name neither a join nor a sort never ask, and their attribution
is complete.

Two questions, and they are different:

1. **Is the selection's expansion the whole of the time the plan output does not name?** The engine
   does other work outside its stages — the plan's resolution, the memory estimate, the build
   side's file — and the plan output does not say how much of it there is.
2. **Do the join and the sort need an index vector at all?** The sort keeps a heap of ten rows and
   the join looks each probe row up in a table; neither needs the row numbers of six million rows
   in a vector that is built first and read once.

## The requirements

### R1 — The whole query is measured, not only the stages

For **every** published query and on **both** paths, the instrument measures the whole query and the
plan output's own stage table, and the **difference between the whole query and the stages is
measured, not inferred** — it is the figure the other shapes have to account for.

The shapes the plan output does not name are measured **once**, on the dataset's **"every row"**
selection, because that is what they are a property of:

| shape | what it is |
|---|---|
| `selection_rows` | the filter's selection expanded into row numbers |
| `selection_walk` | the same rows, read as they are needed |
| `bitmap_rows` | the same expansion, with the selection held as a bitmap |
| `bitmap_walk` | the same walk, with the selection held as a bitmap |
| `plan_resolve` | the plan's resolution against the dataset or the file |
| `memory_estimate` | the three estimates the engine makes before it reads |
| `build_open` | the join's build side's file opened (the join only) |
| `stages` | the plan output's own stage table, sum and per stage |
| `query` | the whole query, as the harness measures it |

Every shape is measured on the same file, in the same session, and the figures are the medians of
three campaigns.

> **Amended, after the instrument was written** — the shapes are measured once, and not once per
> query, and the reason is worth recording. A query **with** a predicate does not have the "every
> row" selection: it has the bitmap of its survivors, which is a different shape, and that bitmap is
> measured as the **control**. The selection the hole was about — the one a plan with no predicate
> has — is the same selection for every such query, so measuring it once is measuring it exactly.
> What is measured **per query**, on **both** paths, is the whole query, the stage table and the
> difference between them, and that is the requirement R2 is checked against. The amendment makes
> the requirement narrower in one place (the shapes are not repeated per query) and leaves the
> per-query measurement exactly as it was.

### R2 — The plan output accounts for the whole query

The plan output is the engine's own account of its work, so it must be one: **the stages it
reports add up to the query's time**. Any part of the query the engine does not time is either
inside a stage or reported as a stage of its own.

The check is mechanical: for every published query, on both paths, the stages' total and the
query's elapsed agree within the measurement's noise, and the plan output says so.

### R3 — The selection is not expanded into an index vector the operators do not need

If the measurement shows the expansion is the time the plan output does not name, the engine stops
building it: the selection is handed to the join and the sort as the rows it is, not as a vector
of their numbers, and the expansion is gone rather than renamed.

If the measurement shows it is not, the expansion is named, and the time it takes is published.

**Either outcome is the same requirement**: the time is inside a stage or it does not happen.

### R4 — The answer does not change

The rows the join and the sort are handed must be the same rows, in the same order, with the same
values, and the engine's answer must not change. The differential suites hold the plan output's
row counts and the aggregate's answer to the same values, and a test asserts that the plan output
accounts for the query.

### R5 — The figures are published

The instrument's medians, the before and the after side by side, the reason for every moved
figure, and the raw reports committed. The gate derives every published figure from the committed
reports.

## The instrument

`crates/columnar-engine/examples/query_cost_probe.rs` — the engine's query, in the shapes above.

The probe asks the engine for the plan it would run, runs the query through the engine's own
entry point, and takes the plan output's stage table as the engine reports it. It measures the
shapes the plan output does not name **separately**, in the same session, on the same file, and it
checks the query's answer against the engine's own before it times anything.

The probe reports, per shape and per query, the median of five iterations and the spread, the
host's load, and the commit it ran at.

## The measurement

The instrument (`crates/columnar-engine/examples/query_cost_probe.rs`), against the old code and
against the new one, three campaigns each, back to back on this host — the median of five
iterations inside each campaign.

**The shapes** — the dataset's "every row" selection, which is the selection a plan with no
predicate has, and the one the old code expanded into row numbers. They are measured **once**,
because they are a property of the dataset and not of a query:

| | the old code | the new code |
|---|---|---|
| the selection expanded into row numbers | 4.13 ms | 0.21 ms |
| the same rows, read as they are needed | 2.86 ms | 0.56 ms |
| the same expansion, with the selection a bitmap — **the control** | 4.69 ms | 4.22 ms |
| the same walk, with the selection a bitmap — **the control** | 2.83 ms | 2.84 ms |

**Every published query, on both paths** — the time the plan output does not name, which is the
whole query less the plan output's own stage table:

| query | the old code, resident | the new code, resident | the old code, cold | the new code, cold |
|---|---|---|---|---|
| `scan` | 0.020 ms | 0.001 ms | 0.016 ms | 0.002 ms |
| `filter-numeric` | 0.001 ms | 0.001 ms | 0.013 ms | 0.007 ms |
| `filter-string` | 0.003 ms | 0.002 ms | 0.020 ms | 0.005 ms |
| `filter-range` | 0.005 ms | 0.004 ms | 0.015 ms | 0.009 ms |
| `q6` | 0.005 ms | 0.004 ms | 0.011 ms | 0.010 ms |
| `q6-revenue` | 0.009 ms | 0.010 ms | 0.016 ms | 0.022 ms |
| **`join-orders`** | **4.684 ms** | **0.016 ms** | 0.257 ms | 0.017 ms |
| **`sort-topk`** | **4.324 ms** | **0.004 ms** | **4.299 ms** | 0.009 ms |

**The two queries with the hole, in full** — the resident path:

| | the old code | the new code |
|---|---|---|
| the whole query (`join-orders`) | 27.03 ms | 22.61 ms |
| the plan output's own stage table (`join-orders`) | 22.36 ms | 22.59 ms |
| **the time the plan output does not name** (`join-orders`) | **4.684 ms** | **0.016 ms** |
| the whole query (`sort-topk`) | 8.95 ms | 3.90 ms |
| the plan output's own stage table (`sort-topk`) | 4.26 ms | 3.89 ms |
| **the time the plan output does not name** (`sort-topk`) | **4.324 ms** | **0.004 ms** |

**The answer to the first question: yes, the expansion was the whole of it.** The unnamed time —
**4.684 ms** on the join and **4.324 ms** on the sort, resident — is the selection expanded into row
numbers: **4.13 ms**, the cost of building the row numbers of six million rows. The bitmap rows are
the **control**, and they did not move: **4.69 ms** against **4.22 ms** on the same host, back to
back. The machine is not what changed; the code is.

**And the cold path tells the same story from the other side.** The old code's cold `join-orders`
left only **0.257 ms** outside its stages — the streamed join reads the selection as it goes and
never expands it — while the cold `sort-topk` left **4.299 ms** outside, because the sort still asked
for the row numbers. The hole was where the code asked.

**The answer to the second question: no, the join and the sort do not need an index vector.** The sort
keeps a heap of ten rows and the join looks each probe row up in a table; neither needs the row
numbers of six million rows in a vector that is built first and read once. So the expansion is **gone
rather than renamed** — R3's first outcome, and the requirement holds either way.

**The attribution, after the change.** The plan output's account of the whole query closes on
**every** published query, on **both** paths: the largest time outside the stages is **0.022 ms**,
and what is left outside the stages is the plan's resolution (**0.0003 ms**) and the memory estimate
(**0.00004 ms**).

The reports are `bench-results/query-cost-run{1,2,3}.json` and
`bench-results/query-cost-before-run{1,2,3}.json`, and `python3 scripts/medians.py` derives the
tables above from them.

**One measurement note, and it is a correction.** The walk is measured by **summing** the rows, and
the sum is taken with a mask drawn from `black_box` outside the loop. A plain sum of `0..n` is a
closed form the compiler computes without walking at all: the plain sum of six million rows measures
**0.000 ms** on this host, and the same walk with the mask measures **0.8 ms**. The mask is the same
for both walks, so the two shapes are measured the same way.
