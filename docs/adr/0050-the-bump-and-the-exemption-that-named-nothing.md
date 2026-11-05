# ADR-0050 — The bump, and the exemption that named nothing

**Status**: accepted
**Date**: 2026-11-05
**Spec**: [041-the-readers-four-gigabytes](../../specs/041-the-readers-four-gigabytes/spec.md)

## Context

**Three things happened in this increment, and two of them are corrections.**

**The bump.** `parquet` **54.3.1 → 58.4.1** and `arrow` **54 → 58.4.1**, in the workspace's shared
dependency table, so the version cannot drift between crates. The reason is [ADR-0049](0049-the-readers-four-gigabytes.md)'s:
**58.4.1 is the smallest jump that closes the hole by default**, because its reader does not read the
page statistics by default — `DEFAULT_READ_PAGE_STATS = false` — so a hostile file's declared length
is never parsed at all. The `parquet` 59 and 60 lines **drop `thrift` outright**; that is a bigger
jump than the evidence asks for, and this repository does not take jumps taken for tidiness.

**The churn the compiler found** is small and mechanical: `WriterProperties::set_max_row_group_size`
is now `set_max_row_group_row_count(Some(n))` — eleven places, the store's writer, the generator's
writer, the engine's source, four tests and four examples — and `chunk.encodings()` hands the
encodings back by reference, so the iterator is not copied. **And the engine's behaviour for an
honest file did not change**: the same rows, the same answers, the same plans, and the store's
acceptance still holds — the store writes the file the generator wrote, **byte for byte**.

**The first correction: the claim was published in five places, not three.** [ADR-0049](0049-the-readers-four-gigabytes.md)
and spec 041's R2 named three — spec 034's R10, `STATUS.md`'s open questions and `deny.toml`'s
comment. The engine's own `README.md` and [`docs/embedding.md`](../../docs/embedding.md) published
the same sentence — *the file's **metadata** can ask for four gigabytes; the fix is `thrift` ≥ 0.23* —
and they were not in the list. They are corrected in the places that published them, and the count
the spec carries is corrected with them.

**The second correction: ADR-0049's reason for removing the exemption was wrong.** Its consequences
say: *"the advisory names `thrift` 0.17.0, and the bump takes `thrift` out of the graph — so an
exemption that names a crate the graph does not contain is removed, with the reason."*

**The bump does not take `thrift` out of the graph.** `parquet` 58.4.1 depends on `thrift` **0.23.0**,
and the graph's `thrift` is **0.23.0**. The exemption is removed for a different reason, and
`cargo deny` says it itself: with the exemption in place it reports

```text
warning[advisory-not-detected]: advisory was not encountered
  ignore = ["RUSTSEC-2024-0436", "RUSTSEC-2026-0435"]
                                no crate matched advisory criteria
```

The advisory's criteria cover `thrift` **0.17.0**; the crate in the graph is **0.23.0**; so the
exemption **named nothing**. An exemption that names nothing is a silence, so it goes — **but the
reason it goes is the one above, and not the one ADR-0049 wrote.**

## Decision

1. **The exemption of `RUSTSEC-2026-0435` is removed, and the reason beside it is the one that is
   true**: the advisory does not match the crate in the graph, so it is watched rather than muted.
   The `paste` exemption — `RUSTSEC-2024-0436`, a build-time macro of the Parquet stack that is not
   shipped code — **stays**, with its reason unchanged.

2. **The claim is corrected in the two places spec 041 did not name** — the engine's `README.md` and
   `docs/embedding.md` — and each correction says what the measurement says: the **page headers**
   are the hole, the metadata path is **bounded**, and the fix is `parquet`'s own reader rather than
   thrift's opt-in limit.

3. **The bump's figures are published old and new**, in the sections that publish them, with the
   reason — and **the host's share of every difference is separated from the code's** by the
   incumbents, which the bump cannot have touched. The tables are in
   [`PERF.md` §3x](../../PERF.md#3x-the-bump-and-the-figures-it-moved).

4. **The engine adds no protection of its own.** The reader is where the hole was, and the reader is
   where the fix is: no wrapper, no bound, no check of the engine's own.

## Consequences

- **The hole is closed, and the closing is measured.** The same probe, the same hostile file and the
  same campaigns on the bumped tree: the largest single allocation the reader asks for is
  **8,192 bytes at every declared length** — the honest file's own figure — against **the declared
  length** before the bump, a share of **1.0** at every point of the curve. The section that
  publishes it is [`PERF.md` §3w](../../PERF.md#3w-the-readers-four-gigabytes).

- **The published figures move, because Arrow's decode is measured.** The engine's cold path is
  **faster** after the bump, and the incumbents — which the bump cannot have touched — are the
  control that says how much of the movement is the host's.

- **The surface does not move**: `execute_on` still takes a `ParquetSource`, the surface is still
  **46 names**, and the read path's semantics for an honest file are unchanged — which the
  repository's own differential suite says, row by row.

- **The `parquet` 59 and 60 lines are not taken.** They are a bigger jump than the hole asks for, and
  a bump taken for tidiness is a bump whose figures have to be measured for nothing.

## References

- [`constitution.md`](../../constitution.md) IV — evidence over claims: the finding is measured, and
  two published sentences were corrected by the measurement.
- [`constitution.md`](../../constitution.md) VII — explicit scope, no silent deferral: what the bump
  does not buy is named.
- [`spec 041`](../../specs/041-the-readers-four-gigabytes/spec.md) — the reader's four gigabytes.
- [ADR-0049](0049-the-readers-four-gigabytes.md) — the hole, and the fix — and the consequence this
  ADR corrects.
- [`PERF.md` §3w](../../PERF.md#3w-the-readers-four-gigabytes) — the hostile file, before and after.
- [`PERF.md` §3x](../../PERF.md#3x-the-bump-and-the-figures-it-moved) — the figures the bump moved.
- [`deny.toml`](../../deny.toml) — the advisory, the exemption that stays, and the one that goes.
- [`crates/columnar-engine/README.md`](../../crates/columnar-engine/README.md) and
  [`docs/embedding.md`](../../docs/embedding.md) — the two places the claim was published besides the
  three the spec named.
