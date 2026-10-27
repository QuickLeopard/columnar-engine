# Spec 020 — The full engine increment: hash join and sort

**Status**: done
**Started**: 2026-10-27
**Finished**: 2026-10-27
**ADR**: [ADR-0027](../../docs/adr/0027-the-engine-joins-and-sorts.md)
**Predecessor**: [019-the-button-always-answers](../019-the-button-always-answers/spec.md)

## The problem

Spec 001's out-of-scope table has one line that is a promise rather than a decision:

| Excluded | Reason |
|---|---|
| Hash join and sort | The Q6 path needs neither. They are in the "full engine" increment. |

Everything else in that table is a decision with a reason behind it — a SQL parser is "a month of
work that produces no evidence", a decimal type would be "a claim, not evidence", GPU acceleration
is "CPU-only by design", nested types are "not present in the TPC-H schema". **Hash join and sort**
are the only two entries the repository promised to build, and they are still not built.

The cost of not building them is not abstract. The engine's published comparison covers **six
queries, all of them single-table**: `scan`, `filter-numeric`, `filter-string`, `filter-range`,
`q6`, `q6-revenue`. A reader who asks "can this engine join?" has to read the answer in the
out-of-scope table of a spec from the project's first week. Every published number is a number
about a **single table**, which is the easy half of a query engine.

And there is a second, quieter cost: **the plan's pipeline is fixed at four stages**.

```text
scan -> filter -> project -> aggregate
```

There is no stage that can bring two tables together, and no stage that can put the rows in an
order. The aggregate is the last stage and the output, which means a query that wants the *rows*
-- sorted, limited -- cannot be expressed at all.

So this is the increment the repository has been deferring since spec 001, and it is deferred no
longer.

## The requirements

### R1 — The dataset can be joined

- [ ] `columnar-testdata` generates an **`orders`** table beside `lineitem`: TPC-H's `orders` at
      SF-1, 1,500,000 rows, with `o_orderkey`, `o_custkey`, `o_orderstatus`, `o_totalprice`,
      `o_orderdate`, `o_orderpriority`, `o_clerk`, `o_shippriority`, `o_comment`.
- [ ] The join key is **consistent**: `o_orderkey` is drawn from the same key space as
      `l_orderkey`, so the join has a real selectivity instead of being empty or universal. The
      manifest states the key space and the resulting match rate.
- [ ] The generator stays a pure function of `(scale_factor, seed, null_rate)`: the same three
      numbers always produce the same rows, in the same order, with the same bits. The `orders`
      table does not perturb `lineitem`'s rows.
- [ ] `just dataset` writes both files, and the manifest records both.

### R2 — The engine joins

- [ ] An **inner equi-join** on one key column, executed as a **hash join**: the right table is
      the build side, read and hashed; the left table is the probe side, and it stays on the lazy
      path with its predicates pushed down.
- [ ] The build side's own predicates are evaluated **before** the hash table is built, so the
      hash table holds only the rows that can match.
- [ ] A NULL key never matches, on either side — SQL's rule, not a convenience.
- [ ] The join's result is **identical to a brute-force nested-loop reference**, row for row, in a
      differential test. The reference is slow and obviously correct; the hash join is fast and
      checked against it.
- [ ] The join's row count and its aggregate are reported in the plan output, so the reader sees
      how many rows the join produced.

### R3 — The engine sorts

- [ ] `ORDER BY <column> [DESC] [LIMIT n]`, as a stage in the pipeline.
- [ ] With a limit, the sort keeps a **bounded heap** of `n` rows rather than sorting everything:
      the published query asks for ten rows out of six million.
- [ ] The sort's result is identical to a reference sort, in a differential test.
- [ ] A plan with a sort and an aggregate aggregates over **the sorted rows**, not over all of
      them: the order of the stages is part of the plan, not an accident.

### R4 — The plan says what it does

- [ ] The pinned optimisation order gains the two new stages, and the constant that holds it is
      printed at the top of every plan output, as it is today.
- [ ] The plan output names the join's key columns, the build side's file and the sort's key and
      limit.

### R5 — The comparison publishes the two new queries

