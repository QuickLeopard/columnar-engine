# Spec 030 — The context, taken apart

**Status**: done
**Started**: 2026-10-29
**Finished**: 2026-10-29
**Amended**: 2026-10-29 — the bottom-up and top-down review's findings, with the reasons recorded below.
**Amended**: 2026-10-29 — the first and second phases' findings: the brackets as the code has them, R3's
route, and the ladder's shape, with the reasons recorded below.
**ADR**: [ADR-0038](../../docs/adr/0038-the-context-taken-apart.md) — the context, taken apart
**Predecessor**: [029-the-lookup-shape-and-the-stage-that-moved](../029-the-lookup-shape-and-the-stage-that-moved/spec.md)

## The problem

Spec 029 answered the two questions spec 028 left, and in answering them it named a third. The
**context** is the gap between the join stage's **residual** and the same work measured outside the
query:

| | the code that ships | the code spec 029 replaced |
|---|---|---|
| the stage's residual, resident | **7.259 ms** | 7.372 ms |
| the standalone probe, resident | **8.395 ms** | 9.884 ms |
| **the context, resident** | **−1.069 ms** | **−2.409 ms** |
| the stage's residual, cold | **10.160 ms** | 9.854 ms |
| the standalone probe, cold | **9.661 ms** | 10.555 ms |
| **the context, cold** | **+0.494 ms** | **−1.440 ms** |

**And the context is not noise.** On the cold path the two numbers it is made of move **together**
across the campaigns — the stage's spread is **2.482 ms** and the standalone probe's is
**2.070 ms** — and what is left between them has a spread of **0.050 ms**. On the resident path
the context's spread is **0.282 ms** against a context of **−1.069 ms**. The context is small,
tight, and it **changes sign between the two code sets**.

**And it is not the lookup's shape.** The remainder — the engine's own probe over the hand-written
one — is **0.109 ms** with a spread of **1.083 ms**, and every part of the shape is inside it. The
context is something else, and it is the last unnamed thing in the join's stage.

**And the sign change is already explained, in part.** The two sets differ by one thing in the
engine: the lookup is now marked `#[inline]`. The old code's standalone probe paid a **call a probe
row** the in-query probe never paid, because the query's lookup and its caller are in one crate and
the instrument's are in two. Removing that artefact moved the standalone probe **1.489 ms** faster
on the resident path and **0.894 ms** faster on the cold one, and moved the in-query stage
**0.132 ms** and **0.376 ms**. The context rose by **1.340 ms** and **1.934 ms**. **The sign was the
crate boundary, seen from the other side** — and what the context is *now* is still unnamed.

**And the context is not what it was called.** It is not "the price of running inside a query rather
than in a quiet process". It is a **difference of three numbers taken in two different contexts**:

> **context** = [the stage, measured **in** the query] − [the build side, measured **outside** it] − [the probe, measured **outside** it]

The stage's clock and the two outside measurements' clocks **do not bracket the same work**. So the
context is the sum of **two** differences that have never been measured apart —

- the **build side's** in-query cost against its standalone cost, and
- the **probe's** in-query cost against its standalone cost —

and of whatever the two clocks bracket differently, part by part. **Neither has been measured.**

**And the residual is not "the stage's own probe".** The stage's clock carries the build side's
work, and the residual is what is left after the **standalone** build side is subtracted: so the
residual is the probe's in-query cost **plus** the build side's own in-query-minus-standalone gap.
The two are not separated by anything published, and the published documents call the residual "the
stage's own probe" — which is an attribution the measurement has not earned. **This spec takes the
context apart.**

### How the published figures are reduced

**Every figure this spec quotes is reduced one way, and the way is part of the method.** The
instrument derives each figure **per campaign**; the published figure is the **median of those
per-campaign values**. The **spread** is the **range** of those per-campaign values across the
three campaigns of that set. A difference **between two sets** is the **difference of their
medians**. This matters: the context is the median of the per-campaign differences, so **subtracting
the two medians in the table above does not reproduce it**. On the **resident** path the median of the
per-campaign differences is **−1.069 ms** while the difference of the two medians is **−1.136 ms** —
that path's two numbers are large and noisy, and the order of the reduction matters. On the **cold**
path the two routes nearly agree — **+0.494 ms** against **+0.499 ms** — because the stage and the
standalone probe move together there. The published figures are the medians, and this spec quotes
them.

