# ADR-0005: The engine's internal representation is its own column chunks, not Arrow arrays

**Status**: accepted
**Date**: 2026-10-23
**Reviewers**: TBD

## Context

The project reads Parquet and compares itself against DuckDB and Polars, both of which are Arrow
implementations. The obvious design is to use `arrow`'s array types (`Int64Array`,
`DictionaryArray`, `BooleanArray`, …) as the engine's internal representation and skip the
translation layer entirely.

That would be free: `parquet`'s Arrow reader hands back Arrow arrays directly, so there is no
conversion, no second set of types, and no risk of the two representations disagreeing.

## Decision

**The engine has its own internal representation**: `columnar_core::ColumnChunk` with an explicit
`Encoding` enum, `columnar_core::NullBitmap`, and `columnar_core::SelectionVector`.

Arrow appears **only at the I/O boundary**, in `columnar-engine`'s Parquet reader and in
`columnar-testdata`'s Parquet writer. Nothing above the reader knows Parquet exists, and nothing
inside the engine knows Arrow exists.

## Consequences

**Easier**: the internal representation can be shaped for the engine rather than for a general
dataframe library.

- The validity mask is a first-class type with a documented bit layout, not an
  `Option`-per-value or a separate null buffer.
- The `SelectionVector` has exactly two representations with a runtime dispatch between them,
  and the dispatch threshold is a named constant in one place. Arrow has no selection-vector
  concept at all — it would have to be invented anyway.
- Encodings are explicit: `Int64Dict { dict, indices }` says what it is, and the dictionary
  length is queryable, so the plan output can report compression.

**Harder**: a translation layer exists and has to be correct. The Parquet reader maps Arrow's
`DictionaryArray` to `Encoding::Utf8Dict` and Arrow's plain arrays to the plain encodings; a
column that arrives as a dictionary but is read as plain would silently change the engine's
behaviour.

**Given up**: the ability to hand an Arrow array to any downstream Arrow consumer, and the
temptation to treat `arrow` as the engine's data model. For this project — whose whole point is
to show that it understands what happens to data inside a machine — building the representation
is the work, not an obstacle to it.

## Measurement

The cost of this decision is one translation layer in `columnar-engine/src/dataset.rs`. The
benefit is measured by the plan output, which reports the encoding of every column and the
dictionary length of every dictionary-encoded column — information the engine has because the
representation is its own.

If the translation layer ever grows a second implementation (a second file format, or a
column-store format that is not Parquet), this ADR should be revisited: two formats would mean
the translation layer is doing real work and deserves its own crate.

## Confidence

**medium** — the decision is right for a single-format project, which is what this is. The
trigger for revisiting is written above because a project usually discovers this only after it
has grown a second format, and by then the migration is more expensive than the decision.
