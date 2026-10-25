# ADR-0010: The pushdown implements the engine's filter semantics, not Arrow's

**Status**: accepted
**Date**: 2026-10-24
**Related**: [ADR-0008](0008-eager-parquet-decode.md) (the eager decode this removes), [spec 002](../../specs/002-lazy-columnar-scan/spec.md)

## Context

Spec 002 replaces the eager decode with a lazy, column-selective scan that pushes the plan's predicates into the Parquet reader. The reader is Arrow's, and Arrow offers two ways to express a filter:

1. `RowFilter` with an `ArrowPredicate` — a predicate evaluated over Arrow arrays, with the reader dropping the rows it rejects.
2. The arrow-row format's `RowPredicate` — a predicate over *row-encoded* data, which the reader can also use against the page index to skip pages.

Option 2 would be faster, because it prunes at page granularity. It was not chosen, and this ADR is about why: **the two options do not compute the same function.**

### The engine's rules

The engine's filter has two rules that are stricter than SQL:

- A row whose validity bit is clear is NULL and **never matches**, for every operator, **including `!=`**.
- A comparison involving a NaN value or a NaN threshold **never matches**, for every operator, **including `!=`**.

These are not incidental. They are what the comparison kernel does, and the differential suite holds the kernel to them.

### Arrow's rules

Arrow's comparison kernels are documented to use **IEEE 754 totalOrder** for floating-point values — `f32::total_cmp` and `f64::total_cmp`:

> For floating values like f32 and f64, this comparison produces an ordering in accordance to the totalOrder predicate as defined in the IEEE 754 (2008 revision) floating point standard. Note that totalOrder treats positive and negative zeros as different.

Three consequences, and each of them is a divergence from the engine:

| | Arrow's kernel | the engine |
|---|---|---|
| `NaN != 5.0` | **true** — totalOrder puts NaN above every value, so it is not equal to anything | **false** — a NaN never matches |
| `NaN > 5.0` | **true** — NaN is the largest value there is | **false** |
| `-0.0 == 0.0` | **false** — totalOrder distinguishes the zeros | **true** |

The arrow-row format's `RowMatcher` has the same property: the row format encodes floats in a total-order-preserving bit pattern, so its ordering agrees with `total_cmp` and disagrees with the engine in the same three places.

A single NaN in a column, or a single `-0.0`, would therefore produce a different answer depending on which path ran it. With six million rows of `l_discount` drawn from a uniform distribution, a NaN is not an exotic event — and the engine's whole claim is that its filter is *stricter* than SQL's on exactly these values. Pushing Arrow's semantics down would quietly drop that claim for the lazy path only, which is the worst possible outcome: two paths, same query, different answers.

### The NULL rule is not the problem

It is worth being precise about the one rule where Arrow agrees:

The `ArrowPredicate` contract says

> true: the row should be returned
> false or null: the row should not be returned

So a predicate that answers `null` for a NULL row **drops** it, and a NULL row is dropped under every operator. That is the same answer the engine gives.

The divergence is entirely in the floating-point rules, and in the row format's lack of a NULL concept in the same sense: in the row format a null is a null *bit*, and the matcher treats a null row as sorting before every value, so under `Ne` a null row is "not equal" to the threshold and is **kept**. The engine drops it.

## Decision

**Implement the pushdown as an `ArrowPredicate` that computes the engine's answer, and never as a row-format `RowPredicate`.**

The pushdown:

- returns `false` for a NULL row, for every operator — the engine's rule, and Arrow's contract;
- returns `false` for a NaN value or a NaN threshold, for every operator — the engine's rule, and *not* totalOrder's;
- compares `-0.0` and `+0.0` as equal, because it uses the ordinary IEEE comparison and not `total_cmp`;
- compares a dictionary-encoded column **once per distinct value** and then maps the answer back through the indices, which is what the engine's own dictionary path does;
- compares strings byte-wise, which is what Rust's `str` ordering is and what DuckDB's binary collation is.

**Accept the cost: no page-level pruning.** The `ArrowPredicate` path evaluates the predicate over the decoded batch, so the reader fetches the pages of the predicate's column. DuckDB prunes pages using the column index, and the engine does not. That is a real gap and it is measured, in `PERF.md`.

**Then check the equivalence, rather than assert it.** The differential test in `crates/columnar-engine/tests/lazy_scan.rs` answers every operator, against NULLs, NaNs, a NaN threshold, a dictionary column and a plain UTF-8 column, over row counts of 0, 1, 2, 63, 64, 65, 127, 128, 129, 191, 192 and 193 — the boundaries of the 64-row block the kernel works in — and requires the lazy path and the eager path to agree, row for row, every time. `crates/columnar-cli/tests/lazy_scan.rs` does the same over the real generated file with the real Q6 plan.

## Consequences

**Good.**

- The two paths provably answer the same question. Not "the same question, probably" — the same question, checked on every operator and every edge the rules mention.
- The pushdown never uses a comparison whose semantics differ from the engine's, so there is no path by which the reader can quietly answer a different question than the filter would have.
- The engine's NULL and NaN rules survive the change. They were the engine's rules before the pushdown, and they are the engine's rules after it.

**Bad, and accepted.**

- **No page-level pruning.** DuckDB reads the pages its predicates exclude; the engine reads all the pages of the columns it needs. On the TPC-H `lineitem` file this is part of the remaining difference, and `PERF.md` measures it rather than describing it.
- **The predicate is evaluated over decoded Arrow arrays**, one row at a time. The engine's SIMD comparison kernel works on the engine's own chunk format, so it cannot be used here without first converting the column — which would cost more than the scalar comparison saves. The pushdown is therefore scalar, and the filter that runs behind it (when a predicate is not pushed) is not.

**Neutral.**

- The row-format `RowPredicate` remains available for a future increment. Using it would mean either changing the engine's NaN rule — which would change every published number — or using the row-format predicate *only* for predicates whose column cannot contain a NaN, which is a narrower win than it sounds.

## Alternatives considered

**Use the row-format `RowPredicate` for its page pruning.** Rejected. It computes a different function: under its total order a NaN is the largest value there is, so `NaN != 5.0` matches, where the engine says it never does. The page-level saving is real and it is measured — but not at the price of answering a different question.

**Use Arrow's own comparison kernels inside the pushdown.** Rejected. They use totalOrder, so they disagree with the engine on NaN and on the signed zeros. The pushdown has to produce the engine's boolean, so it has to do the comparison itself.

**Push nothing, and only project.** Rejected as the whole answer. Projection alone is most of the *byte* saving, but it leaves the reader fetching every page of the columns it needs and materialising every row of them. Pushdown is what makes the surviving rows the only ones that are ever decoded.
