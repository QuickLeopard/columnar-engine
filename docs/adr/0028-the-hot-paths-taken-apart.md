# ADR-0028 — The hot paths, taken apart

**Status**: accepted
**Date**: 2026-10-27
**Spec**: [spec 021](../../specs/021-the-hot-paths-taken-apart/spec.md)
**Predecessor**: [ADR-0027](0027-the-engine-joins-and-sorts.md)

## Context

Spec 020 published a hash join and a sort, and both were behind the incumbents
on the same two files, with the same SQL handed to every system:

| | engine | duckdb | polars | datafusion |
|---|---|---|---|---|
| `join-orders` cold | 216.1 ms | **97.0 ms** | 231.6 ms | 177.3 ms |
| `sort-topk` cold | 61.6 ms | **40.0 ms** | 1460.6 ms | 41.3 ms |
| `sort-topk` resident | 23.6 ms | **4.0 ms** | 1452.7 ms | 4.5 ms |

Spec 020's own lesson was that a benchmark answers one question at a time,
and it answered the wrong one: it proved the join was *correct* against
DuckDB, and it found the `SipHash` defect, and it never asked **where the
time went**. Both operators were written to be correct first and measured
second, and the measurement came after the code.

Spec 021 asks the question the plan output can answer. The join's stage
breakdown, from the published campaign set:

```
scan       6000000  6000000   79.6 ms
project    6000000  6000000    2.6 ms
join       6000000   216783  128.1 ms
aggregate   216783        1    0.3 ms
```

The scan is Arrow's decode, and the existing `materialised_column_probe`
already measured that this file's encoding is the better one — writing
`l_extendedprice` as a dictionary would be **slower**. So the scan is at the
floor of what the format allows, and the join stage is the engine's own code.

## The decision

**The engine's hot paths are decomposed by measurement, and the pieces are
tuned by measurement.** Specifically:

### 1. The join's table is addressed by the key, when the key allows it

The probe's cost is not the hashing. It is that a lookup into a hash table is
a **random access** into a structure of a few megabytes, and 96.4% of the
probe's six million questions have the answer "no".

The build side's keys are integers in a dense range. A table that is **one
byte a key**, indexed by the key itself, turns each of those questions into a
single load: no hash to compute, no chain to walk, and a byte of the table's
own memory rather than a 32-byte key in a `HashMap`.

Measured over six million probe rows against a table of 216,345 keys
(`crates/columnar-engine/examples/operator_probe.rs`), the two code versions
measured back to back on the same host:

| | before | after | |
|---|---|---|---|
| reading the key column | 9.3 ms | 9.4 ms | |
| reading the key column's own integers | 1.7 ms | 1.7 ms | the floor |
| **the engine's probe** | **102.7 ms** | **9.0 ms** | **11.4×** |
| an open-addressed table over the key's bits | 66.2 ms | 70.0 ms | |
| a table addressed by the key itself, 4 bytes a count | 44.0 ms | 42.9 ms | |
| **a table addressed by the key itself, 1 byte a count** | **16.4 ms** | **15.5 ms** | |

The direct table is `O(range)` rather than `O(keys)`, so it is used only when
the range fits `DIRECT_TABLE_LIMIT` (32 MiB), and a hash table takes over
otherwise. The key's **hash** is written by hand over the payload alone: the
derived hash for the key's enum hashes the discriminant as well, and the
discriminant is the same for every key of a type — a second round of
arithmetic over the same bytes, once for every row of the probe side.

The table's **width** comes from the file's own row-group statistics rather
than from the keys, because the estimate has to know the table's size before a
single row is read: the two must be made from the same number or the estimate
is not an upper bound. A key that falls outside the range the statistics claim
means the statistics are wrong, and the hash table takes over rather than the
engine answering wrongly.

### 2. A key's NULL check is hoisted out of the hot loop

The key reader asked the validity bitmap about every row, whether or not the
column held a single NULL. The filter operator already knew how to ask that
question; the key reader did not.

Measured on the key column, six million rows:

| | before | after |
|---|---|---|
| reading the key column | 9.3 ms | 1.7 ms |
| reading the sort's key column | 9.9 ms | 3.6 ms |

The sort's own work — the comparison, the heap — was 9.8 ms of its 13.3 ms.
**The sort was not slow because its comparison was slow.** It was slow
because it read its key through a bitmap that carried no information.

### 3. The rows behind the filter are the dataset's row numbers

The executor kept two numberings: the survivors' positions, and the dataset's
row numbers, and translated between them. The translation was a 24 MB vector
of positions on a six-million-row query — allocated, filled with `0..n`, and
thrown away — and a random access into it for every row the sort compared.

Every stage behind the filter now works in the dataset's own row numbers, so
a stage's output is a stage's input with nothing to translate.

### 4. The sort reads each key once, and compares it as a number

The unbounded sort read both keys of every comparison, and a comparison is
made `O(n log n)` times — so a row's key was read `O(log n)` times over. It
is the operator's only super-linear term, and it is gone.

The bounded sort also built a 24-byte `KeyValue` enum for every row, because
one of the enum's variants is a string. On a plain number column with no
NULLs the key is now read as the column's own value and compared as a number,
with no enum and no `Option`.

