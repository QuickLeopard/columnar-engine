# Spec 006 — The arithmetic aggregate: the real Q6

**Status**: accepted
**Date**: 2026-10-25
**Depends on**: [spec 002](002-lazy-columnar-scan/spec.md) (the lazy scan), [spec 003](003-kernel-pushdown/spec.md)
(the pushdown runs the engine's kernel), [spec 005](005-parallel-decode/spec.md) (the parallel read)

---

## 1. The problem, measured

Everything published so far measures **Q6's predicate**, with `count(*)` as the aggregate:

```sql
SELECT count(*) FROM lineitem
WHERE l_discount >= 0.04 AND l_discount <= 0.07
  AND l_shipdate >= '1994-01-01' AND l_shipdate < '1995-01-01';
```

TPC-H's Q6 is not that. Q6 is:

```sql
SELECT sum(l_extendedprice * (1 - l_discount)) FROM lineitem
WHERE l_shipdate >= '1994-01-01' AND l_shipdate < '1995-01-01'
  AND l_discount >= 0.05 AND l_discount <= 0.07
  AND l_quantity < 24;
```

The difference is not the predicate — it is that Q6 **evaluates arithmetic over columns**. The
engine's aggregate sums a column; it cannot compute `a * (1 - b)` and sum the result. So the
repository measures a query that looks like Q6 and is not Q6, and says so in the README, which is
the honest thing to do and also a smaller thing than doing the work.

**The limitation is the aggregate's input.** The engine's `SUM` takes a column name. Q6 needs an
*expression*. Everything else the engine can already do: the predicate, the pushdown, the lazy
read, the parallel read.

## 2. What this increment does

It adds **arithmetic expressions** to the plan, and lets an aggregate sum one.

The shape of the work is deliberately small:

- An **expression** is a tree of constants, column references, and the four arithmetic
  operators.
- The expression is **evaluated into a column**, over the rows that survived the filter, exactly
  as the projection materialises a column.
- The aggregate then sums that column, with **no change to the aggregate operator at all**.

That last point is the design's whole argument. An expression is a *computed column*, and a
computed column is a column. If the expression becomes a column before the aggregate sees it,
then the aggregate's type rules, its NULL handling, its saturation and its accumulator all keep
working, and the increment does not touch the operator that the existing differential tests
already cover.

## 3. Requirements

**R1 — The expression type.**
`columnar-core` gains an expression: constants, column references, `+`, `-`, `*`, `/`, and unary
negation. It is part of the **shared vocabulary**, next to the predicate, because the plan's
surface is the vocabulary's surface.

**R2 — The expression is resolved and type-checked before anything is read.**
An expression naming a column the file does not have is a plan error, reported before the read.
The expression's *type* is derived from its operands: `int64` only if every operand is `int64`,
and `float64` otherwise. There is no implicit cast, for the same reason there is none in the
predicate: an implicit cast is where a query planner starts lying about what it did.

**R3 — Division is rejected unless an operand is a float.**
`5 / 2` is `2` in PostgreSQL and `2.5` in DuckDB. The engine refuses it rather than pick a side,
and says why. This is the same rule the predicate follows for a type mismatch.

**R4 — The expression's columns are materialised.**
A column the expression reads is a column the plan reads, so it is in the late-materialisation
set, so the reader fetches it, and the plan output names it.

**R5 — The expression is evaluated over the rows that survived the filter.**
SQL applies `WHERE` before the aggregate, so the expression is evaluated over the survivors, not
over the whole file. The evaluation is a new pipeline stage, between the filter and the
aggregate, and the plan output shows it.

**R6 — NULL in, NULL out.**
A row whose expression reads a NULL column is a NULL row, and the sum skips it — which is SQL's
rule, and the rule the engine's column sum already follows.

**R7 — Arithmetic is IEEE-754.**
`NaN` propagates through arithmetic, and `inf` does too. This is deliberately *unlike* the
comparison rules, where a `NaN` never matches: a comparison is a question and a `NaN` has no
answer, while arithmetic is a computation and `NaN` is a value. The two rules live in different
places and are not allowed to leak into each other.

**R8 — Integer arithmetic saturates.**
`i64` arithmetic saturates at the type's bounds rather than wrapping, matching the engine's
existing integer accumulator. A wrapping sum would be a different answer on a different machine.

**R9 — The real Q6 is a built-in query, and it is measured against the incumbents.**
`q6-revenue` is TPC-H's Q6 as written above, with the same SQL handed to `DuckDB` and `Polars`,
on the same file, in the same harness.

**R10 — The differential tests.**
The eager path and the lazy path agree on the expression's answer. The expression's value agrees
with a hand-computed one. NULL, `NaN` and the type rules are pinned by unit tests.

## 4. Out of scope, stated rather than deferred

- **The expression is not a general expression language.** There are no comparisons in an
  expression, no `CASE`, no function calls, no casts. An expression computes a number.
- **The expression is evaluated over the survivors, row by row.** There is no vectorised
  arithmetic kernel in this increment, and no SIMD path for the evaluation. The arithmetic is
  over the *filtered* rows, so it is a fraction of the scan's cost — but it is not free, and the
  measurement will say exactly what it costs. If the measurement says the arithmetic is the next
  bottleneck, the next increment vectorises it, and that is a measurement-driven decision rather
  than a guess.
- **The aggregate's other functions do not take expressions.** `MIN`, `MAX` and `COUNT` still
  take a column. Nothing needs them to, and a wider surface is a wider surface to keep honest.
- **No SQL parser.** The expression is built in Rust, like the predicate.
- **`SUM` over an expression is a floating-point sum**, and floating-point sums are not
  associative. The engine sums in the file's row order, so the answer is deterministic, and the
  comparison against the incumbents uses a relative tolerance — as it already does for every
  floating-point figure in this repository.

## 5. The measurement

The real Q6, cold and resident, against `DuckDB` and `Polars`, on the same file, on the same
machine, in the same three runs as everything else — and, like everything else, published with
its medians, its spread and its conditions.

**The question this increment answers:** what does the engine's arithmetic cost, and where does
the real Q6's time go once the aggregate is an arithmetic one rather than a count?

## 6. What would make this wrong

- **The expression is evaluated before the filter**, because it is easier. Then the arithmetic
  would run over 6,000,000 rows instead of the survivors, and the measurement would be about a
  different engine than the one that ships.
- **The expression's columns are not materialised**, because the scan is already written. Then
  the read would fetch the wrong columns, and the answer would be right by accident.
- **The aggregate is changed** to take an expression directly. Then the NULL, saturation and
  type rules would have to be re-derived for the expression case, and the existing differential
  coverage would no longer cover the new path.
