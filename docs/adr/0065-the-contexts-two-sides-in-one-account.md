# ADR-0065: The context's two sides, in one account

**Status**: accepted
**Date**: 2026-11-09
**Related**: [ADR-0038](0038-the-context-taken-apart.md), [ADR-0042](0042-the-file-state-floor.md), [spec 053](../../specs/053-the-cold-paths-context/spec.md)

## Context

**The context is the engine's own account of itself, minus the same work measured on its own.** The join stage reports its
**parts** in the plan output, so the in-query cost is the engine's account of itself and not an inference; the standalone
probe measures the same work in a quiet process; and the **context** is the difference between the two.

**And the verdict was unattributed since spec 030** — on the resident path it was once *the memory's*, then *unattributed*,
and the house has said so since. [STATUS.md](../../STATUS.md)'s question 1 named what would answer it: *whether the file
state is the whole of it — the same mechanism measured on the cold path, where the verdict is still unattributed*.

**And the instrument that would answer it had a defect of its own.** [ADR-0042](0042-the-file-state-floor.md) moved the
**floors** into the iteration and back to back — the machine's own movement and the file's own read, **8.069 ms** apart —
and corrected the sentence that had claimed the pairing was already there. **But the gap's own two sides were left where
they were.** The query's account of the join stage and the standalone's account of the same work were **271.672 ms apart**
on the cold path and **325.075 ms** on the resident one.

**And a measurement taken hundreds of milliseconds away from the thing it explains is not a measurement of the thing.** Over
that distance the file's state and the machine's movement are **not common to both sides**, and so they cannot cancel in
the difference the context is. **Whatever the day's numbers said, they could not have carried a verdict about the file's
state.**

## The measurement

**The iteration is now two path accounts.** Each account holds the path's query and the path's standalone work **back to
back**, and each account carries **its own build side's work**, so that every leaf of that path's ladder is a difference of
two measurements taken inside one account. The whole iteration is **reversed** in the odd iterations, so that every
position effect lands on the other side in the next one.

**Three campaigns**, on one host, at a load average of **4.55**, **4.34** and **4.31**. The full tables are in
[PERF.md §3aj](../../PERF.md#3aj-the-contexts-two-sides-in-one-account).

| | the cold path | the resident path |
|---|---|---|
| **the context** | **0.032 ms** *(was 0.152 ms)* | **−0.712 ms** *(was −0.928 ms)* |
| **the context's spread** | **0.567 ms** *(was 0.787 ms)* | **0.718 ms** *(was 0.664 ms)* |
| **the separation** | **185.773 ms** *(was 271.672 ms)* | **92.934 ms** *(was 325.075 ms)* |
| **the separation, the query first** | **48.413 ms** | **15.746 ms** |
| **the separation, the standalone first** | **187.695 ms** | **91.776 ms** |
| **the verdict** | **unattributed** *(was unattributed)* | **the memory's** *(was unattributed)* |

**And the verdicts are published as one word and one state each** — the word is a mechanism and the state is not. The
cold path's is **unattributed** and **sound**; the resident path's is **the memory's** and **sound**.

**And the file's share of the cold path's context is published, because the question was about it.** The context's spread
is **0.567 ms**; the file's own read drifts **0.353 ms** of it and the machine's own movement **0.097 ms**, so **62.2%**
of the movement is the file's and **17.1%** is the machine's, and **0.117 ms** is left once both are taken out. **And the
context itself is 0.032 ms — inside what is left.** So the file's state is **not** shown to be the whole of the cold path's
context, **and it is not shown not to be**.

**And the resident path is where the mechanism is visible.** The query's resident probe reads a column the dataset has held
since it was loaded, with the build side's read in between, while the standalone reads its column afresh: that leaf is
**−0.884 ms** against a yardstick of **0.687 ms**, with its own spread of **0.365 ms** narrower than the yardstick. **On
the path where the two sides do not share the file's state, the file's state is the whole of the context.** On the cold
path the two sides do share it — both read the file inside the reader — so it cancels, and what is left is **0.032 ms, at
the noise**.

**And the order's own cost is published, and it is the receipt that goes with the verdict.** The order is not neutral: the
**even** iterations take the query first and the **odd** ones take it last. On the cold path the order moved the leaves by
at most **0.589 ms**, and it could not have moved any of them out of the noise. **On the resident path it moved the probe's
file state by 1.286 ms — wider than the leaf itself** — so that attribution is published with the receipt beside it: the
leaf's value is the median of the two geometries, and the query's build side's read is what moves the held column the leaf
measures.

## Decision

**The context's two sides are measured back to back, in one account, and the question closes.**

**On the cold path the verdict is unattributed at this spread**, and the reason is named: the context is **0.032 ms**
against a spread of **0.567 ms**, and the file's state accounts for **0.353 ms** of that spread without being able to be
shown to be the whole of the context.

**And the mechanism is measured, on the path where it can be seen**: the resident path's verdict is **the memory's** — the
probe's file state, **−0.884 ms** against a yardstick of **0.687 ms** — so **the file's state is the whole of the context
where the two sides do not share it, and the cold path's context is what is left after it cancels.**

**And the instrument's change is published old and new, with the reason: the pairing moved.** Every figure the pairing
touched is published beside the figure the previous pairing gave.

## Consequences

- **The open question closes, and it closes either way.** The question was *whether the file state is the whole of it*, and
  the answer is **yes where the two sides do not share the file's state, and there is nothing left to attribute where they
  do**. A question asked with a proper pairing and answered "unattributable at this spread" is closed.
- **[STATUS.md](../../STATUS.md)'s question 1 is rewritten**, and the sentences in [PERF.md §3p](../../PERF.md#3p-the-join-s-stage-measured-again),
  [§3q](../../PERF.md#3q-the-drifts-own-parts) and [§3r](../../PERF.md#3r-the-file-state-floor) that rested on the old
  pairing are **corrected where they were published**. **The correction is recorded here.**
- **The verdict is a property of the day, and the sets are published beside each other.** The resident path's verdict has
  been *the memory's* and *unattributed* on sets measured with different pairings and on different hosts; the **cold**
  path's has been unattributed throughout.
- **The floors are untouched.** They are still back to back, **7.790 ms** apart, and the interleaving's added work is still
  **0.000 ms** — it adds no clock read.
- **The crates.io publish is not waiting on this.** The crates stay unpublished while the remaining work lands.
