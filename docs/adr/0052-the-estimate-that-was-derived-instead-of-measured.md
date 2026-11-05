# ADR-0052 — The estimate that was derived instead of measured

**Status**: accepted
**Date**: 2026-11-05
**Spec**: [041-the-readers-four-gigabytes](../../specs/041-the-readers-four-gigabytes/spec.md)

## Context

**`PERF.md` §3i published a memory estimate that the engine never made.**

The curve measures the real Q6 at three dataset sizes, and the largest size does not fit the engine's
**default** memory budget: the query is refused before a row is read. The refusal was published with a figure:

> The estimate is about 16 bytes a row — the materialised column, the mask and the machinery — so
> 24 million rows need roughly **391 MiB** against the **256 MiB** default.

**The figure was derived, not measured.** The engine estimates the working set before the read and
**reports** the estimate in its plan output; the section quoted a hand multiplication instead of that
number.

**And the hand multiplication was out.** The engine's own estimate on the 24-million-row dataset is
**377.7 MiB** — 16.5 bytes a row — and 16.5 × 24,000,000 is 377.7 MiB, not 391. The published figure was
**13.3 MiB** above the engine's, which is 3.5%.

**The claim it supported was true; the number was not.** The engine **does** refuse the query at the
default budget at 24 million rows, and the reason — the estimate does not fit — is exactly as published.
Only the figure was wrong.

**The bump did not move it.** The estimate is made from the file's **row count** and its **schema**, and
the bump changed the file's *metadata* and not its rows or its columns. Measured against the tree at spec
014 — the commit that published the claim — the estimate on this dataset is **377.7 MiB** as well: the
engine said 377.7 MiB before the bump too. **The figure was wrong before the bump**, and the bump's
re-measurement is what put a number next to it that could be checked.

**The two places it was published.** `PERF.md` §3i and [ADR-0021](0021-the-materialised-column-and-a-rust-peer.md),
which carried the sentence with the curve's own table beside it.

## Decision

1. **The figure is the engine's own estimate, and the report carries it.** The curve's report records the
   estimate at every point — `engine_estimate_bytes` — beside the budget the campaign ran under, so the
   refusal is a number in a committed report and not a derivation in a paragraph.

2. **The correction is published where the figure was published.** §3i now publishes **377.7 MiB** and says
   what it published before and why; ADR-0021's paragraph says the same in the voice it was written in.

3. **The bump is not the reason.** The estimate is made from the row count and the schema, neither of which
   the bump changed, and the estimate on the pre-bump tree is the same figure. The correction is of a
   pre-existing derivation, and §3i says so rather than filing it under the bump.

4. **The claim keeps its shape.** The engine's default budget refuses the real Q6 at 24 million rows and
   accepts it at 12 million, where the estimate is **188.8 MiB** against the **256 MiB** default. That is
   what the paragraph was for, and it is now true with the right number.

## Consequences

- **The curve's report grew two fields.** `engine_estimate_bytes` on every point and `host_load_average`
  on the report, the latter so that a reader can tell a campaign taken on a busy host from one taken on a
  quiet one. The report's schema change is in `columnar-bench`, which is not the engine's surface.

- **The guard checks the figure where it is published.** `scripts/check-published-figures.py` reads the
  estimate out of the committed curve reports and checks it inside §3i, so a figure that drifts from the
  report fails the gate.

- **The engine's behaviour did not change.** The estimate is computed by the same code it was computed by;
  the section and the ADR that quoted it are what changed.

- **A derived number is a number with no report behind it.** This is the second correction this increment
  has made ([ADR-0049](0049-the-readers-four-gigabytes.md) corrected the hostile file's declared length),
  and the rule they share is the same: **a figure that is derived is a figure that has to be checked**.

## References

- [`constitution.md`](../../constitution.md) IV — evidence over claims: a derived figure published as a
  measured one is a claim without evidence, and it is corrected where it was published.
- [`constitution.md`](../../constitution.md) VI — corrections in place: the wrong figure is corrected in the
  document that published it, and this ADR records the correction.
- [`spec 041`](../../specs/041-the-readers-four-gigabytes/spec.md) — the reader's four gigabytes, and the
  amendment that records this.
- [ADR-0021](0021-the-materialised-column-and-a-rust-peer.md) — the paragraph that published the figure.
- [`crates/columnar-bench/src/size.rs`](../../crates/columnar-bench/src/size.rs) — the report type, and the
  two fields this ADR adds.
- [`crates/columnar-engine/src/memory.rs`](../../crates/columnar-engine/src/memory.rs) — the estimator, and
  the row count and schema it is made from.
- [`PERF.md` §3i](../../PERF.md#3i-the-curve-over-the-size-of-the-data) — the curve, and the corrected
  figure.
