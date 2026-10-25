# ADR-0012: An arithmetic expression is a computed column

**Status**: accepted
**Date**: 2026-10-25
**Related**: [ADR-0010](0010-pushdown-equivalence.md) (the pushdown runs the engine's kernel), [ADR-0011](0011-parallel-decode.md) (the parallel read), [spec 006](../../specs/006-arithmetic-aggregate/spec.md)

## Context

TPC-H's Q6 is `sum(l_extendedprice * (1 - l_discount))`. The engine's aggregate sums a column. So
the repository measured **Q6's predicate** with `count(*)` and published it under the name "Q6" —
and the README said so, which is the honest thing to do and also a smaller thing than doing the
work.

Closing the gap is not hard. Closing it *well* is.

## The decision: an expression becomes a column

**The expression is evaluated into a column, over the rows that survived the filter, and the
aggregate sums that column. The aggregate operator is not changed.**

This is the whole design, and it is worth being explicit about why it is the right one.

The alternative — teaching the aggregate operator to take an expression — would mean the
aggregate's type rules, its NULL handling, its saturation and its accumulator all had to be
re-derived for the expression case. The aggregate's accumulator is chosen from the *column's*
physical type; an expression has no physical type until it is evaluated. So the aggregate would
need a second, parallel notion of "what type is this value", and the differential tests that
cover the aggregate's rules today would cover the column case and not the expression case.

A computed column is a column. The aggregate keeps exactly one notion of value type, exactly one
NULL rule and exactly one saturation rule, and the existing differential coverage covers the new
path because the new path *is* the old path with a different input.

The engine's `SUM` of a plain column is unchanged, down to the plan output: a sum over a plain
column does not even get an `evaluate` stage, because there is nothing to evaluate.

## The type rules, and the one place they are not obvious

An expression's type is `int64` only if **every** operand is `int64`, and `float64` otherwise.
There is no implicit cast in the other direction. This is the same rule the predicate follows — an
`int64` column compared against a `float64` threshold is a plan error — and it is the same rule
for the same reason: an implicit cast is where a query planner starts lying about what it did.

Two consequences worth naming:

- **`1 - l_discount` works even though `1` is an integer.** The `int64` constant is widened
  because the *other* operand is a `float64`, and the whole expression is a `float64`
  expression. The widening is lossy above 2^53, and it has to be — the alternative is refusing
  the expression, which would make Q6 inexpressible for no gain.
- **Integer division is refused.** `5 / 2` is `2` in PostgreSQL and `2.5` in DuckDB, and an
  engine that silently picks one of them produces answers that are not portable. The plan
  rejects it and says why. This is the one rule in the increment that exists purely because the
  engines disagree, and it is the only place where the engine's arithmetic could have been
  ambiguous.

## Arithmetic is not comparison

The engine's comparison rules are strict in a way its arithmetic rules are not, and the two must
not leak into each other:

- **A `NaN` never matches a comparison**, for every operator including `!=`. A comparison is a
  question, and a `NaN` has no answer.
- **A `NaN` propagates through arithmetic**, and so does `inf`, and a division by zero is an
  infinity rather than an error. Arithmetic is a computation, and a `NaN` is a value.

A repository that got this wrong would either reject `NaN * 2` — losing rows a real system keeps
— or make `NaN = NaN` true — losing the strictness that the comparison kernel's whole
five-implementation discipline exists to guarantee.

## What is *not* in this increment

**There is no vectorised arithmetic.** The evaluation is a scalar loop over the rows that
survived the filter.

This is a deliberate scope decision, and the reason it is a *decision* rather than an oversight is
that the arithmetic is evaluated over the **survivors**, not over the file. Q6's predicate leaves
110,915 rows out of 6,000,000, so the arithmetic is 1.9% of the data — and the measurement says
exactly what it costs. If the arithmetic turns out to be the next bottleneck, the next increment
vectorises it, and that is a decision a measurement makes rather than a guess.

The alternative — writing the vectorised path now — would have meant adding an arithmetic family
to the kernel crate: a fifth implementation, a sixth, a seventh, the FFI seam widened, and the
constitution's principle VIII amended — all before a measurement said the arithmetic mattered.
The repository's method is the opposite: measure, then build.

## Consequences

**Good.**

- The engine runs **Q6 itself**, and the answer is checked against `DuckDB`'s on the real file.
- The aggregate operator is untouched, so its rules and its differential coverage are untouched.
- The expression surface is a **superset** of the old one: `Expr::column(..)` is what a column
  name was.
- The type rules are four lines long, because there are four operators.

**Bad, and accepted.**

- **`Aggregate::Sum` changed shape.** `Sum { column: String }` became `Sum { expr: Expr }`. It is
  a breaking change to a public type in a pre-1.0 repository, and every call site changed with
  it. The alternative — a second variant, `SumExpr` — would have left two ways to say the same
  thing and a `match` with an arm for each.
- **The evaluator panics on a type mismatch it cannot have.** A string column inside an
  expression is rejected by the plan, and a float column inside an *integer* expression is
  impossible because the type rules make the whole expression float. Both are `panic!`s with a
  message that says so. The alternative — returning an empty column — would have produced a
  **valid-looking answer** from an impossible state, which is worse than a loud failure.
- **The expression is not a general expression language**, and it is not trying to be. No
  comparisons, no `CASE`, no function calls, no casts, no subqueries.

**Neutral.**

- The filter's column is still not pushed down when the expression reads it. `l_discount` is
  both filtered and summed, so the engine filters it and the reader does not. That is spec 002's
  rule — a materialised column cannot also be pushed down — and changing it is its own piece of
  work with its own correctness argument.
- A floating-point sum is not associative, so the answer depends on the order the rows are
  summed in. The engine sums in the file's row order, so the answer is deterministic; the
  comparison against the incumbents is relative, as it already is for every floating-point figure
  in this repository.

## Alternatives considered

**Teach the aggregate operator to take an expression.** Rejected, and the reasoning is above: it
would have meant a second notion of value type in the accumulator, and the differential
coverage would have covered one of the two paths rather than both.

**Evaluate the expression over the whole file, before the filter.** Rejected. It is easier — the
expression would read the columns densely, with no selection — and it would be **wrong**: SQL
applies `WHERE` before the aggregate, so the expression is only computed for rows that survive.
It would also be 54× more arithmetic. The test that pins this is
`the_expression_is_evaluated_over_the_survivors`.

**Evaluate the expression inside the aggregate's row loop.** Rejected. It would avoid
materialising the computed column, but it would put the arithmetic inside the operator whose
rules the differential tests are there to protect, and it would make the expression's cost
invisible in the plan output.

**A vectorised arithmetic kernel now.** Deferred, deliberately, and the reason is above: the
measurement comes first.

## Revisit when

- The measurement says the arithmetic is a bottleneck. Then the kernel crate gains an arithmetic
  family, with the same five-implementation discipline the comparison kernels have.
- A query needs a `CASE`, a function call or a cast. Then the expression language grows, and the
  type rules grow with it.
- `MIN`, `MAX` or `COUNT` over an expression is needed. Then the aggregate's other functions
  take expressions too, and the question of whether *that* is still "a computed column" comes up
  again.