### What the code says the five clocks bracket

Read out of the engine's own code, and confirmed against it — the first phase of the plan does it
again, and corrects this table if the code says otherwise:

| the clock | it brackets | it does **not** include |
|---|---|---|
| **the stage, cold** — `streamed.elapsed + build.opened` | **two disjoint intervals**: the build table's construction (`build_table`'s own clock), and then **the probe inside the reader** (`filter_elapsed`, the time in `JoinFilter::keep`) | the reader's decode and copy of the probe side, **and the gap between the two intervals** — the cold stage's clock is a **sum**, not a span |
| **the stage, resident** — `started.elapsed() + build.opened` | **one continuous interval**: the build table's construction, the probe key column's borrow (**O(1)** — `KeyColumn::from_chunk` borrows a plain `Int64` column's values), and the probe | the dataset's own read, which happened before the query |
| **the standalone probe, cold** — in the instrument's own reader | the probe, **plus** the per-batch key column's construction, **plus the row-number offset arithmetic** | the reader's decode, which is the scan's own work |
| **the standalone probe, resident** — over a column read from the file for that iteration | the probe alone | the column's read, which it has just done |
| **the standalone build side** — `build_table` in the instrument's process | the build table's construction alone | **the build side's file open**, which the stage's own clock carries |

**And two of the differences are already visible in the instrument's own order of operations.** The
instrument reads the build side's file **five times** — `build_read` runs the reader's own read five
times over — before it measures the build side's whole, so that measurement is over a **warm** file,
while the query reads it once, cold. (`build_keys` reads **nothing** from the file: it walks the
chunk already in memory.) And the standalone probe reads its column **from the file, for that
iteration**, and probes it warm; the query's resident probe probes a chunk the dataset has held
since it was loaded, with the build side's read in between. **Spec 029 measured the same kind of
artefact once already** — the candidate's warm buffer, worth **1.158 ms** — and that one was an
artefact of the instrument. **These two are candidates, not conclusions**: they are named as rungs
to be measured, and neither is claimed.

### What the code says, confirmed

The first phase read the clocks out of the code as it ships. The table above is the **hypothesis**; this is
what the code says, and the two differ in three places.

| the clock | it brackets | it does **not** include | the code |
|---|---|---|---|
| **the stage, cold** | **two disjoint intervals**: the build table's construction (`build_table`'s own clock), and then **the probe inside the reader** (`filter_elapsed`, the time in `JoinFilter::keep`) — and the build side's file open | the reader's decode and copy of the probe side, **and the gap between the two intervals** | `lib.rs`: `streamed.elapsed = build_elapsed + filter_elapsed`, and the stage is `streamed.elapsed + build.opened` |
| **the stage, resident** | **one continuous interval**: the build table's construction, the probe key column's borrow (**O(1)**), and the probe — and the build side's file open | the dataset's own read, which happened before the query | `pipeline.rs`: `started.elapsed() + pipeline.build.opened` |
| **the engine's own cold probe** | the per-batch key column's construction and the probe | **the row-number offset arithmetic**, which `keep_matching_rows` applies **outside** `filter.keep`'s clock | `dataset.rs`: `filter_elapsed` brackets `filter.keep(&batch, mask)` alone |
| **the standalone probe, cold** | the probe, **plus** the per-batch key column's construction, **plus the row-number offset arithmetic** | the reader's decode, which is the scan's own work | the instrument's `probe_in_batches` |
| **the standalone build side** | the build table's construction alone | **the build side's file open**, which the stage's own clock carries | the instrument's `build_whole` |

