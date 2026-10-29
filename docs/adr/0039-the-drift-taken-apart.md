# ADR-0039 — The drift, taken apart

**Status**: accepted
**Date**: 2026-10-29
**Spec**: [spec 033](../../specs/033-the-drift-taken-apart/spec.md)
**Predecessor**: [ADR-0038](0038-the-context-taken-apart.md)

## Context

[ADR-0038](0038-the-context-taken-apart.md) took the **context** apart. The join stage's plan output
reports its parts, the parts tile the context exactly, the **reconciliation is 0.000 ms** on both paths,
and the verdict was **`unattributed`** on both: the context is **0.433 ms** on the cold path against a
spread of **2.211 ms**, and **−1.565 ms** on the resident path against a spread of **0.418 ms**. The
context was a **real thing** on the resident path — outside its own band — but **every** part's own spread
was wider than that band, so **no mechanism could be claimed**.

**And the band was never a measurement of the machine.** It is the **context's own spread**, which is a
**difference of two series** taken a campaign apart — and the difference of two drifting series can be
narrow, or zero, while the machine drifts a millisecond. On the resident path of the set spec 030
published, the band was **0.418 ms** against the machine's own movement over the same three campaigns of
**0.936 ms**: **narrower than half the noise it was meant to measure**. The part that could have carried
the context — the **probe's file state**, **−1.178 ms** — was ruled **inconclusive** by that band, and
**the part was not too small; the band was too narrow.**

**And this was the method's shape, not the code's.** Over **every** set the repository keeps that carries
a context, in **three of the eight cells** the band is narrower than the machine's own movement. **A
yardstick is a measurement, and this one was never measured.** And the published verdict said less than it
appeared to: `unattributed` says "no part accounted for more of the context than the band" — it does not
say "the context has no mechanism", and the reader could not tell the two apart.

**The question this spec takes up: the band was the machine's own noise, so what is the machine's own
noise, and can the parts be judged against it?**

## Decision

### 1. The yardstick is the machine's own drift, and the drift is measured on both sides

Every measurement the instrument takes now has a **drift**: the **range** of its per-campaign values
across the set's three campaigns. The drift is the machine's own movement, measured beside the thing it
is used to judge, and it is **published**.

**The yardstick for a leaf is the sum of the drifts of the two measurements it stands on, and the floor's
drift, and the larger of the two.** The **sum** is the conservative choice and it is an **amendment**: the
pre-registered rule took the **larger** of the two, and the reason it was changed is recorded in the spec.
A yardstick judges a **difference**, and the difference of two drifting series moves by up to the **sum**
of the two drifts — so the larger of them was never an upper bound on what the machine could have
produced on its own. The floor's drift is the second floor: it is the machine's own measure of what a
memory-bound loop costs on this host, and it is the number every **cost** is compared with, because a cost
is not a leaf and has no yardstick of its own.

**The pre-registered yardsticks are published old-and-new, with the reason.** The verdict is unchanged by
the amendment — **sound** on both paths, and the same words — because the rule takes the floor's drift as
its floor and so is sound by construction.

### 2. The two sides of every difference are measured in the same iteration, back to back

The ladder's leaves are **differences**, and a difference of two series measured a campaign apart is a
difference of two **drifting** series. So the sides are now taken **back to back, in the same
iteration**, and the machine's drift while an iteration runs can no longer land on one side of a leaf.

**And the order is alternated between the iterations**: the **even** iterations take the query first and
the **odd** ones take it last. The **order's own cost** — the difference between the two orders' medians,
over the leaves and the context — is **published** as its own figure, so the reader can see whether the
order could have moved any of the parts.

**And the iteration count is even: six.** With five, the two orders' groups are **three and two**, and
the published median inherits the order's bias unevenly; with six they are **three and three**. This is
an amendment too, and the reason is recorded in the spec.

### 3. The verdict carries a state, and the state says whether the test could tell

