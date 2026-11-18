# ADR-0066: The database's price, and the shape the price decides

**Status**: accepted
**Date**: 2026-11-18
**Related**: [ADR-0040](0040-the-engine-as-a-library.md), [ADR-0047](0047-the-stores-publish.md), [spec 055](../../specs/055-the-embedded-database/spec.md)

## Context

**The store measured the write and the engine measured the read, and nobody had measured what a database costs on top of
them.** The store's own crate says what it is not — *"No SQL. No transactions with rollback. No MVCC, no snapshot
isolation. No concurrent writers. No row-level updates. No compaction. One writer."* — and the layer that turns a query
engine over files and an append-only store into a database is five things: a catalog that survives a restart, a row with
an identity, a read that sees one version, an index, and a query language.

**And two of the three decisions this increment asks for were already made, and the third was left to a price.** The
storage model was decided by the spec's amendment A2 — append-only plus compaction, and no write-ahead log — and the
query language by A3 — the engine's typed plan, and not SQL. **And the index's shape was left to the measurement**:
*a point lookup that a scan already answers more cheaply is not given an index, and the database says so.*

**And the tombstone tax had a price nobody had measured.** A read filters on `deleted = 0` and on `row_id != <retired
id>` for every retired version, and the engine's pushdown runs each comparison on a column as one pass and ANDs the
masks — so the tax is one pass over the id column for every retired version. **The prose said the tax was the retired
versions and not the table's size, and that sentence was published**: in the crate's own documentation and in the commit
that named it.

## The measurement

**The instrument is `crates/columnar-db/examples/db_cost_probe.rs`**, and the table is a million rows of the published
lineitem file — three columns, read once through the engine and written in the store's grid, so **sixteen files of
65,536 rows**.