**The hypothesis was right about the cold stage's two disjoint intervals and about the offset**, and the
instrument's own comment — which says the offset is inside its clock "because the engine's cold path pays
it too" — is the thing that is wrong: the engine pays it, but **outside** its probe's clock. The comment is
corrected in the instrument.

**And two things the hypothesis did not have.**

- **The file's state is not in the build side's gap.** The hypothesis said "the query reads it once, cold".
  The code says otherwise: the campaign runs the query **five times**, and every execution reads the build
  side's file, so the **published figure — the median of five — is over a warm file**, and the standalone's
  measurement is over a warmer one still. The two sides read the file in the **same state**, so the file's
  state **cannot** explain any of the build side's gap. The instrument measures it, by taking one read of
  the build side's file before anything else reads it, and it comes to **0.205 ms** with a spread of
  **0.323 ms** — that is the **process's own** state, and it is worth **6.8 ms** more when the page cache
  did not have the file at all. It is therefore published as the **bound** on what the file's state could
  explain of the build side's gap, and not subtracted from it: a subtracted leaf would say the gap is
  explained by a file state that both sides share.
- **The published parts do not add up to the published context, and the reason is the reduction.** The
  stage's elapsed is the three parts **added together** in every iteration, and a **median of a sum is not
  the sum of the medians**. So the ladder is built **per iteration**, where the parts tile exactly, and the
  reconciliation — published — is the median of the difference over the iterations, which is **zero**. The
  difference between the parts' sum and the context, **taken per campaign**, is **the reduction's own
  noise**, and it is published under that name rather than folded away. **The closure is per campaign,
  and the published medians do not reproduce it**: the parts' medians do not add up to their per-campaign
  sum, because the parts move against each other and a median of a sum is not the sum of the medians.

### R3's route, and why

**The route taken is the plan output's parts.** The reason is what the two routes cost:

| | the plan output's parts | the marginal query |
|---|---|---|
| **the clocks it adds** | **none on the cold path** — the build side's construction and the probe are already timed apart, and the stage's clock only sums them | none |
| | **two clock reads and one subtraction** on the resident path, around `build_table` | none |
| **what it assumes** | nothing | that the build side's in-query cost is **the same** in both queries, and that a **marginal** cost is a **total** |
| **the added clock's own cost** | **0.000026 ms** — two reads at 0.000013 ms each, measured and published as `the_added_clock_cost` | none |

**The threshold** is **the floor's own spread across the campaigns of that set**, and the added clock's cost
is **0.000026 ms** against a floor's spread of hundreds of milliseconds: the route is **inside** the
threshold, and it is taken.

**The marginal route is refused, and the reason is published.** It needs the build side's in-query cost to
be **shown** to be the same in both queries, and the build side's **file state moves inside a process** — it
is worth **0.205 ms** between a process's first read of the build side's data and a later one, and
**6.8 ms** when the page cache did not have the file at all — which is the same order as the build side's
own gap, **0.343 ms**. So the two queries cannot be shown to have the same build side, and the fixed cost
they share would have to be **measured**, not assumed. The route that assumes it is not the route that measures it.

**And the plan output's public shape changes, because the route is taken.** `explain::JoinReport` is a
public struct with public fields, and the parts are added to it. The **rendered** plan output does not
change: the parts are in the struct, and the text is the text.

**And the two cold probe clocks do not bracket the same work.** The engine's in-query cold probe is
`JoinFilter::keep`: the per-batch key column's construction and the probe. The instrument's
standalone cold probe carries **the row-number offset arithmetic as well** — the batch's own row
numbers are made into the dataset's — and the engine adds that offset in `keep_matching_rows`,
**outside** its probe clock. The instrument's own comment says the offset is inside its clock
"because the engine's cold path pays it too"; **the code says otherwise**, and the asymmetry is
named here as a rung, not as a finding.

## The requirements

### R1 — The context is decomposed into parts, each measured by a candidate that differs in exactly one thing, and the parts sum to the context

**The context is one number made of three, and it is decomposed until every part of it is either
attributed or explicitly named as unexplained.**

