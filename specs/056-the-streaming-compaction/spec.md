# Spec 056 — The streaming compaction

**Status**: planned
**Started**: 2026-11-18
**Predecessor**: [spec 055 — the embedded database](../055-the-embedded-database/spec.md)

## The problem

**The compaction is refused above six million rows, and the measurement that found it is published.** The probe bisected
for the ceiling and found it: **5,375,000** rows compact, **6,000,000** rows are refused with *"the engine refused: the
estimated working set exceeds the query memory budget"* ([PERF.md §3ak](../../PERF.md#3ak-the-databases-own-price)). **And
the cause is the shape of the read and not a knob**: `compact()` hands the table's **whole** file list to one query plan,
the engine estimates that plan's working set against its **256 MiB** budget, and the plan is refused before a byte is read
([`crates/columnar-db/src/database.rs`](../../crates/columnar-db/src/database.rs), the `live()` call;
[`crates/columnar-engine/src/memory.rs`](../../crates/columnar-engine/src/memory.rs), the budget).

**And that is not the only ceiling.** The compaction also materialises the table's live rows **in the database's own
memory** — every value of every column, in `Live` — and builds every Arrow batch at once. **So where the engine's estimate
fits, the database's own peak is still the table's size.** Two ceilings, one shape: **the compaction reads the table in one
piece.**

**And the compaction is the only thing that pays the tombstone tax back.** The crate says so in its own words: *"The
compaction is what pays the tombstone tax back, and nothing else does"* ([`versions.rs`](../../crates/columnar-db/src/versions.rs)).
**So a table that grows past the ceiling can never be compacted**, and its reads keep paying the tax — **0.115 ns** a
retired version a row, **every read and not only the compaction**, so the table reads in **13,515.470 ms** where it read in
**44.446 ms** before a fifth of its key range changed. **The database has a size limit above which it gets slower and
cannot be reclaimed.**

**And the output file's schema is derived from the data, which a streaming compaction cannot do.** The Parquet writer's
schema is fixed before the first row, and the data's shape is known only after the last one — and the store derives it:
`rows::schema` writes a column **required** when the rows hold no NULL and **optional** when they hold one
([`crates/columnar-store/src/rows.rs`](../../crates/columnar-store/src/rows.rs)). **So two files of one table can disagree
about a column's nullability**, and **the engine's own file-set check does not compare nullability**: it compares the
column count, the names, the types and the storage, and not that
([`crates/columnar-engine/src/file_set.rs`](../../crates/columnar-engine/src/file_set.rs), `check_schemas`). **So the
streaming compaction cannot be written until the output schema is decided by something other than the data.**

**And the store's surface is about to be published.** The crates go to the registry at **0.1.0** and the surface is frozen
there ([spec 054](../054-the-crates-are-published/spec.md)), so **the nullability has to be settled before the publish and
not after it** — a change to a published surface is a break in every embedder's build.

## The requirements

### R1 — The compaction is **streaming**: file by file, one file out

- [ ] The compaction reads **one file at a time** — one query plan a file — so the engine's estimate is a fraction of the
      table's and **the budget stops being the table's ceiling**.
- [ ] **And the database's own memory is bounded by one file's live rows**, and not by the table's: the rows in flight are
      the file's, and the batches written are flushed as they fill.
- [ ] **And it writes exactly one file**: the manifest ends up naming one file and nothing else, because that is the
      promise the compaction makes — the retired versions stop existing, and the table's versions are renumbered from zero.
- [ ] **And the engine's surface is not widened**: a file set of one is a file set, and the store's writer takes batches as
      they come. **And the budget is not raised** — the streaming compaction fits inside it by reading less at a time.

### R2 — The **output schema is decided**, and it is the table's schema

- [ ] Every file the database writes — **the append path and the compaction alike** — takes its Arrow schema from **the
      table's schema**: a column the table declares nullable is written **optional**, one it declares not nullable is
      written **required**, and the hidden `row_id` and `deleted` columns are plain and never NULL.
