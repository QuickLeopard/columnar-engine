# Spec 043 — The set's rows

**Status**: planned
**Started**: 2026-11-06
**Predecessor**: [spec 042 — the set on both sides](../042-the-set-on-both-sides/spec.md)

## The problem

**A store's table is a set of files, the engine reads that set as one table — and cannot take its rows
out.**

### The composition, so far

- The store writes a table as a **set of files**: a commit appends one and never modifies an old one
  ([ADR-0044](../../docs/adr/0044-the-stores-half.md)).
- The engine reads a set of files as **one table** — `FileSetSource` ([spec
  040](../040-the-source-that-reads-a-set-of-files/spec.md)).
- The join's **build side** takes a set ([spec
  042](../042-the-set-on-both-sides/spec.md)).
- The engine's **rows** reach a store's table ([spec
  042](../042-the-set-on-both-sides/spec.md)).

**The last link is missing.** The engine hands rows out in two places: `execute_rows` over a `Dataset` — the
eager path (`crates/columnar-engine/src/facade.rs:258`) — and `execute_rows_on` over a `ParquetSource` — the
lazy path, **one file** (`crates/columnar-engine/src/facade.rs:402`). **Neither takes a set.** The set's own
entry point, `FileSetSource::execute` (`crates/columnar-engine/src/file_set.rs:363`), folds the rows into an
aggregate, and there is no other door.

**So the round trip is not closed.** A store's table cannot be read by the engine and written back by the
store. An embedder can copy the files one by one, and then the engine never saw the rows and the file that
comes out was never the engine's.

### The gap is one method wide

**Everything this increment needs is already built.** The source reads a set; the pipeline hands a plan's rows
out; the store takes the engine's rows (`crates/columnar-store/src/store.rs:180`); the store hands the list of
paths back (`crates/columnar-store/src/store.rs:233`). **What is missing is the door between the set and the
rows** — and the door is what has to be measured, because a door that opens onto the wrong rows is a door
that is worse than no door.

### The claim this increment has to earn

**The round trip is exact.** A table's rows, read by the engine and written by the store, are **the file the
rows came from**, byte for byte — not an equivalent file, not a file with the same values, **the same
bytes**. And the set makes no difference: the rows of **six files** and the rows of **one file** are the
same rows, and they write the same file.

**That is the claim the whole project rests on.** The store is append-only and the engine is a reader; a
table that cannot be read back byte for byte is a table whose bytes depend on how it was written, and every
guarantee above it — the manifest's rows, the grid, the commit — is a guarantee about bytes. **This increment
measures the guarantee end to end, on the published file, through the engine's own rows.**

## The requirements

### R1 — The set's rows are handed out

`FileSetSource::execute_rows(&self, plan, options) -> Result<Dataset, EngineError>` — the set's own
counterpart of `execute_rows_on`, and **the same path underneath**: the plan is resolved against the set's
schema, the scan's decision is the reader's decision, and the stages are the engine's.

**A plan with an aggregation is refused, for the reason the other two refusals give**: the aggregate folds
the rows into groups and there is nothing left to write. The refusal is the **same error and the same
wording** as the eager and one-file paths — one rule, one message.

### R2 — The set's rows are the file's rows

**Measured, value for value, in the same order.** The set's rows and the one file's rows are the same rows,
and the way the measurement says it is the way the store's own bytes say it: **the two write the same file,
byte for byte**. A file is a stronger witness than a checksum, because a checksum is a summary and a file is
the thing itself.

**The set's order is the table's order** — file by file, and inside a file row group by row group — and the
one file's order is the file's own. That the two agree is not a coincidence; it is the claim.

### R3 — The round trip is measured, and published whatever it comes out to be

**Three files, three questions**, the same shape spec 042 used for the store's own claim:

1. **The store's own API** — `append_rows`, the store's schema rule — writes the rows. Compared with the
   published file: the bytes, and **the first differing byte**.
2. **The same rows under the published file's own schema** — the Arrow path with the file's own schema —
   writes them. Compared with the published file: the bytes, and the first differing byte.
3. **The set's rows** — the same two writes, from the six files. Compared with the one file's writes: the
   bytes, and the first differing byte.

**A claim the measurement does not support is not published as a claim.** If the round trip is not exact,
that is what §3aa says, with the first differing byte named.

### R4 — The set's cost is measured

**What the set costs in the write path.** The read of six files against one file, and the write of the rows
that come out. **The pairs are measured in the same campaign**, so the host's own movement is in both sides
of every pair — the host is shared, and a busy host is in one arm and not the other otherwise.

**And the estimate is not forgotten.** The budget check is made of the estimate, so the estimate a set is
charged is published beside what the read actually used.

### R5 — The surface does not move

**No new type, and no break.** `execute_rows` takes a `&QueryPlan` and an `&ExecOptions` and gives back a
`Dataset` — all three already on the surface — so **the surface list does not change** and `cargo
semver-checks` reports no change against the baseline spec 042 moved.

