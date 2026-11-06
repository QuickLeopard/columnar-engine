# ADR-0054: The engine's rows reach a store's table

**Status**: accepted
**Date**: 2026-11-06
**Reviewers**: TBD

## Context

Spec 039 made the engine read a store's table; spec 037 and 038 made the store write one. **Nothing
joined the two ends**: the engine's rows were the engine's own vocabulary — a `ColumnChunk` with an
`Encoding` and a validity mask — and the store wrote Arrow's `RecordBatch`es. Somebody had to turn one
into the other, and the store's claim — *"fed the same rows, it writes the same file, byte for byte"* —
had only ever been measured through Arrow.

Three places could hold the conversion, and the choice had consequences for every one of them:

- **The core** (`columnar-core`) has **no dependencies at all**: *"a vocabulary that depends on a framework
  is not a vocabulary"*. Arrow is a framework, so the conversion cannot live there.
- **The engine** would have to carry Arrow in its surface, and the engine's surface is thin on purpose:
  *"every type that appears in the public signature of a surface item is on it"*.
- **The store** already speaks Arrow — it takes `RecordBatch`es and writes Parquet — so the conversion
  belongs there.

But the store's dependency rule was published in three places: ADR-0044's decision 1, ADR-0047's
correction, `STATUS.md`, the store's `Cargo.toml`, its README and `docs/embedding.md` all said **the
store depends on nothing in this workspace**. Moving the conversion into the store **changes that rule**,
and every place that published it has to be corrected.

And there was a question nobody had answered: **does the engine's own rows write the published file byte
for byte?** The engine's eager read merges every batch's dictionary into **one dictionary for the whole
file**, while the published file's dictionary pages are per row group — so the file-wide dictionary might
have decided the bytes. Nobody knew.

## Decision

1. **The conversion lives in the store**, and the store depends on `columnar-core` — the vocabulary, and
   nothing else in this workspace. The edge is to the **core**, not to the engine: the engine is still
   below the store in the dependency direction, the publish order
   (`columnar-core`, `columnar-kernels`, `columnar-store`, `columnar-engine`) is unchanged, and the
   engine's read path cannot move because of anything in the store.

2. **The store gains an append that takes the engine's rows**: `Store::append_rows`. It is **the same
   rule with one implementation** as the Arrow append — the rows are converted, re-batched on the store's
   grid, written with the store's writer and the caller's policy, and committed. **The grid is the
   store's**: the engine does not learn it, and the rows are re-batched inside the store.

3. **The store's dependency rule is corrected where it was published.** ADR-0044's decision 1, ADR-0047's
   correction, `STATUS.md`, the store's `Cargo.toml`, its README, `docs/embedding.md` and the workspace
   manifest all said the store depends on nothing in this workspace; **that is no longer true**, and each
   sentence is corrected in the place that published it.

4. **The engine gains a way to hand a plan's rows out.** `execute_rows` and `execute_rows_on` return the
   plan's **projection**, gathered, as the surface's `Dataset`. **A plan with an aggregation is refused**:
   the aggregate folds the rows into groups, and there is nothing left to write. The gather **loses the
   dictionary** — that is what a gather does, not a loss the conversion makes — so a plan's rows are
   plain columns and the file they write is a different file from the one they came from.

5. **The schema is the store's own, and the rule is published.** The engine's rows say **which rows are
   NULL**; they do not say **whether a column may be NULL**, and the Parquet writer makes that difference
   visible in the bytes — an optional column carries definition levels and a required one does not. So the
   store's rule is: **a column that holds no NULL is written REQUIRED**, and a column that holds one is
   written OPTIONAL. The rule is the same for every write, and it is stated because it is the one thing
   about the engine's rows that the engine's vocabulary cannot carry.

## Consequences

**What becomes easier.** The engine's rows reach a store's table in two lines, and the conversion is one
implementation the store owns: the store's own Arrow path and the engine's path write through the same
writer, the same grid, the same policy and the same commit.

**What becomes harder.** The store depends on the core, so the publish order's tie is real again: the
store and the kernels are ready at once, and the list in `scripts/check-package.sh` breaks it, as it did
before ADR-0047.

**What we gave up.** The engine's rows cannot carry a column's nullability, so a file written from them
under the store's rule is **not** the file they were read from — and the store cannot write a file with a
nullability the rows do not carry. The caller who needs that exact schema writes the file through the
Arrow path, where the schema comes with the rows.

## Measurement

**The conversion's cost and the three files** are in [PERF.md §3z](../../PERF.md).

- **The conversion is exact.** The store's own conversion, under the **published file's own schema**,
  writes the published file **byte for byte**: 242,859,461 bytes, no differing byte. So the engine's rows
  are the file's rows, and the conversion is value for value.
- **The store's own `append_rows` writes a different file**: 242,854,029 bytes, first difference at
  **20,566,725**, and the difference is the schema's nullability alone — the published file declares two
  columns optional that hold no NULL, and the store's rule writes them required. **5,432 bytes** of
  difference, and every one of them is the schema.
- **The store's Arrow path is unchanged**: the published file, byte for byte, as it always was.
- **A plan's rows are a different file by construction**: 115,821 rows, 982,089 bytes, with the
  dictionary materialised away.

The cost: the engine's eager read **547.7 ms**, the conversion **149.8 ms**, and the store's own API
**1,826.6 ms** end to end — the conversion is **8.2%** of the write it serves.

## Confidence

**high** — the conversion is measured value for value, the identity is exact, and the one difference is
named and explained. The one thing that could change the decision is a caller who needs a schema the
rows cannot carry; that caller writes through the Arrow path, and the store's rule is published so that
the choice is not a surprise.
