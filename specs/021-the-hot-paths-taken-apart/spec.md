# Spec 021 — The hot paths, taken apart

**Status**: in progress
**Started**: 2026-10-27
**ADR**: [ADR-0028](../../docs/adr/0028-the-hot-paths-taken-apart.md)
**Predecessor**: [020-the-engine-joins-and-sorts](../020-the-engine-joins-and-sorts/spec.md)

## The problem

Spec 020 published two new operators and measured both against the incumbents. Both were
behind, and the gap was not small:

| | engine | duckdb | polars | datafusion |
|---|---|---|---|---|
| `join-orders` cold | 216.1 ms | **97.0 ms** | 231.6 ms | 177.3 ms |
| `sort-topk` cold | 61.6 ms | **40.0 ms** | 1460.6 ms | 41.3 ms |
| `sort-topk` resident | 23.6 ms | **4.0 ms** | 1452.7 ms | 4.5 ms |

The engine was **2.2× behind DuckDB on a hash join it wrote five weeks ago**, and **5.9× behind
it on a top-k sort**. Both operators are new code, and both were written to be *correct* first
and measured second. The measurement has now been taken apart, and what it found is not a
missing optimisation or a missing SIMD path. It is **the shape of the code**.

The probe the increment builds (`crates/columnar-engine/examples/operator_probe.rs`) takes the
join's probe and the sort apart, and the parts are these:

**The join's probe, over 6,000,000 rows, against a table of 216,345 keys:**

| | ms | |
|---|---|---|
| reading the key column | 9.0 | the floor the old code paid |
| **reading the key column's own integers** | **1.6** | **the real floor** |
| **the engine's probe, before** | **91.1** | |
| an open-addressed table over the key's bits | 59.9 | 1.53× |
| **a table addressed by the key itself** | **14.4** | **6.33×** |

**The sort, over 6,000,000 rows, with a limit of ten:**

| | ms | |
|---|---|---|
| reading the key column | 9.0 | the floor the old code paid |
| **reading the key column without the validity bitmap** | **3.1** | **2.9×** |
| **the engine's bounded sort, before** | **11.8** | |

So the engine's own work — the part that is not Arrow's decode and not the file — was **89 ms of
the join's 103 ms probe** and **9.8 ms of the sort's 13.3 ms**. The join's probe is where the
engine's time is. The sort is not slow because its comparison is slow; the sort is slow because
it reads the key column through a validity bitmap that carries no information.

And there is a second kind of cost, which does not show up in a timing at all: **the engine's
own memory estimates are wrong in both directions, and its plan output misreports what it did.**

| | |
|---|---|
| the join's build-side estimate | charges the file's whole row count (1,500,000) for a table bounded by the distinct keys (216,345) — and under-charges the worst case by 2.4× |
| the eager path's global aggregate | charged 16 bytes a row for a plan with **one group** — 91.6 MiB of phantom, on every eager plan |
| the row-position vectors | 48 MB of `Vec<u32>` on every join and sort query, and **not counted anywhere** |
| the `positions` vector | 24 MB allocated, filled with `0..n`, and thrown away on every join query |
| the project stage's row count | reported as 6,000,000 when the project gathered 216,783 |

## The requirements

### R1 — The join's probe resolves its key once, and looks it up in a numeric table

- [x] The probe's key type is resolved **once**, before the loop, not per row: the plan already
      requires the two sides to have the same type, so the type is a property of the plan.
- [x] An **integer** join key is looked up in a table of the key's own bits, not in a
      `HashMap` over a 32-byte enum. The enum's derived `Hash` costs two hash rounds a row and
      its 32 bytes are stored for every key, whether the key is a number or a string.
- [x] When the build side's keys lie in a **dense integer range that fits the budget**, the table
      is addressed by the key itself: no hash, no probe chain, one load. When the range is wide
      or the key is not an integer, the table is an open-addressed hash table.
- [x] The build table reserves its capacity, so it does not rehash its way to its final size.
- [x] The probe's cost is **measured**, not estimated: the probe decomposes it and the numbers
      are published.

  Measured over six million probe rows against a table of 216,345 keys, the two code versions
  back to back on the same host:

  | | before | after | |
  |---|---|---|---|
  | reading the key column | 9.3 ms | 9.4 ms | |
  | reading the key column's own integers | 1.7 ms | 1.7 ms | the floor |
  | **the engine's probe** | **102.7 ms** | **9.0 ms** | **11.4×** |
  | an open-addressed table over the key's bits | 66.2 ms | 70.0 ms | |
  | a table addressed by the key itself, 4 bytes a count | 44.0 ms | 42.9 ms | |
  | **a table addressed by the key itself, 1 byte a count** | **16.4 ms** | **15.5 ms** | |

### R2 — A key's NULL check is hoisted out of the hot loop

- [x] A column that holds **no NULL at all** does not have its validity bitmap read, once per
      row, in the join's probe or in the sort. The filter operator already knows how to ask
      whether a column has NULLs; the key reader did not.
- [x] The check is still made where it is needed: a column **with** NULLs keeps the check, and a
      NULL key still never matches and still sorts last.
- [x] The probe's fast path reads the key column's **own integers** — no enum, no validity
      bitmap — and the general path is kept for a column that holds NULLs or is
      dictionary-encoded.

  Measured: reading the key column went from **9.3 ms** to **1.7 ms**, and the sort's key read
  from **9.9 ms** to **3.6 ms**.

### R3 — The sort reads each key once, and compares without an enum

- [x] The direction of the sort is resolved **before** loop, so the comparison does not branch
      on it per row.
