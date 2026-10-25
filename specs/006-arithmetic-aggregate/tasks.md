# Spec 006 — Tasks

**Spec**: [`spec.md`](spec.md)
**Status**: **done.** Every requirement below is met and checked.

Legend: `[x]` done · `[ ]` open · `[~]` partially done, and the gap is written down.

---

## T1 — The expression type

- [x] `columnar-core` gains an expression: constants, column references, `+`, `-`, `*`, `/` and
      unary negation (`crates/columnar-core/src/expr.rs`).
- [x] It is part of the **shared vocabulary**, next to the predicate, and it is re-exported from
      the crate's root.
- [x] `Expr::columns()` lists the columns it reads, once each, in the order it first reads them.
- [x] `Expr::to_sql()` renders it the way SQL writes it, with the binary operations
      parenthesised — so `(l_extendedprice * (1 - l_discount))` cannot be misread as
      `l_extendedprice * 1 - l_discount`.

## T2 — The resolution

- [x] The expression is resolved and type-checked **before anything is read**: an expression naming
      a column the file does not have is a plan error, reported before the read.
- [x] The type is `int64` only when every operand is `int64`, and `float64` otherwise.
- [x] A `utf8` column and a `utf8` constant are both rejected: an arithmetic expression computes
      a number.
- [x] **Integer division is refused**, with a message that says why: `5 / 2` is `2` in
      `PostgreSQL` and `2.5` in `DuckDB`, and the engine does not pick a side.
- [x] The resolved expression carries the **column's type**, so the evaluator does not have to
      look the schema up again.

## T3 — The materialisation

- [x] The expression's columns are in the late-materialisation set, so the reader fetches them and
      the plan output names them.
- [x] The lazy path resolves the plan against the **file's** schema and remaps the indices
      afterwards, and the expression's indices are remapped with the rest — a test writes a file
      whose columns are in a different order from the expression's and checks the answer.

## T4 — The evaluation

- [x] The expression is evaluated over the rows that survived the filter — SQL applies `WHERE`
      before the aggregate, and so does this — into a **column**.
- [x] The evaluation is its own stage, and it appears in the plan output as `evaluate`.
- [x] **A sum over a plain column has no evaluate stage**: there is nothing to evaluate, and the
      old behaviour is exactly the old behaviour.
- [x] NULL in, NULL out: a row whose expression reads a NULL is a NULL row, and the sum skips it.
- [x] Arithmetic is IEEE-754: `NaN` propagates, `inf` propagates, and a division by zero is an
      infinity rather than an error.
- [x] Integer arithmetic saturates, matching the engine's integer accumulator.
- [x] An integer column inside a float expression is widened, which is what makes
      `1 - l_discount` an arithmetic expression rather than a type error.

## T5 — The aggregate

- [x] **The aggregate operator is unchanged.** The expression is a computed column before the
      aggregate sees it, so the aggregate's type rules, its NULL handling, its saturation and its
      accumulator all keep working — and the differential tests that covered them cover this path
      too.
- [x] `Aggregate::Sum` takes an expression rather than a column name. `Expr::column(..)` is the
      old behaviour, so the surface is a **superset** of what it was.

## T6 — The query and the measurement

- [x] `q6-revenue` is TPC-H Q6 itself: `sum(l_extendedprice * (1 - l_discount))` over the
      discount range `[0.05, 0.07]`, the ship-date year 1994 and quantities under 24.
- [x] The same SQL is handed to `DuckDB` and `Polars`, on the same file, in the same harness.
- [x] The engine's answer is `5526985127.48064` and `DuckDB`'s is `5526985127.480705`. The two
      differ in the last few digits because a floating-point sum is not associative and the two
      systems sum in different orders — and the comparison is relative, for exactly that reason.
- [x] The query is measured cold and resident, against both incumbents, and published with its
      medians, its spread and its conditions.

## T7 — The differential tests

- [x] The eager path and the lazy path agree on the expression's answer, over the same data
      including a NULL.
- [x] The expression's value agrees with a hand-computed one.
- [x] The rules are pinned by unit tests: the unknown column, the string column, the integer
      division, the type widening, the NULL row, the `NaN`, the saturation, and the plain column's
      unchanged behaviour.
- [x] The CLI's end-to-end suite checks the real Q6's answer against `DuckDB`'s, on the real
      generated file.

## T8 — The follow-ons, not done

- [ ] **The evaluation is not vectorised.** It is a scalar loop over the rows that survived the
      filter. The measurement in `PERF.md` says exactly what that costs — and if the arithmetic
      turns out to be the next bottleneck, the next increment vectorises it. That is a decision a
      measurement makes, rather than a guess.
- [ ] **The expression language is narrow on purpose.** No comparisons, no `CASE`, no function
      calls, no casts, no subqueries. An expression computes a number.
- [ ] **The other aggregates do not take expressions.** `MIN`, `MAX` and `COUNT` still take a
      column. Nothing needs them to, and a wider surface is a wider surface to keep honest.
- [ ] **The filter's column is not pushed down when the expression reads it.** `l_discount` is
      both filtered and summed, so the engine filters it rather than the reader doing it. That is
      the existing rule from spec 002 — a column that is materialised cannot also be pushed down
      — and changing it is a separate piece of work with its own correctness argument.