The ladder is not a list of suspects. It is a set of **parts** — which **sum to the context** — and a
set of **controls**, which do not and never will:

| | |
|---|---|
| **the parts** | the build side's gap · the build side's file state · the probe's gap · the probe's file state · the probe's row-number offset · the clock's bracket, one named part at a time |
| **the controls** | the **floor** (what a memory-bound loop costs on this host) · the **scan's stage** (the reader's own work less the probe's) |
| **not a rung** | **the context itself**, which is the total the parts sum to |

Every part is measured against the engine's own rows — the **216,783** the join produced, and the six
million the probe examined — **before anything is timed**, and the aggregate's value is checked
against the published one.

**And the ladder reconciles.** The parts sum to the context, **what is left unexplained is named
and published with its spread**, and a ladder that does not close is not a finding; it is a list.

#### The ladder's parts, and what each one is

**The parts sum to the context, and the controls do not.** The parts are:

| the part | what it is | the word it carries |
|---|---|---|
| **the build side's gap** | the query's build side against the standalone's, over the same file state | **the build side's** |
| **the open's gap** | the query's file open against the standalone's — the stage's clock carries the open and the standalone build side's does not | **the clock's** |
| **the probe's file state** | the query's resident probe reads a column the dataset has held since it was loaded, with the build side's read in between; the standalone reads its column afresh | **the memory's** |
| **the probe's row-number offset** | the standalone's cold probe carries the offset and the query's clock does not, so it enters the probe's gap **against** it | **the clock's** |
| **the probe's own remainder** | what is left of the probe's gap | **the probe's** |
| **the part of the context in no gap** | zero when the ladder closes | **the host's** |

**And the controls, which are reported beside every set and are never summed:** the **floor** (what a
memory-bound loop costs on this host), the **scan's stage** (the reader's own work less the probe's), and
**the build side's file state** — the worth of the file's state, published as the **bound** on what the
file's state could explain of the build side's gap, and not subtracted from it.

**The ladder reconciles, and the reconciliation is published.** The parts are built **per iteration**,
where they tile the context exactly, and the reconciliation is the median of the difference over the
iterations. It is **zero**, and it is published so that the claim is checked rather than asserted.

**And the reduction's own noise is published.** The published parts' medians do not add up to the
published context's median, because a median of a sum is not the sum of the medians. The difference,
**taken per campaign**, is published under that name, and the published medians do not reproduce it.

### R2 — Every clock's bracket is written down, and no two measurements are compared until it is

**A figure without a stated bracket is not a measurement, it is a number.**

Every figure the ladder produces states, on its own row: what its clock **brackets**, and what it
**does not include**. The in-query stage's bracket and the standalone measurement's bracket are
published **side by side**, and **every difference between them is its own named part** — the
bracket is not one rung with several possibilities in it, it is several parts. A part that compares
two different intervals says so on its own row.

**And the bracket is not a caption; it is checked.** The instrument's clocks are read from the
engine's own code — `pipeline.rs`'s `started.elapsed()`, `streamed.elapsed`, `lib.rs`'s
`build_elapsed + filter_elapsed`, the instrument's `probe_over_column` and `probe_in_batches` — and
the first phase confirms, by reading, that the bracket it claims for each measurement is the bracket
the engine actually has.

### R3 — The build side's in-query cost is measured directly, and both routes are bounded

**The residual is the stage less the build side's **standalone** measurement. That subtraction is
across two contexts, and it is the reason the context exists at all.**

The build side's in-query cost must therefore be **measured**, in the query's own clock, in the
query's own process. Two routes are allowed, and the plan's second phase picks one on the
evidence:

- **the plan output's parts** — the join stage reports its build side and its probe apart, so the
  build side's in-query cost is the engine's own account of itself and not an inference.
- **the marginal query** — the same query with a probe side that differs by a known number of rows
  and **the same build side**, from which the probe's in-query cost follows and the build side's is
  what is left.

**And both routes are bounded, in writing.**