**That matters now.** The one break the project ever took was taken while the crates were unpublished
([ADR-0053](../../docs/adr/0053-the-build-side-takes-a-set.md)); this increment takes **none**, so it can be
made after the publish as well as before it.

### R6 — The scope is named, so it is a decision and not an oversight

**No SQL. No transactions with rollback. No MVCC. No concurrent writers. No row-level updates. No
compaction. One writer.** And, specific to this increment:

- **The set's rows are handed out, not streamed.** The rows come back as a `Dataset`, in memory, exactly as
  they do for one file. A set that does not fit the budget is refused, as a file that does not fit is.
- **The store is not taught about the engine.** The glue stays the list of paths and the engine's rows: the
  store's library still does not depend on the engine.
- **A table is still not rewritten.** The round trip writes a **new** table, or a new file in a table. The
  store does not overwrite what it holds, and this increment does not change that.
- **The round trip is measured on the published file**, which is one file and one set of six. A table with
  many small files is a different shape, and it is the shape a real store's table has after many appends.

## The plan's shape

1. **`FileSetSource::execute_rows`** — the set's own door to its rows. It calls the engine's one path:
   `facade::execute_source(plan, self, options, Output::Rows)`, the same call `execute` makes with
   `Output::Aggregate`.
2. **No new public type.** `Dataset`, `QueryPlan` and `ExecOptions` are on the surface already; the method
   is a method on a surface type.
3. **The store changes not at all.** `append_rows` takes the engine's rows, `Table::paths` gives the list of
   paths, and the round trip is the embedder's three lines:
   ```text
   let paths = table.paths();
   let set = FileSetSource::open(&paths)?;
   let rows = set.execute_rows(&plan, &options)?;
   store.append_rows("name", rows.chunks(), properties)?;
   ```
4. **The tests** are the store's: the store writes a table, the engine reads it as a set, the engine's rows
   go back into the store, and the file comes out the same.

## The scope

| | |
|---|---|
| the engine's read path | **Unchanged.** Its published figures do not move, and one path is still one path. |
| the one-file paths | **Unchanged.** `execute_rows` and `execute_rows_on` keep their signatures and their behaviour. |
| the set's rows | **Handed out.** `FileSetSource::execute_rows` is the set's own door, and a plan with an aggregation is refused. |
| the store | **Unchanged.** Its writer, its grid, its commit, its schema rule and its dependency rule are all as spec 042 left them. |
| the store's dependencies | **Unchanged.** The store's library still does not depend on the engine. |
| the engine's surface | **Unchanged.** No new type, no break. |
| the write path | **Unchanged.** The write path writes a **new** table or a new file; it never rewrites one. |

## The measurement

One probe, three campaigns, medians with the spreads, the raw reports committed, and the tables derived from
the reports by `scripts/medians.py` and checked by the guard **inside the section that publishes them**.

**The probe** takes the published file's own **six row groups** written as **six files** — the experiment
spec 039 used and spec 042 reused — and asks four questions:

1. **The cost.** The read of the set against the read of the one file, and the write of each one's rows.
2. **The identity.** The set's rows and the one file's rows, written, compared byte for byte.
3. **The round trip.** The written file against the published file — the bytes, and the first differing byte
   — under the store's own rule and under the file's own schema.
4. **The estimate.** What the budget is charged for the set, against what the read used.

**The probe is written after the code it measures, and the reason is recorded.** The set's rows cannot be
measured before the door exists, and the house's rule is that the **publication** waits for the measurement
— which it does.

## The references

- [constitution.md](../../constitution.md) IV — evidence over claims: the round trip is measured and
  published as it comes out, and the first differing byte is named whichever way it comes out.
- [constitution.md](../../constitution.md) VII — explicit scope, no silent deferral: the scope table names
  what is not in this increment, so the deferral is a decision.
- [constitution.md](../../constitution.md) — *Development Workflow & Quality Gates*: Specify → Plan → Tasks →
  Implement; this spec is the specification, and the code follows it.
- [ADR-0044](../../docs/adr/0044-the-stores-half.md) — the store's half: the grid, the commit, the identity
  claim, and the dependency rule this increment does not touch.
- [ADR-0046](../../docs/adr/0046-the-source-over-a-set-of-files.md) — the source over a set of files: the
  set's own door.
- [ADR-0053](../../docs/adr/0053-the-build-side-takes-a-set.md) — the build side takes a set, and the one
  break the project took.
- [ADR-0054](../../docs/adr/0054-the-engines-rows-reach-a-stores-table.md) — the engine's rows reach a
  store's table: the conversion, the grid's owner, and the schema rule the round trip is measured against.
- [spec 039](../039-the-engine-reads-a-stores-table/spec.md) — the file boundary's cost: the experiment this
  increment reuses.
- [spec 040](../040-the-source-that-reads-a-set-of-files/spec.md) — the source that reads a set of files.
- [spec 042](../042-the-set-on-both-sides/spec.md) — the set on both sides: the build side, the write path,
  and the amendment that named this increment.
