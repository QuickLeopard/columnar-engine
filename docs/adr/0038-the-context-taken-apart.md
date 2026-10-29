# ADR-0038 — The context, taken apart

**Status**: accepted
**Date**: 2026-10-29
**Spec**: [spec 030](../../specs/030-the-context-taken-apart/spec.md)
**Predecessor**: [ADR-0037](0037-the-lookups-shape-and-the-stage-that-moved.md)

## Context

[ADR-0037](0037-the-lookups-shape-and-the-stage-that-moved.md) took the lookup's shape apart, found it
free, and named the last unnamed thing in the join's stage: the **context** — the gap between the join
stage's own **residual** and the same work measured outside the query. It left three things open.

- **The name is not what it was published as.** It was called "the price of running inside a query
  rather than in a quiet process", and it is not that. It is a **difference of two numbers taken in two
  different contexts**: the stage, measured **in** the query, less the build side, measured **outside**
  it, less the probe, measured **outside** it. The stage's clock and the two outside measurements'
  clocks **do not bracket the same work**. So the context is the sum of **two** differences that had
  never been measured apart — the build side's in-query cost against its standalone cost, and the
  probe's in-query cost against its standalone cost — and of whatever the two clocks bracket
  differently, part by part. **Neither had been measured.**
- **It changed sign between the two code sets.** The cold context is **positive** on the code that
  ships and **negative** on the code it replaces, in the same session, three consistent campaigns each.
  A cost that flips sign like that is not a fixed overhead, and nothing had said why.
- **The residual is not "the stage's own probe".** The stage's clock carries the build side's work, and
  the residual is what is left after the **standalone** build side is subtracted. So the residual is the
  probe's in-query cost **plus** the build side's own in-query-minus-outside gap. Nothing published had
  separated the two, and the documents call the residual "the stage's own probe" — an attribution the
  measurement had not earned.

**The question this spec takes up: what is the context made of, and can it be attributed?**

**And the spec's own hypothesis about the file's state was wrong.** The hypothesis said the query reads
the build side's file once, cold. The code says otherwise: the campaign runs the query **five times**,
and every execution reads that file, so the published figure — the median of five — is over a **warm**
file, and the standalone's measurement is over a warmer one still. The two sides read the file in the
**same state**, so the file's state **cannot** explain any of the build side's gap. And it is worth a
great deal: the instrument measures it, by taking one read of the build side's file before anything
else reads it, and it comes to **6.8 ms**.

## Decision

### 1. R3's route is the plan output's parts

The join stage now reports its parts — `build_elapsed`, `probe_elapsed`, `open_elapsed` — in
`explain::JoinReport`, which is a public struct.

**On the cold path this costs no new clock**: the build side's construction and the probe were already
timed apart, and the stage's clock only summed them. **On the resident path it costs two clock reads
and one subtraction** around `build_table`, measured at **0.000026 ms** — two reads at **0.000013 ms**
each — against a threshold of **the floor's own spread** across the campaigns of that set.

**The rendered plan output does not change**: the parts are in the struct, and the text is the text.
**The public shape does change**, and the spec's scope records that as an amendment, with the reason.

### 2. The marginal query route is refused, and the reason is published

It needs the build side's in-query cost to be **shown** to be the same in both queries, and the build
side's **file state is worth 6.8 ms** — so the two queries cannot be shown to have the same build
side, and the fixed cost they share would have to be **measured**, not assumed. **The route that
assumes it is not the route that measures it.**

### 3. The ladder is built per iteration

The stage's elapsed is the three parts **added together** in every iteration, and a **median of a sum
is not the sum of the medians**. So the parts tile the context exactly, the **reconciliation** is
published, and it is **zero** on both paths. The difference between the published parts' sum and the
published context is **the reduction's own noise**, and it is published under that name rather than
folded away.

### 4. The context is decomposed into three gaps, and each gap is attributed to leaves