The **leaf** vocabulary stays as it is — **`a part of the context`**, **`at the noise`**,
**`inconclusive`** — and the **verdict** gains a vocabulary of its own: **`sound`**, **`not sound`**,
**`inconclusive`**. **Sound** means the yardstick could measure what it judged and at least one leaf could
be judged; **not sound** means the yardstick was narrower than the machine's own movement; **inconclusive**
means no leaf could be judged at all.

**The house's own gate rule — "gates that can say 'I could not tell'; an inconclusive result must not be
reported as a pass" — is the reason.** A verdict of `unattributed` on a yardstick narrower than the
machine's noise reports a failure to find a mechanism as though a mechanism had been looked for and not
found.

**And the band-based verdict spec 030 published is kept beside it as the figure it was** — `unattributed`,
on a band of **0.418 ms** — **annotated as not sound**, and it is **not** published as a second verdict for
the same path.

### 4. The report carries the per-iteration values, and the time each block took

Every part's **per-iteration values** are in the report, and so is the time each **block** took — the
query's, the standalone's, the build's, the probe's, the controls'. The **separation** — how far apart the
machine's own two measurements of one difference were taken, in the machine's own time — is published as
its own figure, so the reader can see the scale of the drift the yardstick is drawn against.

### 5. The interleaving's added work is measured, not assumed

It adds **no clock read**: every quantity is still timed by exactly the two clock reads it was timed by
before, and the per-iteration values are **written, not timed**. The added work's cost is therefore
**0.000000 ms**, published beside the clock's own cost so the reader can see the resolution it was
measured at.

### 6. The sets are preserved under their own names

Spec 030's sets become **`join-cost-spec030-run*.json`** and **`join-cost-spec030-before-run*.json**; this
increment's set takes the plain names. Every superseded set keeps its own name, so every figure published
old-against-new stays derivable from a committed report.

### 7. The verdict's yardstick is the yardstick of the largest leaf on the path

A verdict that cannot be read without its ruler is not a verdict, so the **README's Next row** and
**`STATUS.md`'s probe row** carry the verdict's **word**, the verdict's **state** and the **yardstick** it
was made with — the yardstick of the **largest** leaf on the path, the leaf the verdict names when one
counts and the leaf that would have named it when none does.

## Consequences

**The campaign** is three campaigns of six iterations, taken on a **clean tree**, with the floor in every
campaign, the reconciliation published, and the per-iteration values in the report. The host's load
average during the campaigns was **3.66**, **3.85** and **4.34**, and every report carries the load average
it was taken under.

**The verdicts:**

| the path | the context | the yardstick | the verdict | the verdict's state |
|---|---|---|---|---|
| **cold** | **0.199 ms** (spread **0.667 ms**) | **7.860 ms** | **unattributed** | **sound** |
| **resident** | **−1.581 ms** (spread **0.356 ms**) | **0.487 ms** | **the memory's** | **sound** |

**On the resident path the mechanism is named: the memory is.** The **probe's file state** is
**−1.238 ms** against a yardstick of **0.487 ms**, with the leaf's own spread of **0.390 ms** inside it.
The query probes a column the dataset has **held** since it was loaded, and the standalone reads its
column **afresh** — that is the leaf, and it counts. **On the cold path the context is not a thing**: it is
**0.199 ms** against a yardstick of **0.878 ms**, at the noise, and the largest leaf that counts is
**none**, so the verdict is **unattributed**.

**And the ladder, with its yardsticks, the new code's set:**

| the part | cold | its yardstick | resident | its yardstick |
|---|---|---|---|---|
| the build side's gap | **0.145 ms** | 0.878 ms | **−0.091 ms** | 0.722 ms |
| the open's gap | **0.013 ms** | 0.362 ms | **0.010 ms** | 0.362 ms |
| the probe's row-number offset | **−0.572 ms** | 7.860 ms | — | — |
| the probe's file state | — | — | **−1.238 ms** | 0.487 ms |
| the probe's own remainder | **0.551 ms** | 7.894 ms | **−0.356 ms** | 0.449 ms |
| the part in no gap | **0.000 ms** | 4.731 ms | **0.000 ms** | 0.362 ms |

