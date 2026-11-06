# Spec 047 — The build side's keys as a filter

**Status**: done
**Started**: 2026-11-06
**Finished**: 2026-11-06
**Predecessor**: [spec 046 — the join-orders query, on both sides, taken apart](../046-the-join-orders-query-taken-apart/spec.md)

**CI green on `67b6d33`** -- run 42260740267, sixteen of sixteen jobs.

## The problem

**The engine's cold `join-orders` is behind `DuckDB`'s, and spec 046 named the reason: `DuckDB` joins
**216,783** rows and the engine joins **6,000,000`.**

`DuckDB` pushes the build side's keys into the probe side's scan as a filter — its own **perfect-hash
filter** over the build side's key set — so the rows that reach its join are the ones that can match.
The engine has no such filter: its probe looks up every row the reader hands it.

**And the engine's lookup is the cheaper of the two.** `DuckDB`'s own join, over the same six million
rows, costs **+44.190 ms** more than the engine's ([§3ac](../../PERF.md#3ac-the-join-orders-query-on-both-sides-taken-apart)).
So the engine is not behind because it looks up slowly. **It is behind because it looks up every row
where `DuckDB` tests a bit.**

**And the filter's cost is the prize's other half.** `DuckDB`'s filter costs its scan **5.827 ms** over
six million rows. If a filter cost the engine the same, the engine's **14.625 ms** join would leave
**8.798 ms** on the table.

### Why this is not the filter spec 021 refused

**Spec 021 built "a bitmap in front of the table" and measured it at three sizes — 8 KiB, 32 KiB and
256 KiB — and it was never better than the table alone** ([§2h](../../PERF.md#2h-the-hot-paths-taken-apart)).

**Those sizes are saturated**, and the house said so at the time: *"with 216,345 keys a filter needs
about six million bits before it rejects anything at all — a filter with fewer bits than keys is
saturated, and a saturated filter rejects nothing."*

**So the filter that rejects was never measured.** The bitmap the size of the **key's own range** — one
bit a key, **six million bits**, three quarters of a megabyte — is a different filter from the three
that were tried, and it is the only shape of "a bitmap in front of the table" that rejects anything at
all on this data.

**And there is a second reason it might be worth a look now.** The engine's table is **one byte a key**
and the filter would be **one bit a key**: the same membership test, **eight times less memory** to
touch, over the same range, with **no hash** — the direct-addressed table consults nothing, and a filter
that consults nothing has nothing to gain.

### The hypothesis, and it is a hypothesis

**A filter one bit a key, over the same range the table already addresses, rejects a probe row for
less than the table's own load costs, and the probe gets faster.**

**If it does not, the increment is a refusal**, and the refusal is published with the numbers that
refused it — the house's own record has five of them already, and this would be the sixth.

## The requirements

### R1 — The filter is a bitmap over the key's own range, one bit a key

- [ ] The filter covers **the same range the direct-addressed table addresses**: the key column's own
      statistics, so the estimate can be made before a row is read and the two are the same number.
- [ ] **One bit a key**, so the filter is **an eighth** of the table's size.
- [ ] **No hash.** The table is addressed by the key itself and consults nothing; a filter that had to
      compute a hash would pay for one the table does not.

> **Not done, and not to be done.** The increment was refused on the measurement, and the requirement is
> the shape the refused increment would have had. The shape was **prototyped and measured** anyway --
> the instrument builds the filter over the build side's keys, one bit a key over the key's own range,
> with no hash -- because a refusal without a measured alternative is an argument. **The filter's own
> load is 5.222 ms over the six million rows, against the table's own load of 4.399 ms: the filter's test
> costs more than the lookup it would replace**, because both are one random access and the bitmap's test
> carries a shift and a mask on top of the access the byte does not. See
> [ADR-0059](../../docs/adr/0059-the-build-side-s-keys-as-a-filter.md) and `PERF.md` §3ad.

### R2 — The filter is consulted **before** the table, and a rejected row never touches the table

- [ ] The filter answers one question: **may this key be in the build side at all?**
- [ ] A row the filter rejects does not read the table's byte, does not consult the table's overflow map,
      and does not push an output row.
- [ ] The table still decides. **The filter is a filter, not a replacement.**

> **Not done, and not to be done**, for the same reason as R1: the filter is consulted before the table
> in the instrument's candidate, and the candidate is the engine's own probe **line for line**, and it
> **is a loss**: **+2.318 ms** at the median, in eleven of its twelve paired samples. The engine's probe
> beside it reads **7.622 ms** against the candidate's **7.967 ms**, so the control says the candidate is
> the engine's probe.

### R3 — The filter can only be wrong in one direction

- [ ] **Every key in the build side sets its bit**, so a key that **is** in the build side always passes.
- [ ] A key that is **not** in the build side may share a bit with one that is, and pass. **A pass is not
      a match**: the table decides, and the answer is the table's answer.
- [x] **A false negative is impossible**, and the test that says so is in the suite: the filtered probe's
      rows are compared with the unfiltered probe's **row for row, in order** -- and the comparison runs in
      **every iteration of every campaign**, over the engine's own probe's rows, and it passed in all
      eighteen.

### R4 — The same rows, in the same order, and the same answer

- [x] The filtered probe returns **exactly** the rows the unfiltered probe returns, in the same order.
- [x] The query's answer is unchanged, and the published join's answer is checked before anything is timed.

### R5 — The ladder measures the filter, and the filter is measured apart

- [x] **Two shapes**, not one:
  - the filter **in front of the table** -- a row the filter refuses does not read the table's byte;
  - the filter **compacting the rows** in front of the probe -- the survivors alone are probed, and the
    probe's own work is not done for the rows the filter refused.
- [x] The **filter's own floor**: the bitmap's load, one a probe row.
- [x] The **table's own floor**: the byte's load, one a probe row. **The two are the same kind of work** --
      one random access -- and the ladder says which of the two costs more: **the filter's, at 5.222 ms
      against the table's 4.399 ms.**
- [x] The **constructions**: the table's own construction, and the filter's own construction. **A filter's
      price is its test and the building of it.**
- [x] The **prize**: the probe over the survivors alone, against the probe as it is -- **+6.981 ms**.
- [x] The **control**: the candidates are the engine's own probe, **line for line**, and the engine's own
      probe is measured **in the iteration**, beside them.
- [x] The **cross-check**: the candidates' rows are compared with the engine's own probe's rows, **row for
      row, in order**.
- [x] Every rung is measured **in the iteration**, and the two sides of every difference are measured **back
      to back with the order alternated**. The six orders are chosen so that **every pair** is back to back
      in four of the six iterations, two in each direction: a rotation of three would have put one pair back
      to back four times and another twice.

### R6 — The memory estimate charges the filter

- [ ] The filter's bytes are **the estimate's**, and the estimate is charged for them before a row is read.
- [x] **If the filter does not pay, it is not built**, and the estimate does not move. **It does not pay**,
      and **the estimate does not move**: the increment's diff on the engine is **empty**.

### R7 — The answer is published either way

- [x] **A win** is published old-and-new, with the reason, and the open question is rewritten.
- [x] **A refusal** is published with the numbers that refused it, and the open question is rewritten to
      name what the refusal leaves standing.
- [x] The figures are derived from the committed reports and checked by the guard **inside the section that
      publishes them**.

## The scope

- **In**: the **direct-addressed** table — the shape the published file's join uses — and the cold
  path's probe over it.
- **Out**: the **hashed** table. Its lookup computes a hash already, and a filter there is a different
  question; the increment is about the one that consults nothing.
- **Out**: the **resident** path's probe, except as the cross-check's other side.
- **Out**: `DuckDB`'s filter. It is measured as `DuckDB` measures it, and it is not copied.
- **Out**: any change to the plan's shape. **The surface does not move**: the filter is the join's own
  business, and the rows are the rows.

## The instrument

**`crates/columnar-engine/examples/key_filter_probe.rs`** -- the increment's own probe.

It is **not** `crates/columnar-engine/examples/join_cost_probe.rs`, the house's join probe: that one is a
campaign instrument for the join's **stage**, with its own report format and its own ladder, and it has
no room for a **paired verdict between three shapes measured back to back in the same iteration** --
which is what this question is. The amendment is recorded below.

## The references

- [ADR-0059](../../docs/adr/0059-the-build-side-s-keys-as-a-filter.md) — the decision: the filter is one
  bit a key over the range the table already addresses, it is consulted before the table, and it is
  removed if it does not pay.
- [PERF.md §3ac](../../PERF.md#3ac-the-join-orders-query-on-both-sides-taken-apart) — the parts, on both
  sides, and the counterfactual that named the filter.
- [PERF.md §2h](../../PERF.md#2h-the-hot-paths-taken-apart) — spec 021's bitmap, and the arithmetic that
  says why the small ones could not reject.
- [PERF.md §3o](../../PERF.md#3o-the-joins-stage-taken-apart) — the join's stage, taken apart.
- [constitution.md](../../constitution.md), principle **IV — Evidence Over Claims**: a number is a
  claim about a measurement, and a claim without its evidence is not published.
- [constitution.md](../../constitution.md), principle **II — A Negative Result Is a Result**: the
  refusal and the numbers that refused it are published as the finding.

## The amendments

### A1 — The instrument is its own file, and it is in `columnar-engine`

**Why**: the hypothesis is about the **shape of the lookup**, not about the join's stage, and the
verdict is a **paired difference between three shapes measured back to back in one iteration**.
`join_cost_probe.rs` is the house's instrument for the join's **stage**: it runs the engine's own plan
through the engine's own entry point, takes the stage from the plan output, and reports the stage's own
parts. Its report format carries one ladder and the context's differences; it has no place for a second
and a third candidate, and no place for a difference between two of them measured in the same iteration.

The instrument is in `columnar-engine` rather than `columnar-bench` because **the other side of the
comparison is not a subprocess here**: the question is what a shape costs *inside this engine*, and the
engine's own probe is the control. `columnar-bench` is where the engine and the incumbents are measured
by one harness, and there is no incumbent on the other side of this question.

### A2 — The ladder measures **two** shapes, not one

**Why**: the hypothesis names one shape -- the filter in front of the table -- and the measurement
refused it. But a refusal of **one** shape is not a refusal of the **idea**, and the shape that the
prize is actually about is the one where the filter keeps the rows out of the probe's own work: the
filter **compacting the rows**, and the survivors alone probed. A ladder that measured only the shape
the hypothesis named would have refused the hypothesis and left the question open; a ladder that
measures both answers the question.