The three gaps are the **build side's**, the **open's** and the **probe's**.

- **The build side's gap is the build side's own.** Both sides read the build side's file **warm**,
  because the campaign runs the query **five times** and every execution reads that file, so the
  published figure — the median of five — is over a warm file, and the standalone's measurement is
  over a warmer one still. The file's state therefore **cannot** explain any of the build side's gap.
  It is published as the **bound** on what it could explain, and not subtracted from the gap: a
  subtracted leaf would say the gap is explained by a file state that both sides share.
- **The open's gap is the clock's.** The stage's clock carries the build side's file open, and the
  standalone build side's does not.
- **The probe's gap is the probe's own, in three leaves**: the **probe's file state** (the resident
  path's probe reads a column the dataset has held since it was loaded, with the build side's read in
  between, while the standalone reads its column afresh), the **probe's row-number offset** (the
  standalone's cold probe carries the offset arithmetic and the engine's own probe clock does not —
  `keep_matching_rows` applies it **outside** `filter.keep`'s clock), and **what is left of it**, the
  probe's own remainder.

### 5. The parts are read by name, out of the plan output's own `Debug` rendering

The instrument reads the engine's parts **by name**, out of the plan output's own `Debug` rendering, and
**not by field**. The A/B's two codes are measured with a **byte-identical** instrument, and a field the
older code does not have cannot be named in a source file that has to compile against both. A name that
is not in the rendering is not a duration that engine reported, so the older code's parts are
**absent** rather than wrong.

### 6. The report names the code each campaign measured, and whether the engine reported its parts

**The commit alone does not distinguish two codes**: spec 029's before-set and its after-set carry
**the same commit**, because the code that was replaced was never committed in that state.

## Consequences

**The campaign is three campaigns each, interleaved** — old, new, old, new — in one session, with the
**floor** in every campaign, and the instrument **byte-identical** in both sets. Every published figure
is the **median of the per-campaign values**, and the spread is their **range**. The host's load average
during the new set's campaigns was **6.80**, **6.99** and **7.36**, and every report carries the load
average it was taken under.

**The context's figures:**

| | the cold path | the resident path |
|---|---|---|
| the stage's residual | **12.896 ms** (spread **3.336 ms**) | **7.386 ms** (spread **0.612 ms**) |
| **the context** | **+0.433 ms** (spread **2.211 ms**) | **−1.565 ms** (spread **0.418 ms**) |

**And the ladder's parts**, the new code's set:

| the part | cold | resident |
|---|---|---|
| the build side's gap | **−0.450 ms** (spread **0.660 ms**) | **−0.343 ms** (spread **0.435 ms**) |
| the open's gap | **+0.094 ms** (spread **0.007 ms**) | **+0.088 ms** (spread **0.007 ms**) |
| the probe's gap | **+0.308 ms** (spread **2.171 ms**) | **−1.376 ms** (spread **1.912 ms**) |
| the probe's file state | — (not a cold-path leaf) | **−1.178 ms** (spread **1.398 ms**) |
| the probe's row-number offset | **+0.118 ms** (spread **2.516 ms**) | — (not a resident-path leaf) |
| the probe's own remainder | **+0.189 ms** (spread **0.346 ms**) | **−0.334 ms** (spread **0.650 ms**) |
| the part in no gap | **0.000 ms** | **0.000 ms** |
| the reconciliation | **0.000 ms** | **0.000 ms** |
| the reduction's own noise | **−0.036 ms** (spread **0.385 ms**) | **+0.057 ms** (spread **2.055 ms**) |

**And the controls, which are reported beside every set and are never summed**: the **floor**
(**6.282 ms**, spread **0.936 ms**, in the new set; **6.192 ms**, spread **0.242 ms**, in the old one),
the **scan's stage**, and the **build side's file state** (**+0.205 ms**, spread **0.323 ms**, in the
new set). **The added clock's cost is 0.000026 ms.**

