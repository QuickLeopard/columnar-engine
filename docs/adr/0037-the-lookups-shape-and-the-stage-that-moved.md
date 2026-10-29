# ADR-0037 — The lookup's shape, and the stage that moved

**Status**: accepted
**Date**: 2026-10-29
**Spec**: [spec 029](../../specs/029-the-lookup-shape-and-the-stage-that-moved/spec.md)
**Predecessor**: [ADR-0036](0036-the-probes-remainder-taken.md)

## Context

[ADR-0036](0036-the-probes-remainder-taken.md) took the probe's remainder apart and found that the
overflow map was never it: the map's share is **0.020 ms** of a **3.337 ms** remainder, at or below the
noise. What was left was **named** — the method over `&self`, the `Option` chain, the `u32` width and the
saturating add — and a name is not a measurement.

It also left a second thing open. The change that removed the map's consult was a simplification, and it
bought nothing the query can see: the probe's own measurement was **0.950 ms** faster, the **floor** —
the table's own load, one a probe row, which the change cannot touch — moved **1.514 ms** slower in the
same two sessions, and the **resident** join stage moved **22.106 ms** against **19.797 ms**. The parts
could not account for it, and ADR-0036 did not claim what it was.

Spec 029 took both questions: the remainder's **parts**, measured one at a time, and the movement, taken
with the floor beside it.

## Decision

**The remainder is decomposed into a ladder, and each part is measured by a candidate that differs from
the one before it in exactly one thing.** The rungs are the plain candidate, then the **chain**, the
**width**, the **state through `&self`**, the **map's `Option`**, the **engine's own lookup** over the
instrument's own key source, and the engine's own probe over the engine's own key column. Every rung's
rows are checked against the engine's rows, in order, before anything is timed.

**Two things that are not differences of the code are removed from the measurement.**

- **The crate boundary.** The instrument is another crate and the engine's lookup is a **private**
  method, so across the boundary it was not inlined: the instrument paid a **call a probe row** that the
  engine's own query does not pay. The lookup is now marked `#[inline]`. It is a small method called six
  million times over this data, and the attribute changes nothing the query does — the query's lookup was
  already inlined there.
- **The buffer.** The engine's query materialises the probe column and then probes it, so the column it
  probes is one the reader has **just written**; the candidate read a buffer kept warm across the
  iterations. Every rung now materialises the column afresh.

**The lookup does not change.** The shape's parts are inside the remainder's own spread, and the engine's
own probe is already as fast as the hand-written lookup over the same rows, the same table and the same
column. The requirement asks for a change only when the measured saving is outside the remainder's own
spread, and there is none.

**The movement was the host's.** The two codes were measured **in the same minutes**, three campaigns
each, interleaved, with the same instrument: the resident stages are **19.108 ms** and **18.976 ms**, and
the floor **5.754 ms** and **5.690 ms**. In ADR-0036's session the stage moved **2.309 ms** and the
floor **1.514 ms**, in the same direction and the same order; here neither moves.

## Consequences

**The remainder is 0.109 ms, with a spread of 1.083 ms — and the engine's probe (8.395 ms) is as fast as
the hand-written candidate (8.315 ms).** The remainder spec 026 and spec 028 published was
**3.337 ms**, and two things that are not differences of the code were in it: the crate boundary
(**+1.332 ms**, measured in the old code's ladder) and the buffer (**1.158 ms**, the candidate's figure
moving from **7.157 ms** to **8.315 ms**). The figures are published old-and-new, with the reason, and
the sets that carried the old ones are kept under their own names.

**The lookup's shape is free, and the code that ships says so.** The chain's cost is **−0.036 ms**, the
width's **+0.956 ms**, the state's **−0.840 ms**, the map's option's **−0.108 ms**, the engine's loop's
**−0.142 ms** and the key source's **+0.121 ms**: the width and the state cancel, and every one of them
is inside the remainder's own spread.

**The context is named, and it is not the probe.** The stage's residual — the stage less the build side's
whole and its open — is **7.259 ms** resident and **10.160 ms** cold, and the gap between it and the
standalone probe is the **context**: the price of running inside a query rather than in a quiet process.
It is **−1.069 ms** on the code that ships and **−2.409 ms** on the old one — the crate boundary again,
inside the context.

**And the names are corrected where they are published.** ADR-0036 named four parts of the shape; two of
them are **not on the fast path** of the code that ships — the saturating add and the map's consult are
both behind the early return — and the one that is on it, the map's `Option` discriminant, had never been
measured. It is measured now.

**The next question** is the **context**: the gap between the stage's own probe and the standalone probe
of the same work, which is worth **1.069 ms** on the resident path and changes sign between sets. It is
not written down as a spec yet.

## References

- [ADR-0035](0035-the-join-stage-taken-apart.md) — the join's stage, taken apart, and the remainder this
  spec measures.
- [ADR-0036](0036-the-probes-remainder-taken.md) — the probe's remainder, taken: the map was never it,
  and the movement it left is this spec's second question.
- [spec 021](../../specs/021-the-hot-paths-taken-apart/spec.md) — the join's probe measured against its
  floor, and the table's shape chosen by measurement.
- [spec 028](../../specs/028-the-probes-remainder/spec.md) — the probe's remainder, the map's share, and
  the movement this spec takes up.
- [`PERF.md`](../../PERF.md) §3o — the tables these figures are published in.