- [x] A key's comparison does not match on an enum per row, and does not carry an `Option` per
      row — on the **fast path**, a plain number column with no NULLs, where the key is read as
      the column's own value and compared as a number. The general path (a column with NULLs, or
      a dictionary-encoded one) still carries the enum, and the measurement says that is the
      right trade: the sort's own work is now a tenth of a millisecond of its 3.7 ms.
- [x] The sort **without** a limit reads each key **once**. It read it `2n·log n` times: the
      comparator calls the key accessor twice per comparison. The rows are decorated with their
      keys before the sort begins.

  Measured: the engine's bounded sort went from **13.3 ms** to **3.7 ms**, against a plain-read
  floor of **3.6 ms** — and it is now faster than the hand-written bounded sort over the same
  plain read (**4.7 ms**), which is the probe's cross-check.

### R4 — The row positions the join and the sort carry are the dataset's row numbers

- [x] The identity `positions` vector — `0..n`, 24 MB on a six-million-row query — is gone.
- [x] The probe and the sort emit **dataset row numbers**, so the aggregate gathers from them
      directly and the sort's key lookup loses one dependent load per row.
- [x] The translation between the two numberings is gone with them.

### R5 — The estimates are honest, and they are tested

- [x] The join's build side is charged by the **rows that can survive the build filter**, bounded
      from the filter column's **row-group statistics**: a row group whose own range cannot
      satisfy a predicate holds no row that can, so its rows are not counted. The table it is
      charged for is the one the join will build, decided from the same numbers the join decides
      it from.
- [x] A **global** aggregate is charged by its **groups**, not by the rows, on the eager path as
      well as the lazy one.
- [x] The row-position vectors are counted, because they are memory the engine holds.
- [x] Every estimate has a **test**, and every test asserts a **property** rather than a number:
      `crates/columnar-engine/tests/memory_estimates.rs`.

### R6 — The plan output says what happened

- [x] The project stage in the ordered path reports the rows it **gathered**, not the rows it
      was handed. The plan output said `project 6000000 6000000` for a projection that gathered
      216,783 rows; it now says `216783 216783`.

## The instrument

- [x] `crates/columnar-engine/examples/operator_probe.rs` takes the join's probe and the sort
      apart, and **checks every candidate against the engine's own output, row for row, before
      it is timed**. A candidate that is wrong is not a candidate.
- [x] The check is **real**, and it earned its place: it caught the sort's new fast path
      reversing its bounded heap's ordering — the query answered **500025.13** instead of
      **1049999.60** — and it caught the probe's own direct-addressed table answering for a key
      below its base. Both were fixed before anything was published.
- [x] A **Bloom filter** in front of the table was built and measured, and it does not help. The
      reason is published: with 216,345 keys the filter needs about six million bits before it
      rejects anything, and a filter that large is no cheaper to consult than the table it
      guards. **A negative result, published.**

## The measurement

- [x] Three campaigns on a host quiet enough to measure on (load 3.60–5.86), the reports
      committed, and the published tables re-derived by `scripts/medians.py`.
- [x] **The old code was re-measured on the same host**, back to back, in its own three
      campaigns, so the code's share of every moved figure is separated from the machine's. The
      six queries this increment did not touch are the control: they moved by **−0.7% to +8.1%**
      between the two host-matched sets, while the join moved by **−45.6%** and the sort by
      **−15.9%**.
- [x] The set before that is committed too, so **every** published figure — old and new — is
      derived from a committed report by a script, and the guard checks all four sets.
- [x] The moved figures published old-and-new, with the reason.

## The gate

- [x] `cargo fmt`, clippy on arm64 and x86-64, the whole test suite (330 tests), `cargo deny`,
      the wasm32 check, the published-figures guard and the page checks all green.
- [x] CI is green on the pushed commits.

## Scope

**Out of scope, with the reasons:**

- **The scan's Arrow decode.** The existing `materialised_column_probe` measured that writing
  `l_extendedprice` as a dictionary instead of plain would be **slower**, and the engine's own
  cost on top of Arrow's decode is one `memcpy` of 4–5 ms a column. Owning the decode is a
  different project, and the repository has already measured that it does not pay.
- **The parallel read's second copy of every column.** It is real — 96 MB and a 2× peak — but
  the published comparison pins `read threads: 1`, so it is off the measured path.
- **Reading the aggregate's column only after the join has decided.** The scan materialises
  `l_extendedprice` for all 6,000,000 rows to use 216,783 of them: 48 MB resident to use 1.7 MB.
  That is a consequence of the stage order, and changing the stage order is a different
  increment. The number is recorded here so it is not forgotten.
- **Parallelising the join or the sort.** The headline comparison is pinned to one thread, and
  the incumbents are pinned to one thread beside it. A parallel join would move a number that
  is not published.
- **A grouped join, a `GROUP BY` over a join, and window functions.** Unchanged from spec 020,
  with spec 020's reasons.
- **A Bloom filter in front of the table.** It was built and measured: with 216,345 keys the
  filter needs about six million bits before it rejects anything, and a filter that large is
  no cheaper to consult than the table it guards. **A negative result, and the reason it is
  negative, are published.**

## References

| principle | how this spec serves it |
|---|---|
| I. One Predicate, Six Implementations, One Answer | The probe's candidates are checked against the engine's own output, row for row, before any of them is timed — and the check found two real defects. |
| II. The Answer Is the Same | The join and the sort keep their answers: every change here is checked by the differential tests, and the published queries are cross-checked against three incumbents. |
| IV. Evidence over claims | Every claim in this spec is a measurement, taken by a probe that is committed and re-runnable. |
| VI. The Dataset Is Named Honestly | The estimates stop charging the file's row count for a table bounded by its distinct keys. |
| VII. Explicit Scope, No Silent Deferral | The six exclusions above, each with its reason and, where there is one, its number. |