Measured: the engine's bounded sort went from **13.3 ms** to **3.7 ms**,
against a plain-read floor of **3.6 ms** — and it is now faster than the
hand-written bounded sort over the same plain read (**4.7 ms**), which is the
probe's cross-check.

### 5. The estimates are honest, and they are tested

The join's build side was charged the file's whole row count for a table
bounded by its distinct keys. A global aggregate was charged sixteen bytes a
row for a plan with **one group** — 91.6 MiB of phantom, on every eager plan.
The row-position vectors were counted nowhere.

The build side's estimate is now derived from the file's own **row-group
statistics**, and it is a bound rather than a guess: a row group whose own
range cannot satisfy a predicate holds no row that can satisfy it, so its
rows are not counted. The table it is charged for is the one the join will
build, decided from the same numbers the join decides it from — and every
estimate has a test that asserts a property rather than a number.

### 6. The plan output says what the engine did

The project stage in the ordered path reported the rows it was handed rather
than the rows it gathered: `project 6000000 6000000` for a projection that
gathered 216,783 rows. It now says `216783 216783`.

## A negative result, published

**A Bloom filter in front of the table does not help, and the reason is
worth more than the filter would have been.** With 216,345 keys the filter
needs about six million bits before it rejects anything at all: a filter with
fewer bits than keys is saturated, and a saturated filter rejects nothing. A
filter that is big enough to reject is a filter of 750 KB, and consulting a
750 KB filter is no cheaper than consulting the 6 MB table it guards. The
filter was built and measured — 8 KiB, 32 KiB and 256 KB, against the table
alone — and it is never better than the table alone: against the table's own
**71.0 ms** the 8 KiB filter reads **70.4 ms** and the 32 KiB filter
**71.3 ms**, the same number to within the run-to-run noise, and the filter
costs memory to hold.

The direct table beat it anyway, for a different reason: the direct table
does not consult anything, so there is nothing for a filter to guard.

## The instrument has to check its own answers

The probe's candidates are compared against the engine's own output, row for
row, before any of them is timed — and the check earned its place twice.

It caught the sort's new fast path **reversing its bounded heap's ordering**,
so the heap kept the best row and dropped the worst. The query still returned
a sum; it was just the wrong one: **500025.13** instead of **1049999.60**. The
differential tests caught it too, and the two campaigns that had been measured
before it was caught were thrown away and re-run.

It caught the probe's own direct-addressed table **answering for a key below
its base**: the index wrapped round to zero, so a key smaller than the
smallest build key was answered with the base key's count — two extra rows in
a 216,783-row answer.

The rule this gives the repository is simple and worth more than either fix:

> **An instrument that does not check its own answers measures a different
> operator than the one it claims to.**

## Consequences

- The join's probe is **11.4× faster** — 102.7 ms to 9.0 ms — and the sort's
  bounded sort is **3.6× faster** — 13.3 ms to 3.7 ms, at the plain-read floor.
- The join's table is **smaller** than the hash table it replaces, on the
  dataset measured: 6 MB against 10.8 MB, because a count is a byte and a
  hash-map entry is not.
- The direct table is `O(range)`. A join whose key is a 64-bit hash would
  allocate nothing sensible, so the range is checked and the hash table takes
  over. The decision is in one place, `direct_table_fits`, and the estimate
  and the builder use the same rule.
- The executor's memory is smaller: the identity `positions` vector is gone,
  and the translation between the two numberings is gone with it.
- The plan output now says what the engine did: the project stage in the
  ordered path reports the rows it **gathered**, not the rows it was handed.
- **The code's share of the published tables is measured, not asserted.** The
  old code was re-measured on the same host, back to back, in its own three
  campaigns: the join's cold figure is **216.1 ms** in the published set,
  **182.5 ms** for the old code on this host and **100.4 ms** for the new code,
  while the six queries the increment did not touch moved by **0% to +14.0%**
  in the cold lazy path. The plan output reads the same way: the join stage
  went from **107.4 ms** to **21.9 ms** and the sort stage from **14.5 ms** to
  **5.3 ms** on this host, while the two scan stages — Arrow's decode, which
  this change did not touch — moved **up** with the busier host. Every
  published figure — old and new — is derived from a committed report by
  `scripts/medians.py` and checked by `scripts/check-published-figures.py`.

## What this does not do

- **The scan.** The decode is Arrow's, and the counterfactual measured that a
  different encoding would be slower. Owning the decode is a different
  project. It is also where the join's remaining **7 ms** gap to DuckDB now
  is: the scan is 71.4 ms of the query's 100.4 ms, and the join stage is
  21.9 ms of it. The scan is also the one stage that did **not** benefit: it
  is 67.2 ms for the old code on the same host and 71.4 ms for the new code,
  which is the host's drift and not the code's.
- **The parallel read's second copy of every column.** It is real — 96 MB and
  a 2× peak — and it is off the measured path, because the published
  comparison pins `read threads: 1`.
- **Reading the aggregate's column only after the join has decided.** The scan
  materialises `l_extendedprice` for all six million rows to use 216,783 of
  them. That is the stage order, and changing the stage order is a different
  increment. The number is recorded in the spec so it is not forgotten.
- **A parallel join or sort.** The headline comparison is pinned to one
  thread, and the incumbents are pinned to one thread beside it.
- **A Bloom filter in front of the table.** Measured, and refused, with the
  reason published above.