- **The marginal query** is attempted only on the same file and the same build side, and only if
  the build side's in-query cost is **shown** to be the same in both queries; otherwise the route is
  refused and the reason published. **A marginal cost is not a total**: the probe's in-query cost is
  the marginal cost over the rows **plus** the fixed cost the two queries share, and **the fixed
  cost is measured, not assumed**.
- **The plan output's parts** add clock reads inside the join's stage, and those reads cost
  something. The added clock's own cost is **measured and published**, and if it is not inside the
  threshold below, **the route is refused** and the reason published.

**And "changing the query's cost" has a number.** It means **slower by more than the floor's own
spread across the campaigns of that set**. The threshold is published with the measurement, and the
third exit — both routes breach it — is taken only when **both** are shown to breach it, with the
numbers that show it.

> **The requirement is rewritten, and the reason recorded.** It said "the plan's first phase picks one
> on the evidence", which left both routes unbounded: the marginal route assumed a build side that
> does not differ, and treated a marginal cost as a total, and the plan-output route added clock
> reads inside the stage whose own cost no requirement said to measure. The requirement now bounds
> both, gives "changing the query's cost" a number, and says that a route which breaches the
> threshold is refused with its reason published rather than taken.

### R4 — The verdict is published in one of six words, and the yardstick is the context's own spread

**The verdict is one of: the build side's, the probe's, the clock's, the memory's, the host's, or
unattributed.**

**The list is closed over the ladder's mechanisms.** A mechanism that is not on the list is
published as **unattributed** with its figures. **The floor is a control, not a part**: it is
reported beside every set and is **never summed**.

**And the yardstick is the context's own spread, in the set that measures it.** In the set the code
that ships was measured in, the context's spread is **0.282 ms** on the resident path and
**0.050 ms** on the cold one; in the set spec 029 replaced, **0.964 ms** and **0.793 ms**. Each set is
judged against its own band, and the band is published with the verdict.

**And the rule for a part is exact.**

- A part is **a part of the context** when its own value is **outside the band**.
- **Every part's own spread is published.** A part whose own spread is **wider than the band** is
  published as **inconclusive**, with both spreads, and is **not counted towards the verdict**. A
  part inside its own spread is published as **at the noise**.
- **"Unattributed" means no part accounted for more of the context than the band.** What is left of
  the context after the parts are summed is published as **what is left unexplained**, with its
  spread — and **it is not the verdict**.

> **The requirement is rewritten, and the reason recorded.** It said "a part of the context that is
> inside **its own** spread **is not a part of the context**", whose antecedent was ambiguous — the
> band is the **context's** spread, and the rule is about the part's own value against that band —
> and it had no case for a part noisier than the band itself. It also listed five words for a ladder
> with a mechanism the list did not carry, and never defined "unattributed" against R1's "what is
> left unexplained". The requirement now says which spread is the band, what happens to a part
> wider than the band, closes the list over the ladder, and separates the verdict from the
> unexplained remainder.

### R5 — The measurement is three campaigns each, interleaved, with the floor in every campaign, and the two codes are named in the report

**The two codes are measured back to back with each other, not minutes apart.** Three campaigns
each, **interleaved** — old, new, old, new — in one session, with the **floor** in every campaign.

The **floor** is the instrument's own measure of what a memory-bound loop costs on this host: it is
**not** a property of the code, and that is exactly what makes it the control for the machine. The
context is the control for the **process**, and it is measured in both sets.

**And the instrument is byte-identical in both sets.** Only the engine's own file changes; a
comparison between two instruments is not a comparison of two codes.

**And the two codes are named in the report, not only in its commit.** The report carries the
commit it was taken at, and **the commit alone does not distinguish the two codes**: spec 029's
before-set and its after-set carry **the same commit**, because the before-set's tree was dirty.
So the report names the **code** each campaign measured, and the two names are published beside the
figures.