- [ ] **And the store's row-writing API grows the nullability as an explicit input** — the caller decides and the store does
      not derive it from the data — because a streaming write's schema is fixed before the first row and the data's shape is
      known only after the last one. **And the store's surface change is deliberate and recorded**, because the store
      publishes at 0.1.0.
- [ ] **And the bytes change, and the change is published**: the old figure and the new one are both given, with the reason
      named, because an optional column that holds no NULL still writes its definition levels.

### R3 — A file whose schema disagrees with the table's is **refused**, and the refusal **names the file and the column**

- [ ] The compaction checks each file's schema against **the table's schema** — including the nullability the engine's own
      check does not compare — because the compaction is the moment a disagreement would be inherited, and so the moment to
      catch it.
- [ ] **And the refusal names the file and the column**, because a refusal that does not say which file is not a refusal an
      application can act on.

### R4 — The **price** is measured and published, and the ceiling is measured again

- [ ] The streaming compaction's cost a row — **clean and taxed** — is measured **beside** the one-shot compaction's cost,
      **back to back, on the same table, at the same moment**, so the machine's movement cannot land on one side of the
      difference and not on the other.
- [ ] **And the ceiling is measured again**: the row count at which the compaction is refused, **before and after** — and the
      table the probe already refuses at **6,000,000** rows is the table the streaming compaction has to compact.
- [ ] **And the tax's total is measured**: the prediction is that streaming does not change the tax's total work and only
      its peak. **The measurement says whether it does, and a prediction that fails is published as a defect.**
- [ ] **And the figures are published**: three campaigns, interleaved, the raw reports committed, the tables derived by
      `scripts/medians.py`, and the guard checking them **inside the section that publishes them**.

### R5 — The record

- [ ] A **new section** in `PERF.md`, the decision in an **ADR**, the defect table's row, and the crate's own README.
- [ ] **And `STATUS.md`'s open question 8 moves** — the compaction's ceiling is answered, and what it cost is published.

### R6 — The closure

- [ ] The spec's status is `done` with the date it finished, and CI is green.

## The scope

- **In**: the streaming compaction, the decided output schema, the store's nullability as an input, the refusal of a file
  that disagrees, the prices, and the record.
- **Out**: **the tombstone tax's shape** — named out with the reason (R4): streaming does not change the tax a read pays, it
  only makes the compaction able to run at all. **The tax's three ways out are three architectures** — the database filters
  the retired versions itself, the engine grows a *"not one of these"* predicate, or the compaction runs by itself — and the
  measurement does not pick between them. That question is open and stays open.
- **Out**: **the index** — its shape is decided by the scan's figure
  ([ADR-0066](../../docs/adr/0066-the-databases-own-price.md)) and it is not built here, because **its write cost cannot be
  measured before it exists**, and a price that is not measured is not a number.
- **Out**: **an automatic compaction** — nothing here triggers a compaction by itself. The tax's shape decides that, and it is
  out.
- **Out**: **the engine's file-set check** — it does not compare nullability, and after R2 it does not need to: the database's
  own files agree by construction. **Whether the engine's check should compare it is the engine's behaviour and a later
  increment's decision.**
- **Out**: **the engine's budget and its estimate** — the budget stays at **256 MiB** and the estimate is not changed. The
  streaming compaction fits by reading less at a time, and a budget that is raised to fit a query is a budget that no longer
  says anything.
- **Out**: **a second output file** — the compaction writes one file, because that is what it promises.
- **Out**: **the publish** — spec 054 publishes the four crates, and this increment changes the store's surface **before**
  that publish, so that the surface is settled when it is frozen.

## The instrument

**`crates/columnar-db/examples/db_cost_probe.rs`** — the compaction's price, one-shot beside streaming, clean and taxed; the
ceiling, bisected; the bytes before and after.

