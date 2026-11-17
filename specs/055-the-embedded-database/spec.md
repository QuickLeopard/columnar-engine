# Spec 055 — The embedded database

**Status**: planned
**Started**: 2026-11-17
**Predecessor**: [spec 054 — the crates are published](../054-the-crates-are-published/spec.md)

## The problem

**The engine reads Parquet and the store writes it, and the database between them is not built.** The store says what it
is not, in its own words, at the top of its own crate: *"No SQL. No transactions with rollback. No MVCC, no snapshot
isolation. No concurrent writers. No row-level updates. No compaction. One writer."* And the engine's catalog is
**in memory** — a name to a path — so it does not survive a restart.

**So what this tree has is a query engine over files and an append-only store, and an embedded database is the layer
that turns them into one.** That layer is five things, and none of them is here:

- a **catalog** that survives a restart;
- a **row** that has an identity, so that it can change;
- a **read** that sees one version of a table and not a mixture of two;
- an **index**, or a measurement that says a scan is cheaper than one;
- and a **query language** an application can be written against.

**And every one of those has a price, and none of them has been measured.** The store measured the *write* — the whole
file at **1641.539 ms**, one row group at **272.694 ms**, the commit at **12.147 ms** ([§3s](../../PERF.md#3s-the-write-side))
— and the engine measured the *read* ([§3aj](../../PERF.md#3aj-the-contexts-two-sides-in-one-account)). **Nobody has
measured what a database costs on top of them**, and the house's first rule is that a thing is not built before it is
measured.

**And the query language is a decision, and the decision is not the engine's.** The engine already has an IR — a typed
`QueryPlan`, built by a typed `PlanBuilder` — and **SQL is a front end to such an IR, not the IR itself**. The difference
is not cosmetic: SQL promises subqueries, CTEs, window functions and arbitrary join graphs, and **this engine refuses all
four**. So the language is a fork with two honest answers, and it is the user's to take.

**And the database is not a mode of the engine.** The engine's surface is frozen and promised (ADR-0040) — **forty-one**
names and **four** of the core's vocabulary — and a database inside it would either widen that promise or hide behind it.
So the database is **a new crate above the four**.

## The requirements

### R1 — The database is a **new crate**, above the four

- [ ] `columnar-db` is a workspace member, and it depends on `columnar-engine` and `columnar-store` — **and on nothing
      else in this workspace**.
- [ ] **And the four crates are not changed to make it work**: a change to any of them is a change to a published
      surface, and it is made only if a defect is found, and then it is published.
- [ ] **And its public surface is small and it is named** — the crate's front page lists the names, the way the engine's
      does, so that what it promises can be read before it is used.

### R2 — The **catalog** is persistent

- [ ] A table's **name**, **schema** and **path** live on disk, and the catalog is **replaced atomically** — written to a
      temporary file and renamed over the old one, the same primitive the store's commit uses.
- [ ] **And a reopened database sees the same tables**, with the same schemas.
- [ ] **And the catalog's open cost is measured** — every open pays it, and it is the first thing an embedded database is
      judged on.

### R3 — The **write path** is typed, and on the grid

- [ ] A row is **typed values** — the engine's own types (`Int64`, `Float64`, `Utf8` — **the three the vocabulary
      actually has**, and a boolean is an `Int64`) and a nullability per column —
      **not** Arrow's, and not a third vocabulary.
- [ ] **And the rows land on the store's grid**: every batch but the last is **65,536** rows, because **the file's bytes
      depend on it**, and a write that cannot be on the grid is **refused** rather than written.
- [ ] **And the commit is the store's five steps**: the database adds **no durability mechanism of its own** — no
      write-ahead log, no rollback, no second recovery path.

### R4 — A read is a **snapshot**

- [ ] A **snapshot** pins the manifest it read, and the file list that manifest names is the table **for the life of the
      snapshot**.
- [ ] **And a reader sees the old file set or the new one, never a mixture** — which the store's rename-commit already
      makes true, and which the snapshot's pinning is what makes it **reachable**.
- [ ] **And there is one writer**: the writer is serialized and the readers are not.

### R5 — A row has an **id**, and a row can **change**

- [ ] Every table carries a hidden **`row_id`** column — an **`Int64`** the database assigns and the application never sees.
      **And the type is the engine's, not a `u64`:** the engine's vocabulary is `Int64`, `Float64` and `Utf8`, and an
      unsigned type would be a second vocabulary for one column — the very thing the vocabulary's rule refuses. **This
      corrects the `u64` this requirement first named**, and the correction is recorded in the ADR.
- [ ] **An update is an append** of the new version and a **tombstone** of the old; **a delete is a tombstone**. Neither
      rewrites a file that exists.
- [ ] **And compaction** rewrites the live rows into a new file and drops the dead ones — **and its cost per row is
      published**, because compaction is the price of the append-only model, and a price that is not published is a price
      nobody has agreed to pay.

### R6 — The query language is the **typed plan**

- [ ] The database's read path **is** the engine's `QueryPlan`, built with the engine's own `PlanBuilder` and
      `ColumnExpr` — **and the engine's surface is not widened** to serve it.
- [ ] **And the database's values are the engine's `ScalarValue`** — one vocabulary, not two, and not a third one that
      has to be converted at the boundary.
- [ ] **And SQL is out of scope, with the reason named** — the plan is the IR, the typed builder covers the same ground
      without a parser, and SQL would promise features this engine refuses. **And the decision is recorded in the ADR**,
      so that the next increment does not ask again.

### R7 — The gap is **priced**, and the price decides the shape

- [ ] The probe measures, on this host, in this tree: the **catalog's open cost**; the **write's throughput** on the grid;
      the **snapshot's read cost** over one file and over many; the **scan cost of a point lookup**; the **compaction's
      cost per row**; and the **space amplification** of append-only growth before a compaction.
- [ ] **And the figures are published** — and **the index's shape is decided by the scan's figure**: a point lookup that a
      scan already answers more cheaply is **not** given an index, and the database says so.

### R8 — The record

- [ ] A **new section** in `PERF.md`, the decisions in an **ADR**, **STATUS.md**'s bullet moves, and the raw reports
      **committed**.
- [ ] **And the guard checks the published figures inside the section that publishes them.**

### R9 — The closure

- [ ] The spec's status is `done` with the date it finished, and CI is green.

## The scope

- **In**: the crate, the persistent catalog, the typed write path, the snapshot, the row id, the update, the delete, the
  compaction, the typed read path, the gap's prices, and the record.
- **Out**: **SQL** — named out, with the reason (R6), and the decision recorded so that it is not re-litigated. **If the
  audience turns out to speak SQL, the front end is a later increment over the same plan.**
- **Out**: **the publish of `columnar-db`** — spec 054 publishes the four; this crate publishes when its own surface is
  frozen, and that is its own increment.
- **Out**: **a transaction over more than one append** — the store's commit is one file and one rename, and this
  increment's transaction is one append. A manifest that names several files is a later increment.
- **Out**: **blobs** — the vocabulary is the engine's, and a blob is a file and a pointer, which is its own increment.
- **Out**: **encryption, a general optimizer, and a second writer** — each is named rather than left as an omission, and
  each would change the store's shape.
- **Out**: **the engine's own surface** — it is frozen, and this increment does not widen it.

## The instrument

**`crates/columnar-db/examples/db_cost_probe.rs`** — the gap, priced: the catalog's open, the write on the grid, the
snapshot over one file and over many, the scan of a point lookup, the compaction, and the space amplification.

**And the committed reports** — three campaigns, interleaved, in `bench-results/`, and the figures derived from them by
`scripts/medians.py` and checked by `scripts/check-published-figures.py`.

## The references

- [crates/columnar-store/src/lib.rs](../../crates/columnar-store/src/lib.rs) — the store's own account of what it is not,
  and of the commit that is a rename.
- [crates/columnar-engine/src/facade.rs](../../crates/columnar-engine/src/facade.rs) — the `Catalog` that is in memory, the
  `PlanBuilder` that is the typed language, and the `ColumnExpr` that is its vocabulary.
- [ADR-0040](../../docs/adr/0040-the-engine-as-a-library.md) — the frozen surface, and why the database is a new crate.
- [ADR-0047](../../docs/adr/0047-the-stores-publish.md) — the store, and the publish.
- [PERF.md §3s](../../PERF.md#3s-the-write-side) — the write side's price, the commit's price, and the grid's rule.
- [PERF.md §3aj](../../PERF.md#3aj-the-contexts-two-sides-in-one-account) — the last increment, and the last open question,
  closed.
- [constitution.md](../../constitution.md), principle **IV — Evidence Over Claims**: a number is a claim about a
  measurement, and a claim without its evidence is not published.
- [constitution.md](../../constitution.md), principle **VII — Explicit Scope, No Silent Deferral**: SQL, the publish of the
  new crate, the multi-file transaction, the blobs, the encryption, the optimizer and the second writer are **named** as
  out of scope, rather than left as omissions.

## The amendments

### A1 — The database is a **new crate**, and not a mode of the engine

**Why**: the engine's surface is a promise (ADR-0040) — **forty-one** names and **four** of the core's vocabulary, frozen
at the publish — and a surface published and then changed is a break in every embedder's build. **A database inside the
engine would either widen that promise or hide behind it**, and both are worse than a crate that says what it is. **So
`columnar-db` sits above the four**, depends on the engine and the store, and touches neither.

### A2 — **Append-only plus compaction**, and **no write-ahead log**

**Why**: the store's commit is a rename, and **the rename is the answer** — the store says so in its own words, and its
reasoning is sound: a reader that sees the new manifest sees the new data, and a reader that sees the old one sees the
old data. **A write-ahead log would add a second durability mechanism and a second recovery path** to a design that has
one of each and works, and that is a cost paid for nothing. **So the database keeps the store's primitive**: an append, a
tombstone, and a compaction that rewrites the live rows into a new file and drops the dead ones. **And the compaction's
price is published**, because an append-only model without a published compaction price is a model whose cost is hidden.

### A3 — The **query language is the typed plan**, and **SQL is out**

**Why**: the engine's plan is already the IR, and `PlanBuilder` is already a fluent builder over it — `filter`, `filters`,
`projection`, `aggregate`, `join`, `join_filtered`, `sort`, `build`. **SQL is a front end to such an IR, and it is not the
IR**: it promises subqueries, CTEs, window functions and arbitrary join graphs, and **this engine refuses all four**. A SQL
front end would therefore have to carry a parser, a type system to reconcile with the engine's, and — above all — **the
engine's limits as a rule set, so that it could say "not supported" honestly**. **That is a large surface bought for a
spelling.**

**So the database's query language is the typed plan**, and the decision is recorded here so that the next increment does
not ask again. **And the decision is not a refusal of SQL** — it is a refusal of SQL *now*: a front end is a separate
increment over the same plan, and nothing this increment builds has to be redone when the audience demands one.

### A4 — The hidden columns are **two**, and the tombstone is a **row**

**Why**: an update cannot rewrite the version it replaces — the files are append-only, and a file that is rewritten is a file
a pinned reader can no longer read. **So the replacement is a tombstone, and the tombstone has to be a row**: a row in the
data file, beside the new version, so that the two land together — one file, one manifest, one rename. **And a row that says
"this version is gone" needs a flag to say it**, which is the second hidden column: the `deleted` flag. **And the retired
version's exclusion is necessarily external** — a file cannot un-live a row by changing its own bytes — so the read filters
on the flag, which drops the tombstones, and on the retired ids, which drops the versions the tombstones name. **That second
filter is the tombstone tax**: one predicate per retired version, and the compaction is what pays it back.

**And the ids are read from what is on disk, never written down.** The id a new version gets is the number of versions the
table has ever had, and that number is the manifest's rows less the retired ones. **A counter kept in the catalog would be a
second thing to keep consistent with the data**, and a crash between the write and the counter's write would make two rows
share an id — so the counter is a function of the durable state and there is no window in which it can be wrong.

### A5 — The database owns the **file naming**, and a file is removed only when **no reader holds it**

**Why**: the store names a file after the number of files the manifest holds, and a compaction replaces the manifest with one
file that names nothing else — **so the store's numbering would start again and the next write would land on a name an old
file still has**. The database names its files by the highest index in the table's directory plus one, and the bytes are
still the store's writer's bytes.

**And a compaction removes the files the old manifest named**, because the new file holds every live row and the old ones hold
nothing an application can see. **But a reader that pinned them still has to read them**, so a snapshot registers the files it
pinned and gives them back when it is dropped, and a file that is not removed is not lost: it stays outside the manifest, and
no reader will ever open it.
