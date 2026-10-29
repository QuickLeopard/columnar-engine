# Spec 033 — The drift, taken apart

**Status**: in progress
**Started**: 2026-10-29
**Amended**: 2026-10-29 — **spec 030's R4**, the yardstick, is superseded by this spec's R2 and R3:
the band it named is a **difference of two drifting series**, and on the resident path it came out
**narrower than the machine's own movement**. The reason is recorded in R2 and R3, and in
[spec 030's own header](../030-the-context-taken-apart/spec.md).
**Amended**: 2026-10-29 — the plan's **functional decomposition** and its **dependency graph**:
the rename's place in the order, the two thresholds that named a yardstick that did not exist yet,
the quantity R5 asked for and R4 abolishes, the derivation's two passes, the guard's place in the
publication, the edge the guard's own state derivation was missing from, and the families the
spec's own tables left out — with the reasons recorded in **The plan's shape**.
**Predecessor**: [030-the-context-taken-apart](../030-the-context-taken-apart/spec.md)

## The problem

Spec 030 took the **context** apart, and the decomposition closes exactly: the join stage's plan
output now reports its **parts**, the parts tile the context in every iteration, and the
**reconciliation is 0.000 ms** on both paths. The context is **0.433 ms** on the cold path against
a spread of **2.211 ms** — at the noise there — and **−1.565 ms** on the resident path against a
spread of **0.418 ms** — **outside** its band, so on that path the context is a real thing. And the
verdict is **`unattributed`** on both paths: **no leaf counted**. The verdict is taken over the
ladder's **five leaves** — the build side's gap, the open's gap, the probe's file state (or, on the
cold path, its row-number offset), the probe's own remainder and the part in no gap; **the probe's
gap is their parent, and it is not a leaf of its own.** On the **cold** path one leaf is
**inconclusive** — the row-number offset's own spread, **2.516 ms**, is wider than the band — and the
other four are **at the noise**. On the **resident** path three are inconclusive — the build side's
gap (**0.435 ms**), the probe's file state (**1.398 ms**) and the probe's own remainder
(**0.650 ms**) — and the other two are at the noise; **and the file state's value, −1.178 ms, is
outside the band, so it is ruled out by its own spread, not by its size.**

**And on the resident path the band is not a measurement of the machine.** Here is the arithmetic,
out of the committed reports — the code that ships, the resident path, the three campaigns of one
session:

| | campaign 1 | campaign 2 | campaign 3 | **the spread** |
|---|---|---|---|---|
| the stage's residual | 7.207 ms | 7.386 ms | 7.819 ms | 0.612 ms |
| the standalone probe | 8.482 ms | 9.080 ms | 9.384 ms | 0.901 ms |
| **the context** — the difference of the two | **−1.275 ms** | **−1.694 ms** | **−1.565 ms** | **0.418 ms** |
| **the floor** — the machine's own measure of a memory-bound loop | 6.876 ms | 5.941 ms | 6.282 ms | **0.936 ms** |

**The band is 0.418 ms and the machine's own movement over the same three campaigns is 0.936 ms.**
The band is **narrower than half the machine's variation**, and a yardstick that is narrower than
the noise it is meant to measure cannot measure anything.

**And the reason is in the two rows the context is the difference of: they co-move.** The residual
rises by **0.612 ms** across the campaigns and the standalone probe rises by **0.901 ms**; the
difference moves by **0.418 ms** and comes back — it ends **0.289 ms** from where it started.
**A band that is the difference of two drifting series can be narrow — or zero — while the machine
is drifting a millisecond.** It is not a yardstick, and it was never going to be one.

**And this is not the code's shape, it is the method's.** The same comparison, over **every set the
repository keeps that carries a context** — the two spec 028 sets predate the context and carry
none:

| the set | the path | **the band** (the context's own spread) | **the floor's drift** | the standalone probe's drift |
|---|---|---|---|---|
| **the code that ships** (spec 030) | cold | 2.211 ms | 0.936 ms | 1.159 ms |
| **the code that ships** (spec 030) | **resident** | **0.418 ms** | **0.936 ms** | 0.901 ms |
| the code it replaces (spec 030's before) | cold | 0.801 ms | 0.242 ms | 2.071 ms |
| the code it replaces (spec 030's before) | resident | 0.846 ms | 0.242 ms | 0.644 ms |
| spec 029's set | **cold** | **0.050 ms** | **0.623 ms** | 2.070 ms |
| spec 029's set | **resident** | **0.282 ms** | **0.623 ms** | 0.227 ms |
| the code spec 029 replaced | cold | 0.793 ms | 0.722 ms | 3.193 ms |
| the code spec 029 replaced | resident | 0.964 ms | 0.722 ms | 1.455 ms |

**And in three of the eight cells the band is narrower than the machine's own movement.** The two
that matter most: on the **resident path of the code that ships** — the path where the context was
**outside** its band — the band is **0.418 ms** against the machine's **0.936 ms**; and in **spec
029's set**, on **both** paths, the band is **0.050 ms** and **0.282 ms** against a floor that moved
**0.623 ms**.

**And the cold path of the code that ships is the counter-example.** Its band is **2.211 ms** against
the machine's **0.936 ms** — **wider**, and therefore sound. Its `unattributed` stands, and the
resident path's does not.

**And the published verdict says less than it appears to say.** `unattributed` says "no part
accounted for more of the context than the band". It does **not** say "the context has no
mechanism" — and today the reader cannot tell the two apart, because the band was never checked
against the machine it is supposed to measure. **A yardstick is a measurement, and this one was
never measured.**

**And the part that could have carried it was ruled out by that yardstick.** On the resident path
the one part worth carrying the context is the **probe's file state** at **−1.178 ms** — the query
probes a column the dataset has held, and the standalone reads its column afresh. It was published
as **inconclusive**, because its own spread (**1.398 ms**) is wider than a band of **0.418 ms** that
is itself narrower than the machine's **0.936 ms**. **The part was not too small; the band was too
narrow.**

### How the published figures are reduced

**Every figure this spec quotes is reduced one way, and the way is part of the method.** The
instrument derives each figure **per campaign**; the published figure is the **median of those
per-campaign values**. The **spread** is the **range** of those per-campaign values across the
three campaigns of that set. A difference **between two sets** is the **difference of their
medians**.

**And the band is reduced like every other figure, which is exactly the problem.** The band is the
**spread of the context's per-campaign values**, and the context's per-campaign value is

> the stage's **own median** − the standalone build side's whole − the standalone build side's open
> − the standalone probe.

The stage's median and the standalone's medians are **three measurements taken in three different
blocks of time**, and the context is the difference of what they say. So the band is the difference
of two drifts, and the drift of the difference is smaller than either drift whenever they co-move
— which, on this host, they do.

**And the sum of the parts is reduced on a different footing from the context, and the difference is
published.** The context is the **median over the iterations** of a sum; the ladder's parts are the
**medians over the iterations** of each part, and their **sum** is the sum of three medians — and
**the sum of three medians is not the median of three sums**. The difference is the **reduction's
own noise**, and it is published: its **figure** is **0.057 ms** on the resident path and its
**spread** is **2.055 ms** — **nearly five times** the band of **0.418 ms**. It is the reason the published
"the three gaps together" and the published context do not add up. **The house already publishes
this; what the house does not do is check the band against it.**

### What the code says

**The first phase reads the instrument's own code and confirms the claims below; where the code
says otherwise, they are corrected.** These are what the code says as it ships.

**The two sides of the context are measured in different blocks of time, and the separation is not
recorded.** In `join_cost_probe.rs`'s `measure()`, the order is:

1. the build side's **file state** — the process's first read of the build side's data;
2. **`engine_stages`** — the query, five iterations of the cold path and the resident path;
3. **`build_parts`** — the standalone build side's open, read, keys and whole;
4. **`build_table`** — the table the probe will use;
5. **`probe_parts`** — the standalone probe, over the column, in the batches, over the held column,
   the floor and the candidates;
6. the **controls** — the plain read, the small read and the clock's own cost.

**So the in-query side is measured first and the outside side is measured after it**, with the
whole of the other measurements in between, and the context is the difference of the two blocks.

**And the outside side's per-iteration samples are thrown away.** `Timing` keeps the **median**,
the **fastest** and the **slowest** sample, and nothing else; `build_whole` and `probe_over_column`
return a `Timing`, not a series. So the standalone's five samples do not survive into the report,
the drift between the two blocks cannot be measured, and **there is no pairing at all**: every
iteration is compared against the standalone's **median**, one number for the whole campaign, taken
with **the whole of the measurements in between** in time. **How far apart, is not recorded
anywhere.**

**And the report carries medians, not series.** Every **measured** part is published as its
**median** over the five iterations, with the **range across those five iterations** beside it; the
**derived** figures — the gaps, the context, the leaves and the reduction's noise — carry the
**median alone**, and their range **across the three campaigns** is derived by `scripts/medians.py`
and checked by `scripts/check-published-figures.py` from the three reports. So the leaves'
**correlation** — whether they move against each other, which is what decides whether their sum is
tight — **cannot be checked by a reader from the committed report at all.**

## The requirements

### R1 — The machine's drift is measured on both sides, and it is published

**The yardstick is a measurement of the machine's variation, and the measurement is published
beside every set.**

The **drift** of a measurement is the **range across the campaigns** of its per-campaign values —
the same reduction every other spread in this repository uses. The measurements that carry the
machine's drift are the **controls** and the **engine's own parts**:

| the measurement | whose drift it is | where it is measured |
|---|---|---|
| the **floor** | the machine's own measure of what a memory-bound loop costs on this host | the standalone block, in `probe_parts` |
| the **plain read** | a read a probe row cannot go under | the **controls** block, after `probe_parts` |
| the **standalone probe** | the same work, measured **outside** the query | the standalone block, in `probe_parts` |
| the **build side's whole** | the build side, measured **outside** the query | the standalone block, in `build_parts` |
| the **engine's own probe** | the same work, measured **inside** the query | the query's iteration, in `engine_stages` |
| the **engine's own build** and **open** | the engine's own account of the stage | the query's iteration, in `engine_stages` |

**And R4 brings the query's iteration and the standalone's measurements together**, so the column
above says where each measurement is taken **today**; after R4 they are taken **in the same
iteration**, and the drifts are published from the set that R4 measures.

**And the drifts are computed from the committed reports and published, before anything is
measured.** Here they are, for the set the campaign will be measured against — the code that ships
— with each measurement's own three campaigns beside it:

| the measurement | campaign 1 | campaign 2 | campaign 3 | **the drift** |
|---|---|---|---|---|
| the **floor** | 6.876 ms | 5.941 ms | 6.282 ms | **0.936 ms** |
| the **plain read** | 0.574 ms | 0.521 ms | 0.555 ms | 0.053 ms |
| the **small read** | 0.041 ms | 0.047 ms | 0.041 ms | 0.006 ms |
| the **standalone probe** — cold | 13.622 ms | 12.498 ms | 12.463 ms | 1.159 ms |
| the **standalone probe** — resident | 8.482 ms | 9.080 ms | 9.384 ms | 0.901 ms |
| the **build side's whole** | 13.172 ms | 12.605 ms | 12.519 ms | 0.654 ms |
| the **build side's open** | 0.033 ms | 0.026 ms | 0.030 ms | 0.007 ms |
| the **engine's own build** — cold | 12.440 ms | 12.155 ms | 12.446 ms | 0.291 ms |
| the **engine's own build** — resident | 12.829 ms | 12.262 ms | 11.742 ms | 1.088 ms |
| the **engine's own open** — cold | 0.133 ms | 0.120 ms | 0.125 ms | 0.013 ms |
| the **engine's own open** — resident | 0.121 ms | 0.108 ms | 0.120 ms | 0.013 ms |
| the **engine's own probe** — cold | 15.943 ms | 12.649 ms | 12.771 ms | 3.295 ms |
| the **engine's own probe** — resident | 8.349 ms | 7.704 ms | 7.338 ms | 1.011 ms |
| the **probe's held column** | 8.683 ms | 7.902 ms | 8.186 ms | 0.781 ms |
| the **probe's batches, no offset** | 15.759 ms | 12.119 ms | 12.582 ms | 3.640 ms |
| the **stage's own elapsed** — cold | 28.552 ms | 24.642 ms | 25.446 ms | 3.910 ms |
| the **stage's own elapsed** — resident | 20.412 ms | 20.017 ms | 20.368 ms | 0.395 ms |

**And the drift is the range across the three campaigns** — the same reduction every other spread in
this repository uses — **and it is the yardstick's only input.** The floor's **0.936 ms** is the
floor under every leaf's yardstick: the machine moved that much over three campaigns of the same
work on the same host. **And the machine moved most of all in the engine's own cold probe**
(**3.295 ms**) **and in the probe's batches without the offset** (**3.640 ms**) — the two
measurements the cold path's leaves stand on.

Each is published with its drift, beside the set it belongs to, and **none of them is ever summed**
— they are controls, and this spec says so where the floor's own section says it.

**And the sets that do not carry a measurement do not get a drift for it.** The engine's own parts
exist only in the sets whose engine **reports the parts** — the set this increment takes, and spec
030's set beside it; the sets that predate the plan output's parts carry **no** engine parts and so
carry no engine drift.

**And so the ladder, the yardsticks, the verdict and the state are published for those two sets
only.** The guard derives the ladder today from the set that measures the code that ships; **under
this spec it derives it for both**, so that spec 030's ladder is **checked against the yardstick**
as well and not only quoted. The other **five** families publish their parts and the floor's drift,
and no ladder — **a missing measurement is published as missing, not as a zero.**

**And the two sides are named, because a difference of two measurements carries the drift of both.**
Every part of the ladder is the difference of a measurement the query made and a measurement the
instrument made outside it — or of two measurements the instrument made outside it — so its noise is
carried by **the sides it stands on**, and the yardstick has to know which they are.

### R2 — The yardstick is the machine's drift, and the context's own spread is published as what it is

**The yardstick is the machine's own drift, not a difference of two drifts.**

> **The predecessor's requirement is rewritten, and the reason is recorded here and in the
> predecessor's own header.** Spec 030's R4 said "the yardstick is
> the context's own spread, in the set that measures it". That made the yardstick a difference of
> two drifting series, and on the resident path of the code that ships the difference came out at
> **0.418 ms** against the machine's own **0.936 ms** — a yardstick narrower than the noise. The
> requirement now says the yardstick is the machine's drift, measured on the controls, and it says
> the context's own spread is published **as what it is** — a difference of two drifts — so the
> reader can see both.

**And the rule is fixed before the campaign is taken, and it is fixed without reference to any
verdict.** The yardstick for a leaf is the **larger** of the drifts of the measurements that leaf
stands on — the one the query made and the one the instrument made outside it — and of the
**floor's** drift, the machine's own measure of a memory-bound loop, which every part of the ladder
resembles. Every drift below is computed from the **committed** reports, and every leaf's two
sides are checked against the reports as well:

| the leaf | its sides, **resident** | the yardstick, **resident** | its sides, **cold** | the yardstick, **cold** |
|---|---|---|---|---|
| the build side's gap | the engine's own build ↔ the build side's whole | **1.088 ms** | the engine's own build ↔ the build side's whole | **0.936 ms** |
| the open's gap | the engine's own open ↔ the build side's open | **0.936 ms** | the engine's own open ↔ the build side's open | **0.936 ms** |
| the probe's file state | the held column ↔ the probe over the column | **0.936 ms** | — | — |
| the probe's row-number offset | — | — | the probe in the batches ↔ the same without the offset | **3.640 ms** |
| the probe's own remainder | the engine's own probe ↔ the held column | **1.011 ms** | the engine's own probe ↔ the probe without the offset | **3.640 ms** |
| the part in no gap | the stage's own elapsed, alone | **0.936 ms** | the stage's own elapsed, alone | **3.910 ms** |

**And the probe's gap is not in the table, because it is not a leaf.** It is the **parent** of the
file state and the remainder — on the cold path, of the row-number offset and the remainder — and
judging it beside its own leaves would break the tiling the reconciliation rests on.

**The larger is the conservative choice, and it is the one that is taken**: a narrower yardstick
would count leaves that the machine could have produced on its own, and the house's rule is that a
gate must be able to say "I could not tell".

**And the yardstick is per leaf, because the leaves stand on different sides.** One number per path
would be wrong: the build side's gap stands on the engine's own build and the build side's whole,
and the file state stands on two measurements the instrument made outside the query. **So each leaf
has its own yardstick, and each leaf's yardstick is published with its state.** The guard
**computes** the yardstick by this rule from the drifts in the report, and checks that the
published figure and the computed one agree.

**And the rule is published with the number it gives, from the committed reports, before the
campaign.** On the resident path of the code that ships, the yardstick for the **probe's file state**
is **0.936 ms** — the floor's drift, which is **larger than either of its two sides'** (**0.781 ms**
and **0.901 ms**) — against the leaf's own spread of
**1.398 ms**, so the leaf is **inconclusive**; and its value, **−1.178 ms**, is **outside** the
yardstick, so if the leaf's own spread were narrower than the machine's drift, it would be a part of
the context. **On the same reports and the same rule**, the **build side's gap** (−0.343 ms, spread
**0.435 ms**), the **open's gap** (0.088 ms, spread **0.007 ms**), the **probe's own remainder**
(−0.334 ms, spread **0.650 ms**) and the **part in no gap** (0.000 ms) are all **at the noise** — so
no leaf is counted and the verdict is **`unattributed`**, with a yardstick that could measure.
**Which of those happens in the new campaign is measured, and the rule is fixed before the
measurement.**

**And a leaf is judged exactly as spec 030's R4 judges it, with the yardstick changed and nothing
else changed.** A leaf whose own spread is **wider than its yardstick** is **inconclusive**, with
both spreads published, and is **not counted**. Otherwise a leaf whose own **value** is outside its
yardstick is **a part of the context** and is counted; otherwise it is **at the noise**. The verdict
is the **largest counted leaf's** word, and **`unattributed`** when no leaf is counted.

### R3 — The verdict has a state, and the state says whether the test could tell

**The list of six words is a list of mechanisms, and a state of the test is not a mechanism.**
Spec 030's R4 closed the list over the ladder's mechanisms, and published **`unattributed`** whenever
**no leaf counted** — whether the leaves were **at the noise** (the band was sound and they sat
inside it) or **inconclusive** (their own spread was wider than the band, so the band could not
tell). **Those are different findings, and they were published with the same word.** The resident
path's verdict is one of them: the **probe's file state** was ruled inconclusive by a band of
**0.418 ms**, and `unattributed` does not say whether the context has a mechanism or whether the
band was too narrow to look.

**And there is one vocabulary per level, and each level has its own published phrase.** The house's
leaf vocabulary stays as it is — **`a part of the context`**, **`at the noise`**,
**`inconclusive`** — and the verdict's own vocabulary is new, and the two are named apart:

| the level | the vocabulary | the published phrase |
|---|---|---|
| the **leaf** | `a part of the context` · `at the noise` · `inconclusive` | "the {leaf}, on the {path} path, is **{state}**" |
| the **verdict** | `sound` · `not sound` · `inconclusive` | "the verdict's state, on the {path} path, is **{state}**" |

**And the verdict's state has three values, and what they mean is exact.**

- **`sound`** — the yardstick could measure what it judged, and at least one leaf could be judged.
- **`not sound`** — the yardstick was narrower than the machine's own movement, so it could not
  measure what it judged.
- **`inconclusive`** — no leaf could be judged at all: every leaf's own spread was wider than its
  yardstick, so the machine could have produced every one of them.

**And the soundness check is a check on the yardstick, not on the parts.** Under R2's rule the
yardstick is **sound by construction** — the rule takes the floor's drift as its floor — so a set
measured under it is `sound`. The state exists to **annotate the verdict that was made with the
legacy band**, and to catch a yardstick that a later change made narrower. **The guard's real check
is the recompute**: the yardstick it derives from the drifts must be the one published.

**And there is one verdict per set and path.** The guard derives **one word and one state** per
path, from the set that measures the code that ships. The band-based verdict spec 030 published is
**kept beside it as the figure it was** — `unattributed`, on a band of **0.418 ms** — **annotated as
not sound**, and it is **not** published as a second verdict for the same path.

**And on the committed reports the two paths come out differently.** The resident path's band —
**0.418 ms** against the floor's **0.936 ms** — is **not sound**; the cold path's band —
**2.211 ms** against the same **0.936 ms** — **is sound**. **And under R2's rule, on the same
numbers, both paths are `unattributed` with a `sound` yardstick**, because R2's yardstick is sound
by construction and the leaves on the resident path that are judged are all at the noise. **The
band moved, and so the yardstick moved with it; both are published, with the reason.**

> **The predecessor's requirement is rewritten, and the reason is recorded here and in the
> predecessor's own header.** Spec 030's R4 published **`unattributed`** whenever no leaf counted,
> and gave the verdict no state. The house's own gate rule — "gates
> that can say 'I could not tell'; an inconclusive result must not be reported as a pass" — is the
> reason. A verdict of `unattributed` on a yardstick narrower than the machine's noise reports a
> failure to find a mechanism as though a mechanism had been looked for and not found.

### R4 — The two sides are measured in the same iteration, back to back, and the order is alternated

**The drift between the two blocks is inside the context today, and it is not measured.** The
standalone's per-iteration samples are discarded, the blocks are separated by the whole of the
other measurements, and **there is no pairing at all** — every iteration is compared against the
standalone's median.

**So the instrument measures the two sides in the same iteration, back to back.** Each iteration
takes the query's own measurement and the standalone measurements that stand on the other side of
the difference, in that iteration, and the ladder is built from them **per iteration** — as it is
today — **with one change: the standalone side enters as the iteration's own measurement instead of
the campaign's median.** The reconciliation stays **0.000 ms** and the ladder still closes, because
the reconciliation is the stage's own parts against the stage's elapsed and nothing else changes.

**And the ladder's figures change with it, and they are published old-and-new.** The context and
the gaps are computed against a per-iteration standalone value rather than a median, so the
**reduction's own noise** — the difference between the sum of the medians and the median of the sums
— is a different number. **It is published, under the same name, with the reason.**

**And the build side's file state is measured first, and stays measured first.** The build side's
**file state** — the process's first read of the build side's data — is what makes the two build
sides comparable: both read the build side's file **warm**. **Interleaving does not move it.** The
file state is still taken before any iteration, the query's build side and the standalone's still
read the same warm file, and the build side's gap still measures what it measures today. **If the
interleaving would change the file state's bracket, the route is refused, with the reason
published.**

**And the route was checked, and it is not refused.** The standalone's whole is taken after the
engine's own execution in the even iterations, and after the previous iteration's execution in the
odd ones, and it is **never** the file's first read: the file state is taken before any iteration
and it is the process's first read of the build side's data. **The bracket is unchanged**, and the
file state's own figure is published with the rest.

**And the ladder's reconciliation is published.** The parts still sum to the context in every
iteration, the reconciliation is still **0.000 ms**, and it is published so that the claim is
checked rather than asserted — **and what is left unexplained is named** and published with its
spread.

**And the order is alternated between the iterations, and the order's own effect is published.**
The order is not neutral: a machine that drifts while an iteration runs gives the first
measurement a different price from the second. So **even iterations take the query first and odd
iterations take it last**, the two orders' difference is published as its own figure — **the
order's own cost** — and it is compared with **the floor's own spread**, so that the reader can see
whether the order could have made any of the parts. **The floor's own spread is the one number
every cost is compared with**, because the yardstick is per leaf and a cost is not a leaf.

**And nothing is compared with the new set's yardstick before it exists.** The **yardsticks** that
fix the rule before the campaign are the **pre-registration** — computed from the committed
reports, and published before the campaign is taken. The **costs** — the order's own cost and the
interleaving's added work — are compared with **the floor's own spread**, which is the machine's
own number and is in hand before the campaign. **The new set's yardstick does not exist until after
the campaign, and nothing is compared with it before it does.**

**And the instrument's own cost is measured, not assumed.** The interleaving costs the campaign
time, and the per-iteration values the report now carries are written rather than timed — but the
interleaving changes what is measured, so **the added work is measured** the way spec 030 measured
the added clock: the cost of one interleaved iteration's extra work, published, and compared with
**the floor's own spread** — the same threshold R6 uses for the query's cost, because that is the
machine's own measure of what a change of this kind costs. **A route that costs more than the
floor's own spread is refused, with the reason published.**

### R5 — The report carries the per-iteration values, and the time each block took

**The ladder's reduction, the leaves' correlation and the drift between the blocks are checkable by
a reader from the committed report, or they are not published.**

The report carries every **measured** part's **median** and the range across its five iterations; the
**derived** figures carry their median alone. That is enough to publish a figure and not enough to
check it: the leaves' **correlation** — whether they move against each other, which is what decides
whether their sum is tight — cannot be seen, and the **reduction's own noise** can only be taken on
trust.

So the report gains, for every part **measured and derived**, the **per-iteration values** and the
**time each block took**, so that:

- the ladder's reduction — the median of a sum against the sum of medians — is checkable;
- the leaves' correlation is checkable;
- the **separation between the two sides of one difference** is measurable, and is published.
  **There is no drift between two blocks to publish after R4, because there are no blocks left** —
  the two sides are measured in the same iteration — and what the reader needs instead is **how far
  apart the machine's own two measurements of one difference were taken**, which is the distance the
  machine drifts across a single pair, and which the yardstick has to cover.

**And the raw reports stay committed**, under their own names, as every set in this repository has.

### R6 — The answer does not change, and the query's cost does not change

**The join's rows, their order, their count and the aggregate's value are the same** — the same
rows in the same order, with the same count a key, and a NULL key that still never matches. **The
engine is not touched by this spec**: the query path does not change, the plan output does not
change, and no published query figure moves. **The instrument changes, and the instrument's own
cost is measured** (R4).

**And the query's cost does not change by more than the floor's own spread**, which is the
threshold spec 030's R3 named and which this spec keeps. **The query's cost is measured against it,
and the difference is published** — on both paths, against the set this increment takes.

**And every part's rows are checked against the engine's rows, in order, before anything is
timed**, and the aggregate's value against the published one — the same rule spec 029's ladder
held and spec 030 kept.

**And the tests that already hold the behaviour keep passing**, and they are named here by their
identifiers, so that a stranger knows what "does not change" means:

| test | what it holds |
|---|---|
| `crates/columnar-engine/tests/join.rs` — `a_build_key_more_than_255_rows_carry_still_counts_every_one` | the overflow map's semantics: a key's count is not lost when its byte fills |
| `crates/columnar-engine/tests/join.rs` — `a_probe_key_below_the_table_s_base_matches_nothing` | the range rule |
| `crates/columnar-engine/tests/join.rs` — `a_null_key_never_matches_on_either_side` | the NULL rule, which the lookup does not touch |
| `crates/columnar-engine/src/operators/join.rs` — `the_direct_table_agrees_with_the_hash_table` | the direct table's answer against the hash table's, key for key |
| `crates/columnar-engine/src/operators/join.rs` — `the_overflow_map_is_allocated_only_when_a_key_overflows` | the map is not allocated when nothing overflows |

**And the unit test is read again after any change**, not only run.

### R7 — Every figure that moved is published old-and-new, and the guard follows the new sets

**A figure that moved is published with the figure it replaced and the reason it moved.** This
spec moves figures in the places that publish the context, the band and the verdict:

| where | the figure | where its old value lives |
|---|---|---|
| [`PERF.md`](../../PERF.md) §3o | the context, the band, the yardstick, the verdict and its state | §3o's own "the code this increment ships" column, and the spec 030 set beside it |
| [`PERF.md`](../../PERF.md) §3o's ladder | the leaves' values, spreads, yardsticks and states | the same table's earlier columns |
| [`PERF.md`](../../PERF.md) §3o's parts | the stage, the floor, the probe, the build side's whole and the reduction's own noise | the same table's earlier columns |
| [`README.md`](../../README.md) | the Next row's context and verdict | the row's previous values |
| [`STATUS.md`](../../STATUS.md) | the join probe's row — the stage, the floor, the probe, the build side's whole, the remainder and the context | the file's own probe table |
| [`STATUS.md`](../../STATUS.md) | the context's open question | the file's own open-questions section |

**And the sets are kept, and the guard's set list moves with them.** Spec 030's sets become
`join-cost-spec030-run*.json` and `join-cost-spec030-before-run*.json`, and this increment's set
takes the current names.

**And the guard's families are re-pointed, because a glob that matches nothing is a failure.**
`scripts/check-published-figures.py` keeps a **before** family — the code the increment replaces —
and this increment replaces **no code**, so the family is **re-pointed at the set the last increment
replaced**, and the last increment's own set is added beside it:

| the family | the glob, after this increment | the set it holds |
|---|---|---|
| the code this increment ships | `join-cost-run*.json` | this increment's set |
| the code the last increment shipped | `join-cost-spec030-run*.json` | spec 030's set |
| the code that one replaces | `join-cost-spec030-before-run*.json` | the code spec 030 replaced |
| the set before that | `join-cost-spec029-run*.json` | spec 029's set |
| and the code that one replaces | `join-cost-spec029-before-run*.json` | the code spec 029 replaced |
| the set before that | `join-cost-spec028-run*.json` | spec 028's set |
| and the code that one replaces | `join-cost-spec028-before-run*.json` | the code spec 028 replaced |

**And the family is kept, not dropped** — the guard checks every family's parts against `PERF.md`
§3o, and the set it names is published in that document's "the code it replaces" columns, so a
family that disappears would **uncheck a published figure**. **The glob changes; the family does
not.**

**And the guard is told exactly how, and what it must now derive.**

- `scripts/check-published-figures.py` gains the new report glob and the new entries in its set
  list, the **yardstick** figures, the **drifts** beside every set, and the **verdict's state**.
- **The guard's verdict is re-derived against the yardstick, not against the band.** The guard today
  takes the band as the context's own spread and judges every leaf against it; under this spec it
  **derives the yardstick** from the drifts in the three reports, by R2's rule, judges every leaf
  against that, and checks that the **published** yardstick is the one it derived.
- **The verdict's state is checked as a phrase of its own**, the way the six words are checked
  today: "the verdict's state, on the {path} path, is **{sound|not sound|inconclusive}**", and each
  leaf's state as "the {leaf}, on the {path} path, is **{state}**".
- **The yardstick is published inside §3o, under no new `###` heading.** The guard's section lookup
  ends at the next `## ` or `### `, so a new `###` inside §3o would end the section and stop every
  join figure from being checked. A `####` sub-heading is safe.
- **A missing set fails the guard**, as it already does for the join probe's sets — and a family
  whose glob matches **nothing** fails it too, which is why the family is **re-pointed** rather
  than left pointing at a glob that no longer matches.
- `scripts/medians.py` derives the drifts, the yardsticks and the verdict's state, and prints them.
- **The phrase checks move with the figures.** The guard builds **literal phrases** for the README's
  Next row and for `STATUS.md`'s probe row — "the context on the {path} path is **{value}**", "the
  {path} path's spread is **{spread}**" — and it checks them against the set that measures the code
  that ships. **Those phrases are re-derived from the new set and re-published**, and the guard
  gains the **yardstick** and the **state** phrases beside them. **A phrase that is left behind
  fails the guard**, which is the point.
- **The conclusion is restated, not only the figures.** The documents say today that "the context is
  a real thing" on the resident path, on the strength of a band of **0.418 ms**. R3 says that band
  was **not sound**. **So the sentence is restated on a yardstick that can measure** — the context's
  **value** against the **machine's own drift** — and the numbers beside it are the ones that decide
  it. The claim survives; **the reason it rested on does not**, and a claim and its reason are
  published together or the claim is not published at all.
- `bench-results/README.md` gains a row per new set, and its §3o row lists the new globs.

### R8 — The open question is answered in the open, whether the parts separate or not

**The next question is written down where this one was** — the README's Next row and
[`STATUS.md`](../../STATUS.md)'s last section — and it is written down **either way**. A leaf that
separates is an answer; a leaf that does not separate is also an answer, and it is published with
the numbers that show it, together with **the resolution that would be needed**.

## The plan's shape

**A plan is a set of functions and the order they can be done in.** This one is decomposed here,
function by function, and the decomposition is checked against the phases below — because a phase
list that disagrees with the dependencies is how work gets done in the wrong order.

| the function | what it touches | what it produces | **the phase** |
|---|---|---|---|
| **A1** the interleaved iteration | the instrument | the two sides of a difference, measured in one iteration | 3 |
| **A2** the alternated order | the instrument | the order's own cost | 3 |
| **A3** the file state's bracket | the instrument | the build side's gap, measured what it measures today | 3 |
| **A4** the report's series | the instrument | every part's per-iteration values, and each block's time | 3 |
| **B1** the drift | `scripts/medians.py` | the range across a set's three campaigns, for every measurement | 1, 2 and 7 |
| **B2** the yardstick | `scripts/medians.py` | each leaf's yardstick, by R2's rule | 2 and 7 |
| **B3** the states | `scripts/medians.py` | each leaf's state, and the verdict's state | 7 |
| **C1** the recompute | `scripts/check-published-figures.py` | the yardstick the guard derived, checked against the published one | 7 |
| **C2** the verdict | `scripts/check-published-figures.py` | one word and one state per path | 7 |
| **C3** the phrases | `scripts/check-published-figures.py` | the published sentences, checked as literals | 7 |
| **C4** the set list | `scripts/check-published-figures.py` | the families and their globs | 4 and 7 |
| **C5** the ladder, twice | `scripts/check-published-figures.py` | the ladder of both sets whose engine reports the parts | 7 |
| **D1** the renames | the reports on disk | spec 030's sets, under their own names | 4 |
| **D2** §3o | `PERF.md` | the ladder, the yardsticks, the states, the parts | 7 |
| **D3** the Next row | `README.md` | the context, the spread, the verdict, the state, the conclusion | 7 |
| **D4** the status | `STATUS.md` | the probe row, the open question, the counts | 7 |
| **D5** the conclusion | all three | the claim, restated on a yardstick that can measure | 7 |
| **D6** the reports' index | `bench-results/README.md` | a row per set, and the globs | 7 |
| **E1** the supersession | spec 030's header | the reason, recorded in the spec that is amended | **done** |
| **E2** the ADR | `docs/adr/` | the decision | 7 |
| **E3** the next question | `README.md`, `STATUS.md` | the question this one leaves | 7 |
| **M1** the campaign | the instrument | three campaigns, five iterations each | 5 |
| **M2** the answer | the engine, the tests | the rows, the tests, the query's cost | 6 |

**And every function carries the phase it is done in.** The column is the phases' own numbering, so
the check is not a claim about the plan — it is the plan, read against the phases. A function with
no phase would be work with no place, and a phase with no function would be work with no
description.

**And a function's phase is the phase its work is *finished* in**, so that an edge never runs
backwards. **B1** and **B2** run twice on purpose — once over the **committed** reports, to fix the
rule before the campaign, and once over the **new** set, to publish it — and **C4** is re-pointed at
the renamed files in phase 4 and gains the guard's new checks in phase 7. **A function's work is
finished in its last phase, and every edge names a function that is finished no later than the one
it feeds.**

**And the dependencies that constrain the order** — the load-bearing edges, and why each one is
there:

| from | to | why |
|---|---|---|
| **A3** | **A1** | the interleaving must not move the file state |
| **A1**, **A2**, **A4** | **M1** | the campaign measures the interleaved instrument |
| **M1** | **B1** | a drift is a range over a set's three campaigns |
| **B1** | **B2** | the yardstick is the larger of drifts |
| **B2** | **B3** | the states follow from the yardsticks |
| **B1**, **B2** | **C1**, **C2** | the guard derives them the same way, or the check is not a check |
| **B3** | **C2** | the guard derives the verdict's state too, by the same rule |
| **C1** | **C2** | the verdict is judged against the recomputed yardstick |
| **C4** | **C1**, **C2**, **C5** | the guard reads the sets by glob |
| **D1** | **C4** | the glob must match the files on disk |
| **D1** | **M1** | **the new set takes spec 030's file names, so the rename comes first** |
| **M1** | **D2**, **D5** | the figures come from the campaign |
| **B2**, **B3** | **D2** | the yardsticks and the states are published |
| **B1** | **D5** | the conclusion rests on the machine's drift |
| **C3** | **D2**, **D3**, **D4** | the guard's phrases **are** the documents' sentences |
| **D2** | **D3**, **D4** | `PERF.md`, then the README, then `STATUS.md` |
| **D1** | **D2**, **D6** | the tables name the sets |
| **M1** | **E3** | the next question depends on what the campaign found |
| **D2** | **E2** | the ADR records the decision |
| **E2** | **D4** | the ADR's arrival moves `STATUS.md`'s ADR count |
| **M2**, **C1**…**C5**, **D2**…**D6** | the gate | the gate checks the published figures |

**And the graph is acyclic.** No edge runs backwards, so the work can be done in this order and
every step has what it needs — and the places where the order is not obvious are the places below,
where the plan was wrong.

> **The plan is amended, and the reason is recorded.** The decomposition and the dependency graph
> above are the review's findings, bottom-up and top-down. The requirements, the phases and the
> tasks are corrected to agree with them, and the corrections are these.

**1. The sets' rename was not in the plan, and the campaign depends on it.** The new set takes
`join-cost-run{1,2,3}.json` — and those are **spec 030's set's names**. A campaign taken before the
rename **overwrites spec 030's committed reports**, and the "before" that R7 publishes old-and-new
against is gone. **The rename is a phase of its own, before the campaign**, and it moves with the
guard's set list **and with the campaign** in the same commit — because a glob that matches nothing
fails the guard, and an intermediate commit would leave `join-cost-run*.json` **empty**.

**2. Two comparisons named a yardstick that does not exist yet.** The order's own cost and the
interleaving's added work are measured **before** the campaign, and the **new** set's yardstick does
not exist until after it. **The comparisons are now against the floor's own spread** — one number,
the machine's own measure — because the yardstick is per leaf and a cost is not a leaf. **And the
yardsticks that fix the rule before the campaign are the pre-registration**: the ones computed from
the committed reports, published before the campaign is taken.

**3. The phases contradicted R4.** Phase 3 compared the added work's cost with "the yardstick";
R4 compared it with the floor's own spread. **The phases were wrong and are corrected to the
requirement**, and the order's own cost now uses the same threshold as the added work's cost.

**4. R5 asked for a quantity that R4 abolishes.** "The drift between the two blocks" cannot be
published after the interleaving, because there are no blocks left to drift apart — the two sides
are in the same iteration. **What is published instead is the separation between the two sides of
one difference**, and the block times that show it.

**5. The derivation runs twice, and the phases described one of the passes.** The drifts, the
yardsticks and the states are derived from the **committed** reports to fix the rule before the
campaign, and again from the **new** set to publish it. **The phases say so now.**

**6. The guard's changes were not a phase, and the publication cannot pass without them.** The
guard's phrases are the documents' sentences, so the guard's changes and the publication are **one
phase**, and the guard's changes come **first** — the documents are written to the phrases the guard
checks.

**7. The ADR's arrival moves a checked figure.** `STATUS.md` publishes the number of ADRs and the
guard checks it, so the ADR lands in the same change as the publication that names it.

**8. The graph was missing an edge the guard actually has.** The guard does not only re-derive the
drifts and the yardsticks — it re-derives the **leaf's states** and the **verdict's state** too, by
the same rule `scripts/medians.py` uses, or the check is not a check. **B3 → C2** was not in the
graph, and the phases named the states' phrases without saying where the state came from. **The
edge is now in the graph, and the phases say that the guard derives the states the same way.**

**9. The spec's own tables named fewer families than the guard reads.** The guard reads **six**
join-cost families today — the code that ships, the code it replaces, spec 029's set and its
before, spec 028's set and its before — and this spec's table of the band against the floor's drift
showed **three** of them: **spec 029's before was missing**, and spec 028's two sets were missing
without saying why. **Spec 029's before carries a context** — a band of **0.793 ms** cold and
**0.964 ms** resident against a floor that moved **0.722 ms** — so it is in the table now, and the
count is **three of the eight** cells, not three of six. **Spec 028's two sets predate the context
and carry none**, and the table says so rather than leaving them out silently. **And R1's "the
other four families" was "the other five"**, and R7's family table listed four families where the
guard reads seven.

## The scope

**What this spec does not do, and why.**

- **The engine is not touched.** No plan, no operator, no plan output's shape, no query path. The
  instrument changes; the engine does not. **The public API is not changed**, so spec 030's
  carve-out does not apply here.
- **The lookup's shape is not changed.** Spec 029 measured it part by part: the remainder is
  **0.109 ms** with a spread of **1.083 ms**, and the shape is free.
- **The `#[inline]` is not undone.** It is what makes the instrument's measurement the query's cost,
  and this spec's whole method rests on that.
- **The build side's read is not changed.** It is Arrow's decode of `orders.parquet`, and it has
  been measured three times and refused every time.
- **The floor is not changed** and does not become a part: it is a control, it is reported beside
  every set, and it is **never summed**. **And it is not made quieter to make the yardstick
  tighter** — the yardstick is the machine's drift, and a quieter floor would make it *narrower*,
  which is the wrong direction.
- **The comparison campaigns are not taken again.** This increment touches no query's cost, so no
  published query figure moves, and the comparison set in §2 stays the one spec 028 published.
- **The old code is not re-measured.** The engine does not change, so there is no A/B: the code
  that ships is the code spec 030 shipped. Spec 030's set is the **before**, preserved under its
  own name, and every figure that moved is published old-and-new with the reason. **A comparison
  between two instruments is not a comparison of two codes**, and this spec does not make one.
- **The context's sign is not explained.** It changes sign between the code sets, and spec 030
  named the crate boundary as the part of it that is explained. What is left is not this spec's.
- **The plan output's parts are not changed.** They were added by spec 030 and they stay.
- **No ARM64 figure is compared with an x86-64 one.** Every figure here is ARM64, on this host,
  with the load average the reports carry.
- **The x86-64 absolute figures, the morsel driver and the `filter-range` gap are not this
  spec's.** They are named in [`STATUS.md`](../../STATUS.md) and stay there.

## The instrument

`crates/columnar-engine/examples/join_cost_probe.rs`, extended by spec 029 and spec 030, gains the
two things this spec measures:

- **the two sides, in the same iteration** (R4), with the order alternated between iterations;
- **the per-iteration values, and the time each block took** (R5), in the report.

**And the report gains the drift's figures**: nothing — **a report cannot carry a drift.** What one
campaign knows is its own values; the drift is a range over three campaigns, and it is **derived**
(R1, and the instrument's table below). What the report gains is the **per-iteration values** and
the **block times** (R5), so that the derivation can be checked by a reader.

**And the drifts, the yardsticks and the states are not in the report, because they cannot be.**
**One campaign is one process and one file.** A **drift** is a range across the three campaigns, and
a report knows only its own campaign — so the report carries what one campaign knows, and the
**scripts derive** the rest:

| the figure | where it is derived |
|---|---|
| every part's per-campaign value | the **report**, as it is today |
| every part's per-iteration values and each block's time | the **report** — new (R5) |
| the **drifts** — the ranges across the campaigns | `scripts/medians.py`, from the three reports |
| the **yardsticks** — each leaf's, by R2's rule | `scripts/medians.py`, from the drifts |
| the **leaf's state** and the **verdict's state** | `scripts/medians.py`, from the yardsticks and the leaves |

**And the published table MUST be derivable from the committed reports by a checked-in script**, so
the prose and the evidence cannot drift apart — which is why the yardstick is derived and not
typed in.

**And the instrument does not become the engine's**, and the engine's figure is never compared with
the instrument's without the two brackets being published side by side.

**And the engine's rows are checked against every part's, in order, before anything is timed**, and
the aggregate's value is checked against the published one — the same rule spec 029's ladder held.

## The measurement

**The plan, so that a stranger can run it:**

| what | how | where it lands |
|---|---|---|
| the commits | the code this spec writes — the instrument only — and the code spec 030 shipped, which is the same code | the spec's own history |
| the join probe's campaigns | `just probe-join "data/lineitem.parquet" "bench-results/join-cost-run1.json"`, three campaigns, five iterations each | `join-cost-run*.json`; spec 030's sets are preserved as `join-cost-spec030-run*.json` and `join-cost-spec030-before-run*.json` |
| the interleaving | the two sides, in the same iteration, the order alternated between iterations | the ladder's parts, and the order's own cost |
| the host | an Apple M4 Max, arm64, one thread, and the **load average**, which the join probe's reports carry | in every report |
| the guard | `scripts/medians.py` derives the drifts, the yardsticks and the verdict's state; `scripts/check-published-figures.py` checks every published figure against the reports, in the section that publishes it, and **fails when a set is missing or short** | the CI job "published figures match the evidence" |

**And the phases**, so that the order of the work is part of the method too:

1. **The brackets and the reduction.** Read the instrument's own code — the order of the blocks,
   what `Timing` keeps and what it throws away, how the context and the parts are each reduced —
   and write down, for each, what it brackets and what it does not include. The tables above are
   the **hypothesis**; this phase confirms them against the code as it ships, and corrects them if
   the code says otherwise. **The drifts of every control and of the engine's own parts are computed
   from the committed reports and published, before anything is measured.**
2. **The yardstick.** The **rule** is fixed before the new campaign is taken — the larger of the two
   sides' drifts, and the floor's — and the drifts and the yardsticks are computed from the
   **committed** reports first. **The rule is not chosen by looking at any verdict.**
3. **The interleaving.** The instrument measures the two sides in the same iteration, the order is
   alternated, the order's own cost is measured, and the added work's cost is compared with
   **the floor's own spread** — the threshold R6 uses, and the one number every cost is compared
   with. **A route that costs more than the floor's own spread is refused, with the reason
   published.**
4. **The sets' rename.** Spec 030's sets become `join-cost-spec030-run*.json` and
   `join-cost-spec030-before-run*.json`, the guard's set list is re-pointed at them, and **only
   then** is the campaign taken. **The new set takes the names spec 030's set has today, so the
   rename comes first — a campaign taken before it would overwrite spec 030's committed reports.**
   **The rename and the campaign land in the same commit**, because a glob that matches nothing
   fails the guard, and an intermediate commit would leave `join-cost-run*.json` empty.
5. **The campaign.** Three campaigns, five iterations each, with the floor in every campaign, the
   reconciliation published, and the per-iteration values in the report.
6. **The answer does not change.** The named tests are run, the unit test is **read again**, and
   the query's cost is measured against the threshold.
7. **The publication.** **The guard's changes come first** — the yardstick's recompute, the verdict's
   re-derivation, **the states' derivation and their phrases**, and the set list — **and then the
   documents**, written to the phrases the guard checks. Every figure that moved, old-and-new, with
   the reason; the verdict in one of the words and in one of the states; the conclusion restated on a
   yardstick that can measure; the resolution published either way; the next question written down
   where this one was.
   **The derivation runs again here, on the new set**, and the ADR lands with the publication that
   names it.

## The references

| | |
|---|---|
| [constitution.md](../../constitution.md) | **IV. Evidence Over Claims** — R5 and R7: the report carries what a reader needs to check the ladder, and a figure that moved is published with the figure it replaced |
| [constitution.md](../../constitution.md) | **"A statistical gate MUST also carry a practical threshold, set from a measurement of the machine's own run-to-run drift"** — R1 and R2: the yardstick *is* that measurement, and the band was not |
| [constitution.md](../../constitution.md) | **"Gates that can say 'I could not tell'"** — R3: the verdict carries a state, and an inconclusive result is not reported as a pass |
| [constitution.md](../../constitution.md) | **"Repeated runs, published medians"** — R1 and R4: the drift and the yardstick are ranges of per-campaign values, and the raw reports are committed |
| [constitution.md](../../constitution.md) | **I. One Predicate, Six Implementations, One Answer** — R6: the answer does not change, and the tests that hold it are named |
| [constitution.md](../../constitution.md) | **V. The Comparison Is Reproducible by a Stranger** — R5 and R7: the published table is derivable from the committed reports by a checked-in script, and the guard checks it |
| [constitution.md](../../constitution.md) | **"Definition of done": the benchmark re-run and the baseline updated deliberately, with the change explained** — R7: every figure that moved is published old-and-new, with the reason |
| [constitution.md](../../constitution.md) | **VII. Explicit Scope, No Silent Deferral** — the scope's exclusions, each with its reason, and R8: the open question is answered in the open either way |
| [constitution.md](../../constitution.md) | **"Complexity requires justification"** — the interleaving, the alternation and the report's new fields |
| [constitution.md](../../constitution.md) | **"Compliance is checked per spec"** — this section's citation of the principle each non-obvious requirement enforces |
| [ADR-0038](../../docs/adr/0038-the-context-taken-apart.md) | the context taken apart, the ladder, and the band that is a difference of two drifts |
| [ADR-0037](../../docs/adr/0037-the-lookups-shape-and-the-stage-that-moved.md) | the lookup's shape, the two artefacts of the instrument, and the context spec 030 took up |
| [ADR-0035](../../docs/adr/0035-the-join-stage-taken-apart.md) | the join's stage, taken apart, and the residual the context is measured against |
| [spec 026](../026-the-join-stage-taken-apart/spec.md) | the join's stage taken apart on both paths, and the derivation the stage's residual comes from |
| [spec 028](../028-the-probes-remainder/spec.md) | the probe's remainder, and the context it named as the control for the process |
| [spec 029](../029-the-lookup-shape-and-the-stage-that-moved/spec.md) | the lookup's shape, the crate boundary, and the set whose cold band is 0.050 ms |
| [spec 030](../030-the-context-taken-apart/spec.md) | the context taken apart, the ladder's parts, and the yardstick this spec replaces |
| [`PERF.md`](../../PERF.md) §3o | the published tables this spec's figures are re-derived into |
| [`STATUS.md`](../../STATUS.md) | where the project's open questions live |