**Three campaigns**, on one host, at a load average of **10.63**, **11.73** and **15.47**. **And every
difference below is a difference of two measurements taken back to back**, so the machine's movement over the hundreds
of milliseconds between them cannot land on one side of the difference and not on the other. The full tables are in
[PERF.md §3ak](../../PERF.md#3ak-the-databases-own-price).

| the price | |
|---|---|
| the catalog's open | **0.242 ms** |
| the write on the grid | **389.590 ms** — 2,566,802 rows/s |
| the snapshot's read, one file | **2.732 ms** |
| the snapshot's read, many files | **44.446 ms** |
| the engine's own read, the same files | **37.145 ms** |
| **the database's overhead on the read** | **6.777 ms** |
| the point lookup's scan, through the database | **52.168 ms** |
| the point lookup's scan, the engine's own | **43.319 ms** |
| **the point lookup over one file: the floor** | **2.657 ms** |
| the open of the table's files | **0.461 ms** |
| the compaction, with no retired versions | **293.293 ms** — 293 ns a row |
| the compaction, with its retired versions | **12,431.089 ms** — 13,088 ns a row |
| **the tax the read pays** | **13,282.677 ms** |
| **the tax, a retired version over a row** | **0.115 ns** |
| the space amplification | **1.339** |
| the compaction's ceiling | **5,375,000 rows compacted, 6,000,000 refused** |

**And the tax is measured at two sizes, and the two prices are the same.** The same share of the key range changes in the
small table and in the large one, so the two taxes are two numbers of predicates over two different numbers of rows:
**0.115 ns** a retired version a row in the large one, and **0.120 ns** in the small one. **So the tax
is the retired versions and the table's size together**, and the two sizes say so independently.

**And the read that pays it is the database's own.** The table's live rows read in **44.446 ms** before anything
changed, and in **13,515.470 ms** after a fifth of its key range did — **a 304.1× cliff**, for a
change that touched **100,136** of the table's **1,000,000** rows.

**And the compaction's ceiling is measured, not asserted.** The compaction reads the table's live rows in one piece, so
the engine's memory budget is the compaction's ceiling: **5,375,000 rows compacted**, and **6,000,000 rows
refused** with *"the engine refused: the estimated working set exceeds the query memory budget"*.

## Decision

**The storage model is append-only plus compaction, and the tombstone tax is the retired versions times the table's
size.**

The append-only model stands, for the reason the spec gave and the store's own commit supports: the rename is the
answer, and a write-ahead log would buy a second durability mechanism and a second recovery path for nothing. **But the
tax is not what the prose said it was.** Each retired version costs one predicate, and each predicate is one pass over
the id column, so **the tax is the retired versions and the rows together** — **0.115 ns** a retired version a
row, measured at two sizes — **and every read pays it, and not only the compaction.**

**So the compaction is not only the price of the append-only model: it is the price of the tax, and nothing else pays
it back.** A table whose rows change is a table whose every read pays the tax, and the tax is paid until a compaction
rewrites the live rows and the retired versions stop existing. **And the compaction's own cost is 293 ns
a row**, which is the whole of what the compaction costs when there is no tax to pay — and the tax is
**13,088 ns** a row on top of it.

**The query language is the engine's typed plan, and SQL is out of scope, with the reason named.** The engine's plan is
already the IR, and `PlanBuilder` is already a fluent builder over it — `filter`, `filters`, `projection`, `aggregate`,
`join`, `join_filtered`, `sort`, `build`. **SQL is a front end to such an IR and not the IR**: it promises subqueries,
CTEs, window functions and arbitrary join graphs, and **this engine refuses all four**. A SQL front end would carry a
parser, a type system to reconcile with the engine's, and — above all — **the engine's limits as a rule set, so that
it could say "not supported" honestly**. **So the decision is recorded here so that the next increment does not ask
again**, and the refusal is not of SQL forever: a front end is a later increment over the same plan, and nothing this
increment builds has to be redone when the audience demands one.

**And the index is given, and its shape is a map from the key to the files that hold it.**

**The scan does not answer the lookup more cheaply than an index would.** The point lookup over the whole table costs
**43.319 ms** in the engine's own read, and the same lookup over **one file** — the floor an index would reach
— costs **2.657 ms**. **So the index's shape is decided by that figure: the lookup is 17.20× more
expensive as a scan, and the most an index could save is 40.828 ms.**

**And the gap is the data and not the files.** The open of the table's sixteen files, with nothing read, costs
**0.461 ms** of the **43.319 ms** — **1.1%**. **So an index does not win by avoiding the
files' open, and it wins by not reading the data of the fifteen files it does not need.**

**And the granularity the figure decides is the file, because that is where the floor was measured.** The database
writes on the grid, and the grid is **65,536** rows, so a file is one grid. **So the index is a map from the key to the
file that holds it** — and **the map is many-to-one**, because an update writes a new file whose keys are the changed
ones, so a key's versions live in more than one file until a compaction rewrites them into one.

**And the index cannot name a row, and the floor says so.** The engine reads whole files and has no row-group skipping,
so the **2.657 ms** floor is not a number an index could go below by being cleverer: **it is the engine's own
read granularity, measured**. **So the index's shape is bounded by the engine's, and the bound is published.**

## Consequences

- **The sentence the price corrects is corrected where it was published.** The crate's documentation said the tax was
  *"the retired versions and not the table's size"*; **it is the retired versions and the table's size together**, and
  **the correction is recorded here**. The prose in `crates/columnar-db/src/versions.rs` is corrected, and the price that
  corrects it is in [PERF.md §3ak](../../PERF.md#3ak-the-databases-own-price).
- **The tax's shape is the next increment's question, and the question is named.** **0.115 ns** a retired version a
  row is a price no application can pay at a real change rate: a table of a million rows with a fifth of its rows changed
  spends **13,282.677 ms** on **every** read. **So the retired versions cannot be filtered with one predicate per
  retired version** — and the ways out are named, and none of them is built here: **the database filters the retired
  versions itself**, in one pass over the ids it has already read, and not through the engine's predicates; **or the
  engine grows one predicate that means "not one of these"**, which widens a frozen surface and is its own increment;
  **or the compaction runs by itself**, so the tax is bounded by the change rate and not by the history.
- **The compaction's ceiling is a defect, and it is published.** The compaction reads the live rows in one piece, so it
  is refused above **6,000,000 rows** by the engine's **256 MiB** budget. **So the compaction must be
  streaming** — file by file, one output file, and the Arrow schemas of the files must agree — and that is a later
  increment, and it is named in [STATUS.md](../../STATUS.md)'s open questions.
- **The index is not built here, and its write side is not measured.** The read side is what the figure decided, and
  **the index's own cost — its build, its storage and its lookup — is unmeasured, because there is no index to
  measure.** So the shape is decided and the price is not, and the next increment that builds an index measures it.
- **The crates.io publish is not waiting on this.** The crates stay unpublished while the remaining work lands.