> **The requirement is rewritten, and the reason recorded.** It said "the reports carry the commit
> they were taken at, and an uncommitted tree is not a measurement of anything" — which is true and
> not enough: the commit is the same for both sets of the A/B this spec repeats, because the code
> that is replaced was never committed in that state. The requirement now asks the report to name
> the **code** each campaign measured.

### R6 — The answer does not change, and the query's cost does not change

**The join's rows, their order, their count and the aggregate's value are the same** — the same
rows in the same order, with the same count a key, and a NULL key that still never matches.

**And the query's cost does not change by more than the threshold R3 names.** A change that makes
the query slower by more than **the floor's own spread across the campaigns of that set** is not a
change, and the figure says so.

The tests that already hold the behaviour keep passing, and they are named here by their
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

**A figure that moved is published with the figure it replaced and the reason it moved.** The
increment moves figures in the places that publish the context and the stage's residual:

| where | the figure | where its old value lives |
|---|---|---|
| [`PERF.md`](../../PERF.md) §3o | the context, the residual, the parts and the reconciliation | §3o's own "the old code" column |
| [`PERF.md`](../../PERF.md) §2h | the stage table's rows | §2h's own "the set this one replaces" table |
| [`README.md`](../../README.md) | the Next row's context | the row's previous values |
| [`STATUS.md`](../../STATUS.md) | the context's open question | the file's own open-questions section |

**And the sets are kept.** The repository's convention is that **every superseded set is preserved
under its own name**, so that every figure published old-against-new stays derivable. The sets
spec 029 published become `join-cost-spec029-run*.json` and `join-cost-spec029-before-run*.json`, and
this increment's sets take the current names — measured **back to back** with each other, on the
code this spec replaces and on the code it ships.

**And the guard follows them, and it is told exactly how.**

- `scripts/check-published-figures.py` gains the two new report globs, the two new entries in its
  set list, and **one entry per part** in its parts list and its differences list — each with its
  decimal places, three for the ladder's figures.
- **The ladder is published inside §3o, under no new `###` heading.** The guard's section lookup
  ends at the next `## ` or `### `, so a new `###` inside §3o would end the section and stop every
  join figure from being checked. A `####` sub-heading is safe.
- **The context's spread is published and checked.** The report carries no spread for the derived
  figures, so the spread is computed across the campaigns by `scripts/medians.py` and by the guard,
  and both derive it the same way.
- **A missing set fails the guard.** Today a missing join-cost set is skipped and the guard still
  passes; after this increment, a set with fewer than three campaigns is a failure, and a family is
  never skipped silently.
- `scripts/medians.py` derives the parts and the reconciliation, and prints the spread.
- `bench-results/README.md` gains a row per new set, and its §3o row lists the new globs.
- **The README's Next row's context figures are added to the guard**, which today checks three
  phrases in that row and not these.

### R8 — The open question is answered in the open, whether the context is explained or not

**The next question is written down where this one was** — the README's Next row and
[`STATUS.md`](../../STATUS.md)'s last section — and it is written down **either way**. A context
that is attributed to the clock's bracket is an answer; a context that is **unattributed** is also an
answer, and it is published with the numbers that show it.

## The scope

**What this spec does not do, and why.**

- **The lookup's shape is not changed.** Spec 029 measured it part by part: the remainder is
  **0.109 ms** with a spread of **1.083 ms**, and every part of the shape is inside it. The shape
  is free.
- **The `#[inline]` is not undone.** It is what makes the instrument's measurement the query's cost,
  and this spec's whole method rests on that.
- **The build side's read is not changed.** It is Arrow's decode of `orders.parquet`, and it has
  been measured three times and refused every time. This spec **measures** the build side's cost in
  the query; it does not try to make it smaller.
- **The table's shape is not changed**, and neither is the overflow map's semantics, nor either
  hasher, nor the hashed path.
- **The scan's stage is the control, and it is not touched.** The scan's elapsed is the reader's own
  work **less** the probe's, so the probe's movement cancels out of it.