**The verdict is `unattributed`, on both paths.** The yardstick is the context's own spread — the
band — in the set that measures it: **2.211 ms** on the cold path and **0.418 ms** on the resident one.

**On the cold path the context itself is inside its own band** — **+0.433 ms** — so the context is at the
noise. **On the resident path the context is outside its band** — **−1.565 ms** — so the context is a
real thing there, but the parts cannot be told apart: the build side's gap (spread **0.435 ms**), the
probe's file state (spread **1.398 ms**) and the probe's own remainder (spread **0.650 ms**) each have
a spread wider than the band, so each is **inconclusive**, and the open's gap (spread **0.007 ms**) is
**at the noise**. **No mechanism is claimed.** That is the honest answer: the decomposition closes
exactly, and on this host the parts cannot be told apart from the machine's own variation.

**The A/B: the old code's set.** Its context's residual is **13.727 ms** (spread **2.873 ms**) cold and
**7.644 ms** (spread **0.285 ms**) resident, and its context is **+0.149 ms** (spread **0.801 ms**) cold
and **−1.614 ms** (spread **0.846 ms**) resident. **The old set's ladder is absent** — that code does
not report the join stage's parts, so there is no ladder to judge and no verdict to publish.

**And the query's cost did not change.** The added work is **0.000026 ms**; the stages moved
**−0.505 ms** (cold) and **+0.322 ms** (resident) between the two codes, and the floor moved
**+0.090 ms**; the context moved **+0.284 ms** (cold) and **+0.049 ms** (resident), both inside the
band's own width. **The change is inside the threshold** — the floor's own spread across the campaigns
of the new set, **0.936 ms**.

**And the answer did not change.** The join's rows, their order and their count are the same; the
aggregate's value is the same; the five named tests hold — `a_build_key_more_than_255_rows_carry_still_counts_every_one`,
`a_probe_key_below_the_table_s_base_matches_nothing` and
`a_null_key_never_matches_on_either_side` in `crates/columnar-engine/tests/join.rs`, and
`the_direct_table_agrees_with_the_hash_table` and
`the_overflow_map_is_allocated_only_when_a_key_overflows` in
`crates/columnar-engine/src/operators/join.rs`. **The lookup's shape is untouched**: the change does
not touch `operators/join.rs` at all — the `#[inline]`, the overflow map's semantics, both hashers and
the hashed path all stand.

**And the sets are preserved under their own names.** The sets spec 029 published become
`join-cost-spec029-run*.json` and `join-cost-spec029-before-run*.json`, this increment's sets take the
current names, and the older sets are kept.

**And the next question is written down where this one was** — the README's Next row and
[`STATUS.md`](../../STATUS.md)'s last section — and it is written down **either way**. The question this
leaves is: the context is real on the resident path, and the parts that could account for it cannot be
told apart from the host's variation, because each of their own spreads is wider than the context's own
spread — so the question is **what would make the parts separable**. The decomposition is built and
closes exactly; what is missing is a resolution at which the parts are not dominated by the machine's.

## References

- [ADR-0035](0035-the-join-stage-taken-apart.md) — the join's stage, taken apart, and the residual this
  spec's context is measured against.
- [ADR-0036](0036-the-probes-remainder-taken.md) — the probe's remainder, taken: the map was never it.
- [ADR-0037](0037-the-lookups-shape-and-the-stage-that-moved.md) — the lookup's shape, the two artefacts
  of the instrument, and the context this spec takes up.
- [spec 026](../../specs/026-the-join-stage-taken-apart/spec.md) — the join's stage taken apart on both
  paths, and the derivation the stage's residual comes from.
- [spec 029](../../specs/029-the-lookup-shape-and-the-stage-that-moved/spec.md) — the lookup's shape, the
  crate boundary, and the figures this spec takes up.
- [`PERF.md`](../../PERF.md) §3o — the tables these figures are published in.
