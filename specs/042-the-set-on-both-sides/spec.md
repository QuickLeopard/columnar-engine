# Spec 042 — The set on both sides

**Status**: done
**Started**: 2026-11-06
**Finished**: 2026-11-06
**Predecessor**: [spec 041 — the reader's four gigabytes](../041-the-readers-four-gigabytes/spec.md)

**CI green on `c2be4802`** -- run 42190345103, fifteen of fifteen jobs.

## The problem

**The store writes a table as a set of files, and the engine reads a set of files as one table. One
side of that is not done: the join's build side takes one path, and the engine cannot write a table
back into a store.**

### The build side names one path

The join's build side is the right-hand table, and it is the one the hash table is built from before the
probe looks anything up. It is named in the plan by **one path**:

```rust
pub struct JoinSpec {
    /// The Parquet file of the **build side** -- the table on the right.
    pub build_path: PathBuf,
```

and it is opened, concretely, in four places: `BuildSide` holds an `Option<ParquetSource>`
(`crates/columnar-engine/src/pipeline.rs:114`), it is opened as `ParquetSource::open(&join.spec.build_path)`
(`pipeline.rs:135`), `build_table` takes a `&ParquetSource` (`crates/columnar-engine/src/operators/join.rs:424`),
and the estimate charges a `&ParquetSource` (`crates/columnar-engine/src/memory.rs:384`).

**The probe side has none of that.** The source is handed *in* — `execute_with<S: EngineSource>(plan,
source: &S, options)` (`crates/columnar-engine/src/facade.rs:337`) — and `FileSetSource` implements
`EngineSource` (`crates/columnar-engine/src/file_set.rs:623`). **That genericity is the whole of spec
040**: it is why a store's table can be the left-hand table of a join and not the right-hand one.

**So a store's table can sit on the probe side and not on the build side.** The limit is named in
`STATUS.md` rather than left to be discovered, and this increment is where it is taken.

**And the case for it is narrow, and the measurement has to say so.** A hash join builds the **small**
side on purpose. A store's table is many files, so it is usually the **big** side — the side the probe
handles lazily, and handles today. The case this unlocks is the store holding the **small** table.

### The engine cannot write a table back

The store's writer takes **Arrow** `RecordBatch`es (`crates/columnar-store/src/store.rs:118`), and the
engine holds **its own** format: `ColumnChunk { name, encoding, validity, len }`
(`crates/columnar-core/src/chunk.rs:137`), with `Encoding` being `Int64`, `Float64`, `Utf8` and their
dictionary forms. **There is no chunk-to-Arrow conversion in the tree**, and there cannot be one in
`columnar-core`: the core has **no dependencies at all** — *"a vocabulary that depends on a framework is
not a vocabulary"* — and Arrow is a framework.

**And the rows the engine hands back are not on the store's grid.** The grid is **65,536 rows**, and the
store **refuses** a batch off it (`Error::Grid`), because the file's bytes are a function of the batch
boundaries. The engine's reader batch is the **derived** batch — the largest divisor of the row group's
row count under 131,072 — which is **125,000** rows for the published file's million-row row groups
(`crates/columnar-engine/src/source.rs:1229`). **125,000 is not 65,536**, so the engine's rows cannot be
handed to the store as they are.

**And the store's strongest claim has never been made through the engine.** The store writes the
published file **byte for byte** — because the probe reads the file at exactly the grid's batch size
(`crates/columnar-store/examples/store_probe.rs:124`, `with_batch_size(BATCH_ROWS)`) and writes the
batches unchanged. **Whether the engine's own rows produce the same bytes is not known**, and the
engine's rows are not Arrow's rows: the engine's dictionary is the page's, and the writer's dictionary
fallback is decided at the mini-batch boundaries, which are the grid's.

**And two facts about the engine's rows decide what the write path can promise.**

1. **The engine materialises the whole read set.** The lazy path decodes every row of the columns the
   plan reads into one dataset, and the estimate charges the file's rows over the read set's columns
   (`crates/columnar-engine/src/memory.rs:384`). **So a write path does not have to be streaming to be
   affordable** — the memory is the engine's own budget, already charged.
2. **The projection materialises the survivors' values, and a dictionary is lost on the way.** The
   gather reads the dictionary and writes the values out
   (`crates/columnar-engine/src/operators/project.rs:410`). **So a plan's rows are plain columns**,
   while the file's rows are dictionary-encoded. A plan's result written back is therefore **a
   different file from the published one** — the same values, plainly encoded — and that is a fact to
   publish, not a gap to hide.

**So this is two increments' worth of work, and they are one shape**: a set on the probe side and a set
on the write side, and the build side in between.

## The requirements

### R1 — The build side takes a set, and the one-file path does not move

`ParquetSource`'s read path is the most measured code in this repository. **It stays exactly as it is**,
its published figures do not move, and **one path is still one path**: a build side named by a single
file is opened as a `ParquetSource`, exactly as it is today, and the set is the new case beside it.

### R2 — The set's build side is measured before it is published

The house's rule: measure first, publish what the measurement says. The probe takes the published file's
own **six row groups** written as six files — the experiment spec 039 already used — and asks four
questions:

1. **The cost.** Building the table from six files against one file, in the same campaign.
2. **The identity.** The same rows, the same counts per key, the same output rows in the same order.
3. **The table's shape.** The direct-addressed table's width is the key's **range**, and a set's range
   is the **union** of the files' ranges (`crates/columnar-engine/src/file_set.rs:225`). A wider range
   can pass `DIRECT_TABLE_LIMIT` (32 MiB) and swap the direct table for the hash table — 16.4 ms against
   66.2 ms on the measured shape. **Whether it does is a fact, not a guess.**
4. **The estimate.** What the budget is charged for a set, against what the build actually uses. **The
   estimate is what the budget check is made of**, so an estimate that under-charges is a budget check
   that lies.

### R3 — The estimate charges the set's rows and the union's range

The build side's charge is the file's rows over the read set's columns, plus the survivors' keys, plus
the table (`crates/columnar-engine/src/memory.rs:384`). **For a set the rows are the sum, the range is
the union, and the workers are the set's** — the same three facts a source publishes. The estimate's
shape does not change; its inputs do.

### R4 — The engine's rows reach a store's table

The engine's rows reach a store's table, and the write is a **store's** write: the store's writer, the
store's policy, the store's grid, the store's commit. **The embedder's glue stays two lines** — hand the
engine's rows to the store — because the glue is the thing the architecture promised.

**Two shapes, and they are not the same claim.** The engine's rows are either the file's own rows, as
they lie (the eager path, with the dictionary) or a plan's rows (the projection's, plain). The first is
**the store's identity claim, extended through the engine**; the second is **a plan's result written
back**, which is a different file and says so.