- **The plan is not changed.** No new plan, no new operator, no new join type. The plan **output**
  may report the join stage's parts — that is what the engine **says**, not what it **does** — and
  only if R3's route is taken, only if the added clock's own cost is inside R3's threshold, and only
  with the reason recorded below.
- **The plan's shape, the pipeline's stages and the stage's rows are not changed.** The stage
  reports the rows the probe **consumed**; that rule stands.
- **The comparison campaigns are not taken again.** This increment touches no query's cost, so no
  published query figure moves, and the comparison set in §2 stays the one spec 028 published.
- **The resident path's asymmetry stands.** The engine's resident path reads the build side
  **inside** its clock, where the incumbents read it before the clock starts. A resident-stage
  improvement is therefore **not** comparable with the incumbents' resident figures, and this spec
  does not claim it is.
- **The instrument's own key source is the instrument's**, and it does not become the engine's.
- **The comparison campaigns' figures are not compared with the instrument's.** They are different
  measurements of the same work, and each is published with the set it came from.
- **No ARM64 figure is compared with an x86-64 one.** Every figure here is ARM64, on this host, with
  the load average the reports carry.
- **The `filter-range` gap, the `join-orders` gap, the morsel driver and the x86-64 absolute
  figures are not this spec's.** They are named in [`STATUS.md`](../../STATUS.md) and stay there.

> **The scope's carve-out is recorded, and the reason.** The house treats a plan and a plan output
> as different things — a `QueryPlan` is the plan, a `PlanOutput` is the engine's own account of its
> work — and spec 026 recorded the same carve-out as an amendment, because what changed there was
> "where a rule lives, **what the stage reports**". Spec 030 does the same, and adds the condition
> spec 026 did not need: **the plan output's public shape changes only if R3's route is taken**, and
> then the scope's "the engine's public API is not changed" is **amended** — `explain::JoinReport`
> is a public struct with public fields, and adding the parts to it is a public change. **If the
> route is not taken, the public API is not changed.**

## The instrument

`crates/columnar-engine/examples/join_cost_probe.rs`, committed in the tree by spec 026 and extended
by spec 029, gains the parts of R1.

**And the parts are measured across two places, and each says which it is.** The **standalone**
halves — the build side's whole, the probe on each path, the file states, the row-number offset —
come from the instrument, in the instrument's own process. The **in-query** halves come from the
engine's own plan output, in the query's process. Every part's row states which of the two it is
and which bracket it carries (R2). **The instrument does not become the engine's**, and the engine's
figure is never compared with the instrument's without the two brackets being published side by
side.

**And the engine's rows are checked against every part's, in order, before anything is timed**, and
the aggregate's value is checked against the published one — the same rule spec 029's ladder held.

**And the report gains the fields the parts need.** The report already carries the context
(`the_context_cold`, `the_context_resident`) and the floor (`floor_probe`). It gains: the build
side's **in-query** cost and the probe's **in-query** cost, or the parts R3's route produces; the
build side's **file state**; the probe's **file state**; the probe's **row-number offset**; and the
**name of the code** each campaign measured (R5). The **spread** of the derived figures is computed
across the campaigns by the two scripts, because the report carries spreads for the parts only.

## The measurement

**The plan, so that a stranger can run it:**

| what | how | where it lands |
|---|---|---|
| the commits | the code this spec writes, and the code it replaces — spec 029's — measured **back to back** with it | the spec's own history, and the **name of the code** in each report |
| the join probe's campaigns | `just probe-join "data/lineitem.parquet" "bench-results/join-cost-run1.json"`, three campaigns, five iterations each | `join-cost-run*.json`; the before-set, on the code this spec replaces and measured back to back with it, is `join-cost-before-run*.json`; the sets spec 029 published are preserved as `join-cost-spec029-run*.json` and `join-cost-spec029-before-run*.json` |
| the interleaved A/B | the two codes, **three campaigns each, interleaved**, in one session, with the **floor** in every campaign and the instrument **byte-identical** | the measurement R4 and R5 turn on — a set, not a diagnostic, because a verdict rests on it |
| the host | an Apple M4 Max, arm64, one thread, and the load average — the join probe's reports **carry** it | in every report |
| the guard | `scripts/medians.py` derives the tables and the spreads; `scripts/check-published-figures.py` checks every published figure against the reports, in the section that publishes it, and **fails when a set is missing or short** | the CI job "published figures match the evidence" |