**And the committed reports** — three campaigns, interleaved, in `bench-results/`, and the figures derived from them by
`scripts/medians.py` and checked by `scripts/check-published-figures.py`.

## The references

- [`crates/columnar-db/src/database.rs`](../../crates/columnar-db/src/database.rs) — the compaction, the one plan over the
  table's files, and the file it writes.
- [`crates/columnar-db/src/versions.rs`](../../crates/columnar-db/src/versions.rs) — the live filter, the tombstone tax, and
  the sentence that says the compaction is what pays it back.
- [`crates/columnar-store/src/rows.rs`](../../crates/columnar-store/src/rows.rs) — the schema derived from the data, and the
  grid.
- [`crates/columnar-engine/src/file_set.rs`](../../crates/columnar-engine/src/file_set.rs) — the file-set check, and the
  nullability it does not compare.
- [`crates/columnar-engine/src/memory.rs`](../../crates/columnar-engine/src/memory.rs) — the budget, and the estimate the
  budget checks.
- [PERF.md §3ak](../../PERF.md#3ak-the-databases-own-price) — the prices, the ceiling, and the tax's shape.
- [ADR-0066](../../docs/adr/0066-the-databases-own-price.md) — the storage model, the tax, and the index's shape.
- [constitution.md](../../constitution.md), principle **IV — Evidence Over Claims**: the ceiling is published before it is
  fixed, and the fix's price is published after it is fixed.
- [constitution.md](../../constitution.md), principle **VII — Explicit Scope, No Silent Deferral**: the tax's shape, the index,
  the automatic compaction, the engine's check and the budget are **named** as out of scope, rather than left as omissions.

## The amendments

### A1 — The compaction reads **file by file**, and the ceiling becomes the file's rows

**Why**: the ceiling is the engine's estimate of **one plan over the whole table**, and the estimate is refused before any
work is done. **The estimate is not wrong and the budget is not tight** — the plan is simply too big, and a plan that reads
a file is a plan a sixteenth of the size. **So the compaction issues one plan a file**, and the budget's verdict becomes a
verdict about a file's rows and not about the table's. **And the database's own peak falls with it**, because the rows in
flight are the file's rows.

**And the output is still one file.** The compaction's promise is that the retired versions stop existing and the table's
versions are renumbered from zero, and that promise is kept by the **manifest**, not by the read: the rows are gathered
from the files one at a time and written into one file. **A streaming read that wrote several files would be a compaction
that does not compact**, and the table would stay fragmented at the space amplification the grid costs today.

### A2 — The output schema is **decided by the table's schema**, and the store's row API grows the nullability

**Why**: the writer's schema is fixed before the first row, and the data's shape is known only after the last one — so
**a schema derived from the data cannot be used by a streaming write**. **And the derivation is itself the defect**: a column
is written required when the rows happen to hold no NULL and optional when they do, so **the same table's files can disagree
about a column's nullability**, and the engine's own check does not compare it.

**So the schema comes from the table's schema instead** — the database knows it, it is persistent, and it is the same for
every file the table will ever hold. **And the nullability becomes an explicit input to the store's row-writing API**,
because a caller that decides the schema must be able to say so, and the store does not infer it from the data any more.
**And the bytes change** — an optional column that holds no NULL still writes its definition levels — so **the change is
published**, old and new, with the reason named.

### A3 — A file that disagrees with the table's schema is **refused**, and the refusal names the file and the column

**Why**: after A2 the database's own files agree by construction, so a disagreement can only come from a file written before
the rule, or from outside the database. **And the compaction is the moment the disagreement would be inherited**: it is the
moment the table's files become one file, and one file has one schema. **So the compaction checks each file against the
table's schema — the nullability included — and refuses the table** rather than writing a file whose schema is not the
table's.

**And the refusal names the file and the column**, because a refusal that does not say which file is not a refusal an
application can act on, and a compaction that failed without a reason is a defect with no record.