### R5 — The grid has one owner: the store

**The engine does not learn the grid.** The grid is the store's rule (ADR-0044's decision 5), and the
store keeps it: the rows are re-batched on the grid **inside the store**, and a batch that cannot be on
the grid is still an error. **The engine's own reader batch — 125,000 rows for the published file — is
not the store's business.**

### R6 — The store's identity claim is measured through the engine's own rows

The store's claim is *"fed the same rows, it writes the same file, byte for byte"*. This increment asks
the harder question: **fed the engine's own rows, does it?** The answer is published **whatever it is**
— equal, or different, with the first differing byte named. **A claim the measurement does not support
is not published as a claim.**

**And the plan's rows are a second measurement, not the same one.** They are the same *values* and
different *bytes*, because the projection materialises the dictionary away. Both figures are published:
what the store's write of the engine's own rows costs in bytes against the published file, and what the
same write costs when the rows came through a plan.

### R7 — The store's dependency rule changes, and every place that published it says so

The conversion from the engine's chunks to Arrow **cannot** live in `columnar-core` (no dependencies) and
**must not** live in the engine (the engine's surface would carry Arrow). **So it lives in the store**,
and the store gains a dependency on `columnar-core` — the vocabulary, not the engine. **The engine's read
path still cannot move because of anything in the store** ✓, and the publish order is unchanged (core,
kernels, store, engine).

**ADR-0044's decision 1, ADR-0047's correction, `STATUS.md`, the store's `Cargo.toml`, its README and
`docs/embedding.md` all say the store depends on nothing in this workspace. Every one of them is
corrected where it says it**, and a new ADR records the decision.

### R8 — The scope is named, so it is a decision and not an oversight

**No SQL. No transactions with rollback. No MVCC. No concurrent writers. No row-level updates. No
compaction. One writer.** And: the write path **writes a table**; it does not update one, and the
engine still cannot write into a table the store already holds without appending a file.

## The plan's shape

**The build side:**

1. `EngineSource` — crate-private — gains `column_index` and `column_range`, so that the build side's
   lookups and the estimate's range work over any source. **No public change.**
2. `BuildSide` holds a source rather than a file: one path opens a `ParquetSource`, several open a
   `FileSetSource`.
3. `build_table` and `build_key_type` take a source, not a file.
4. The estimate's build-side charge takes a source. Its **public signature** — it is a harness function
   in a `#[doc(hidden)]` module, not on the surface — follows.
5. `JoinSpec.build_path: PathBuf` becomes `build_paths: Vec<PathBuf>`. **This is the one breaking change
   on the published surface**, and it is free now and a major version after the publish.

**The write side:**

6. The engine gains a way to hand **a plan's rows** out: the projection's columns, gathered, as the
   surface's `Dataset`. A plan with an aggregate is refused — the aggregate folds the rows, and there is
   nothing left to write.
7. The engine gains the **batch column to chunk** conversion — the dense, gathered column to the
   file-level chunk the surface already names.
8. The store gains the **chunk-to-Arrow conversion** — the engine's format to Arrow's, value for value,
   dictionary for dictionary, and the validity mask with them.
9. The store gains an append that takes the engine's rows, **re-batches them on the grid**, writes them
   with the store's writer and policy, and commits. The existing Arrow-taking append stays: it is the
   same rule with one implementation, not a second rule.
10. The store's dependency on `columnar-core` is the vocabulary's, and nothing else moves.

## The scope

| | |
|---|---|
| the engine's read path | **Unchanged.** Its published figures do not move, and one path is still one path. |
| the probe side | **Unchanged.** `FileSetSource` is what it was — and **the set's rows are not handed out yet**: `execute_rows` and `execute_rows_on` take a dataset and a file, and the set's own `execute` still folds the rows into an aggregate. **That is the next increment, and it is named here so it is a decision and not an oversight.** |
| the store's writer, the grid and the commit | **Unchanged.** The grid's owner is still the store, and the commit is still the five measured steps. |
| the build side's shape | **A set may be named.** One path behaves as it does today. |
| the write path | **Built.** The engine's rows reach a store's table, on the grid, committed. |
| the engine's surface | **One breaking change**: `JoinSpec.build_path` → `build_paths`. Arrow does **not** enter the surface — the conversion lives in the store. |
| the store's surface | **Grows**: an append that takes the engine's rows, the core's `ColumnChunk` in its signature, and the conversion itself — `rows::arrays`, `rows::schema` and `rows::batches` — because a caller who wants to see the schema the store would use, or to write the engine's rows under their own, has to be able to ask for it. |
| the store's dependencies | **The core is added.** The engine is not, and never will be. |

## The measurement

Two probes, three campaigns each, medians with the spreads, the raw reports committed, and the tables
derived from the reports by `scripts/medians.py` and checked by the guard inside the section that
publishes them.

**The build side's probe** takes the published file's six row groups as six files and answers R2's four
questions.

**The write path's probe** reads the published file through the engine's own reader, hands the engine's
own rows to the store, and compares the result with **two** files: the published file, and the file the
store's own Arrow path writes from the same rows. **The first differing byte is named**, whichever way
it comes out.

## The amendment

**Two things moved, and the reason is recorded here.**

1. **The ADRs are numbered 0053 and 0054, not 0052 and 0053.** ADR-0052 was taken by spec 041's
   correction -- *"the estimate that was derived instead of measured"* -- and the numbering follows the
   ADRs that exist, not the ones the tasks first guessed.
2. **The store's schema rule is named, and it is the store's.** The plan said the store would write the
   engine's rows *"with the store's writer and policy"* and did not say what the **schema** would be. The
   measurement found the answer and the answer is a rule: **a column that holds no NULL is written
   REQUIRED, and a column that holds one is written OPTIONAL.** The reason is in the vocabulary itself --
   the engine's rows say *which rows are NULL* and not *whether a column may be* -- and the rule's cost
   is visible in the bytes: the published file declares two columns optional that hold no NULL, so the
   file the store writes from the engine's own rows is 5,432 bytes smaller and first differs at byte
   20,566,725. **And the same rows under the published file's own schema write the published file byte
   for byte**, so the difference is the schema and nothing else. Published in PERF.md §3z and recorded in
   ADR-0054.

3. **The scope table names two things the implementation settled.** The store's surface grows by the
   **conversion** as well as by the append — `rows::arrays`, `rows::schema` and `rows::batches` — because
   a caller who wants to see the schema the store would use, or to write the engine's rows under their
   own, has to be able to ask for it. And the **set's rows are not handed out**: `execute_rows` and
   `execute_rows_on` take a dataset and a file, and the set's own `execute` still folds the rows into an
   aggregate. **That is the next increment, and it is named here so it is a decision and not an
   oversight.**

**The probes were written after the code they measure, and the reason is recorded.** The build side's
**cost** is the cost of a build side that takes a set, and the write path's probe measures the **store's
own** conversion: neither can be measured before the code exists. The house's rule is that the
**publication** waits for the measurement ✓, and it did ✓.

## The references

- [constitution.md](../../constitution.md) II — specify, plan, tasks, implement: this spec is the
  specification, and the code follows it.
- [constitution.md](../../constitution.md) IV — evidence over claims: the byte identity is measured and
  published as it comes out, not asserted.
- [constitution.md](../../constitution.md) V — symmetry: the set is read on one side and written on the
  other, and both sides are measured the same way.
- [ADR-0044](../../docs/adr/0044-the-stores-half.md) — the store's half: the grid, the commit, the
  identity claim, and the dependency rule this increment changes.
- [ADR-0046](../../docs/adr/0046-the-source-over-a-set-of-files.md) — the source over a set: the generic
  source the build side will join.
- [spec 039](../039-the-engine-reads-a-stores-table/spec.md) — the file boundary's cost: the experiment
  this increment reuses.
- [spec 040](../040-the-source-that-reads-a-set-of-files/spec.md) — the source that reads a set of files.
- [spec 041](../041-the-readers-four-gigabytes/spec.md) — the reader's four gigabytes: the bump whose
  figures every measurement here sits beside.