**And the order of the campaigns is part of the method.** The two codes are measured **interleaved**
— old, new, old, new — so that a drift in the host's state is shared rather than attributed. The
**code is committed before** the campaign, and **the report names the code it measured**, because the
commit alone does not distinguish a dirty tree.

**And the phases**, so that the order of the work is part of the method too:

1. **The brackets.** Read the engine's own clock — `pipeline.rs`'s `started.elapsed()`,
   `streamed.elapsed`, `lib.rs`'s `build_elapsed + filter_elapsed`, the instrument's
   `probe_over_column` and `probe_in_batches` — and write down, for each, what it **brackets** and
   what it **does not include**. The table above is the **hypothesis**; this phase confirms it
   against the code as it ships, and corrects it if the code says otherwise.
2. **The route.** Pick R3's route on the evidence: the plan output's parts, or the marginal query.
   If neither can be taken without breaching R3's threshold, publish that and stop.
3. **The ladder.** Measure the parts, each differing from the last by exactly one thing, with the
   engine's rows checked first, and with **each part's own spread** published beside it.
4. **The campaign.** Three campaigns each, interleaved, with the floor in every campaign, and the
   reconciliation: the parts sum to the context, and what is left unexplained is named.
5. **The answer does not change.** The named tests are run, the unit test is **read again**, and the
   query's cost is measured against R3's threshold.
6. **The publication.** Every figure that moved, old-and-new, with the reason; the verdict in one
   of R4's six words; the next question written down where this one was.

## The references

| | |
|---|---|
| [constitution.md](../../constitution.md) | **IV. Evidence over claims** — R1, R2, R4 and R7: a figure is checked against the report it came from, and a figure without a stated bracket is not a measurement |
| [constitution.md](../../constitution.md) | **VII. Explicit Scope, No Silent Deferral** — R3's two bounded routes and its third exit, and the scope's carve-out recorded with the reason |
| [constitution.md](../../constitution.md) | **"Complexity requires justification"** — the two routes, the ladder's parts, and the A/B's four campaigns each |
| [constitution.md](../../constitution.md) | **"Compliance is checked per spec"** — this section's citation of the principle each non-obvious requirement enforces |
| [ADR-0035](../../docs/adr/0035-the-join-stage-taken-apart.md) | the join's stage, taken apart, and the residual this spec's context is measured against |
| [ADR-0036](../../docs/adr/0036-the-probes-remainder-taken.md) | the probe's remainder, taken: the map was never it |
| [ADR-0037](../../docs/adr/0037-the-lookups-shape-and-the-stage-that-moved.md) | the lookup's shape, the two artefacts of the instrument, and the context this spec takes up |
| [spec 023](../023-the-scan-that-feeds-the-join/spec.md) | the probe moved inside the scan, and the copy of the rows the join discards removed |
| [spec 024](../024-the-time-the-plan-output-does-not-name/spec.md) | the plan output's own account of the query, and the rule that a stage's rows are the rows it consumed |
| [spec 026](../026-the-join-stage-taken-apart/spec.md) | the join's stage taken apart on both paths, the derivation the stage's residual comes from, and the same carve-out between a plan and a plan output |
| [spec 028](../028-the-probes-remainder/spec.md) | the probe's remainder, and the context it named as the control for the process |
| [spec 029](../029-the-lookup-shape-and-the-stage-that-moved/spec.md) | the lookup's shape, the crate boundary, and the figures this spec takes up |
| [`PERF.md`](../../PERF.md) §2h, §3l, §3o | the published tables this spec's figures are re-derived into; §3l is the scan's stage, which is the ladder's control |
| [`STATUS.md`](../../STATUS.md) | where the project's open questions live |