**And the controls, reported beside every set and never summed**: the **floor**, **6.147 ms** with a
**drift of 0.362 ms**; the **scan's stage**; and the **build side's file state**, **1.000 ms** with a
spread of **4.273 ms**.

**And the order's own cost is published, and it is a diagnostic and not a gap.** It is **0.557 ms** on the
cold path and **0.740 ms** on the resident one, against the floor's own spread of **0.362 ms**. **So the
order could have moved a part on the cold path** — and the reader can see that it did not move this one:
even if the order had moved the resident path's file state by all of its **0.740 ms**, it would still be
**−0.498 ms**, outside its yardstick of **0.487 ms**.

**And the query's cost did not change.** The query's own cost — the join stage, as the query pays it —
under the two orders the alternation makes, is **0.085 ms** apart on the cold path and **0.356 ms** apart
on the resident one, both **inside the floor's own spread** of **0.362 ms**. **The published stage is not
the order's.** The interleaving's added work is **0.000000 ms**, and the engine itself is not touched, so
no published query figure moves.

**And the answer did not change.** The join's rows, their order and their count are the same; the
aggregate's value is the same; the named tests hold — `a_build_key_more_than_255_rows_carry_still_counts_every_one`,
`a_probe_key_below_the_table_s_base_matches_nothing` and
`a_null_key_never_matches_on_either_side` in `crates/columnar-engine/tests/join.rs`, and
`the_direct_table_agrees_with_the_hash_table` and
`the_overflow_map_is_allocated_only_when_a_key_overflows` in
`crates/columnar-engine/src/operators/join.rs`. **The engine is not touched at all**: the change is the
instrument's, and the lookup's shape stands.

**And the figures that moved are published old-and-new, with the reason.** The engine's own probe was
**9.080 ms** and is **8.860 ms**, the floor was **6.282 ms** and is **6.147 ms**, the hand-written lookup
was **10.695 ms** and is **8.953 ms**, the stage was **25.446 ms** cold and is **24.690 ms**, **20.368 ms**
resident and is **19.431 ms**, the build side's whole was **12.605 ms** and is **12.347 ms**, the residual
was **7.386 ms** and is **7.187 ms**, and the context was **−1.565 ms** and is **−1.581 ms**. **The code
did not change** — the difference is the **instrument's** and the **host's**, and the two sets were not
taken in the same session: this one ran on a host at **3.66–4.34**, spec 030's at **6.80–7.36**.

**And the next question is written down where this one was** — the README's Next row and
[`STATUS.md`](../../STATUS.md)'s last section. The question this leaves is: the **drift** is now the
yardstick for everything, and **it has never been taken apart**. The machine's own floor moved
**0.362 ms** across one session's three campaigns, but the join stage moved **4.731 ms** on the cold path
and the build side's read of the build file alone moved **10.330 ms** — so **the yardstick is as wide as the
engine's own cold read makes it**, and that is why the cold path's parts cannot be told apart. **Which of
the drift's parts is the engine's, and how much of the engine's own cost moves under the measurement?**

## References

- [ADR-0038](0038-the-context-taken-apart.md) — the context taken apart, the ladder, and the band that is
  a difference of two drifts.
- [ADR-0037](0037-the-lookups-shape-and-the-stage-that-moved.md) — the lookup's shape, the two artefacts of
  the instrument, and the context spec 030 took up.
- [ADR-0035](0035-the-join-stage-taken-apart.md) — the join's stage, taken apart, and the residual the
  context is measured against.
- [spec 030](../../specs/030-the-context-taken-apart/spec.md) — the context taken apart, the ladder's parts,
  and the yardstick this spec replaces.
- [spec 029](../../specs/029-the-lookup-shape-and-the-stage-that-moved/spec.md) — the lookup's shape, the
  crate boundary, and the figures spec 030 took up.
- [`PERF.md`](../../PERF.md) §3o — the tables these figures are published in.