- [ ] Two new queries, measured **against the same three incumbents** on the same files, with
      **the same SQL** handed to every system:

      | name | what it is |
      |---|---|
      | `join-orders` | `lineitem` joined to `orders` on the order key, with a date range on the build side, aggregated over the whole join |
      | `sort-topk` | the ten largest `l_extendedprice` values, aggregated over the sorted rows |

- [ ] The answers are **cross-checked** against the incumbents' with the harness's existing
      relative tolerance, and a disagreement is a warning in the report, not a silent difference.
- [ ] The two queries are published in §2a (resident) and §2b (cold) as the **median of three
      full campaigns**, with the spread, and the numbers are derived by `scripts/medians.py` and
      checked by `scripts/check-published-figures.py`.
- [ ] **A figure that cannot be made fairly is named, not published.** The join's **build side is
      a file the engine reads**: in cold mode, inside the query, which is like-for-like with the
      incumbents reading the same file inside their query. In resident mode the incumbents have
      both tables in memory before the clock starts, so the engine's resident figure is published
      **only if the engine's build side is in memory before the clock starts too**. If it cannot
      be, the reason the figure is withheld is stated in the table — the same rule the repository
      applies everywhere else: a number that is not like-for-like is not a number that gets
      published next to one that is.
- [ ] The **incumbents get the second table too**: `DuckDB` reads both files by path in cold mode;
      `Polars` and the `DataFusion` peer register both tables under their names in resident mode.
      A comparison where the engine joins and the incumbent does not is not a comparison.
- [ ] Where a published figure moves because of this campaign set, **both numbers are published**
      and the reason is named.

### R6 — The evidence is checkable

- [ ] The differential tests for the join and the sort are in the tree, and they run in CI.
- [ ] The published figures come from committed raw reports, and the guard fails if the prose
      drifts away from them.

## Scope

**Out of scope, with the reasons:**

- **Outer, semi and anti joins.** The increment's join is an inner equi-join. The others are
  different operators with different semantics, and no published query needs them. An outer join
  that is never measured is a claim.
- **Multi-key joins.** One key column. A composite key is a different hash table, and the
  published query does not need one.
- **`GROUP BY` over a join.** The published join query aggregates over the **whole** join, so its
  answer is a single number and the harness's existing cross-check applies unchanged. A grouped
  join's answer is a *set* of groups, and comparing two sets of groups across four engines is a
  different piece of machinery -- worth building, but not worth building in the same increment as
  the join itself.
- **A sort that spills to disk.** The sort is in memory. The memory budget rejects a plan whose
  working set does not fit, which is the same answer the rest of the engine gives.
- **A SQL parser.** Unchanged from spec 001: a month of work that produces no evidence about the
  part of this project that is under test.
- **Window functions.** They need a sort and a partition, and they are in none of TPC-H's first
  twenty queries. They follow the sort, once the sort exists.
- **Morsel-driven parallelism.** Unchanged, with spec 005's reason: the operators are
  single-threaded, and the headline comparison pins every system to one thread.
- **A decimal type and a date type.** Unchanged: TPC-H's decimals are `Float64` and its dates are
  `Utf8` in `YYYY-MM-DD`, and the manifest says so. A richer type system than is implemented
  would be a claim rather than evidence.
- **GPU acceleration and nested types.** Unchanged, with spec 001's reasons: the project is
  CPU-only by design, and nested types are not in the TPC-H schema.

## References

| principle | how this spec serves it |
|---|---|
| I. One Predicate, Six Implementations, One Answer | The join and the sort are checked against brute-force references, row for row. |
| II. The Answer Is the Same | The new queries are cross-checked against three incumbents with the same SQL, so the comparison is a comparison. |
| IV. Evidence over claims | The two new rows in §2a and §2b are measured, published with their spread, and derived by a checked-in script. |
| V. The Comparison Is Reproducible by a Stranger | The `orders` table is generated by the repository's own generator, and the SQL the incumbents run is the SQL in the source. |
| VI. The Dataset Is Named Honestly | The manifest records the second table, its key space and the join's match rate. |
| VII. Explicit Scope, No Silent Deferral | The nine exclusions above, each with its reason. |
