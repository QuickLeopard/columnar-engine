# Spec 030 — The context, taken apart

**Status**: in progress
**Started**: 2026-10-29
**Predecessor**: [029-the-lookup-shape-and-the-stage-that-moved](../029-the-lookup-shape-and-the-stage-that-moved/spec.md)

## The problem

Spec 029 answered the two questions spec 028 left, and in answering them it named a third. The
**context** is the gap between the join stage's own probe and the same probe measured outside the
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
This spec takes the context apart.

### What the code says the five clocks bracket

| the clock | it starts before | it stops after | it does not include |
|---|---|---|---|
| **the stage, cold** — `streamed.elapsed + build.opened` | the build side's read | the probe's last row | the probe side's read, which is the scan's own stage |
| **the stage, resident** — `started.elapsed() + build.opened` | the build side's read | the probe's last row | the dataset's own read, which happened before the query |
| **the standalone probe, cold** — in the instrument's own reader | the probe's first row | the probe's last row | the reader's decode, which is the scan's own work |
| **the standalone probe, resident** — over a column read from the file for that iteration | the probe's first row | the probe's last row | the column's read, which it has just done |
| **the standalone build side** — `build_table` in the instrument's process | the build side's read | the table's last row | everything else the process is holding |

**And the two measurements are not even in the same kind of process.** The standalone ones run in
the instrument's process, which holds the table, the counts and the column it is probing. The
in-query ones run in the query's process, which holds the dataset, the selection, the scan's
batches and the build side's own decode. **What is in memory around the probe is different in the
two measurements, and that difference has never been measured either.**

**And two of the differences are already visible in the instrument's own order of operations.** The
instrument reads the build side's file **twice** before it measures the build side's whole —
`build_read` and `build_keys` come first — so the standalone build side is measured over a **warm**
file, while the query reads it once, cold. And the standalone probe reads its column **from the
file, for that iteration**, and probes it warm; the query's resident probe probes a chunk the
dataset has held since it was loaded, with the build side's read in between. **Spec 029 measured the
same kind of artefact once already** — the candidate's warm buffer, worth **1.158 ms** — and that
one was an artefact of the instrument. **These two are candidates, not conclusions**: they are named
as rungs to be measured, and neither is claimed.

## The requirements

### R1 — The context is decomposed into named gaps, and each is measured by a candidate that differs in exactly one thing

**The context is one number made of three, and it is decomposed until every part of it is either
attributed or explicitly named as unexplained.**

The ladder is not a list of suspects; it is a set of **rungs**, and each rung differs from the one
before it by **exactly one thing**, the way spec 029's ladder did:

| the rung | what it is |
|---|---|
| **the context** | the gap itself, and the yardstick every other rung is judged against |
| **the build side's gap** | the build side's in-query cost − its standalone cost |
| **the build side's file state** | the instrument reads the build side's file **twice** before it measures the build side's whole — `build_read` and `build_keys` come first — so its measurement is over a **warm** file, while the query reads it once, cold |
| **the probe's gap** | the probe's in-query cost − its standalone cost |
| **the probe's file state** | the standalone probe reads its column **from the file, for that iteration**, and probes it warm; the query's resident probe probes a chunk the dataset has held since it was loaded, with the build side's read in between |
| **the clock's bracket** | each part the two clocks bracket differently, named one part at a time |
| **the memory's state** | the same probe, over the same rows, in a process that holds what the query holds |
| **the host's** | what the floor says about the machine under both measurements |

Every rung is measured against the engine's own rows — the **216,783** rows the join produced, and
the six million the probe examined — **before anything is timed**, and the aggregate's value is
checked against the published one.

**And the ladder reconciles.** The rungs sum to the context, and **what is left unexplained is
named and published with its spread**. A ladder that does not close is not a finding; it is a list.

### R2 — Every clock's bracket is written down, and no two measurements are compared until it is

**A figure without a stated bracket is not a measurement, it is a number.**

Every figure the ladder produces states, on its own row: what its clock **starts before**, what it
**stops after**, and what it **does not include**. The in-query stage's bracket and the standalone
measurement's bracket are published **side by side**, and **every difference between them is a named
rung**. A rung that compares two different intervals says so on the figure's row, and is not counted
as evidence for a mechanism it does not measure.

**And the bracket is not a caption; it is checked.** The instrument's clocks are read from the
engine's own code — the same `pipeline.rs` lines the query runs — and the ladder's first task is to
confirm, by reading, that the bracket it claims for each measurement is the bracket the engine
actually has.

### R3 — The build side's in-query cost is measured directly, and not inferred by subtracting a number from another context

**The residual is the stage less the build side's **standalone** measurement. That subtraction is
across two contexts, and it is the reason the context exists at all.**

The build side's in-query cost must therefore be **measured**, in the query's own clock, in the
query's own process. Two routes are allowed, and the plan's first phase picks one on the evidence:

- **the plan output's parts** — the join stage reports its build side and its probe apart, so the
  build side's in-query cost is the engine's own account of itself and not an inference. This
  changes what the plan output **says**, not what the query **does**, and the clock it adds is
  measured and published.
- **the marginal query** — the same query with a probe side that differs by a known number of rows
  and a build side that does not differ at all, from which the probe's in-query cost follows and
  the build side's is what is left.

**And if neither route can be taken without changing the query's cost, that is the finding** — the
context is then published as the difference between two brackets that cannot be aligned, with the
numbers that show it, and the next question is written down where this one was.

### R4 — The verdict is published in one of five words, and the yardstick is the context's own spread

**The verdict is one of: the build side's, the probe's, the clock's, the host's, or unattributed.**

The yardstick is **the context's own spread, in the set that measures it**. In the set the code that
ships was measured in, the context's spread is **0.282 ms** on the resident path and **0.050 ms** on
the cold one; in the set spec 029 replaced, **0.964 ms** and **0.793 ms**. A part of the context
that is inside its own spread **is not a part of the context**, and each set is judged against its
own band.

**And more than one word may hold.** When the ladder attributes the context to more than one
mechanism, the **largest** is the verdict and the others are published beside it with their figures.

### R5 — The measurement is three campaigns each, interleaved, with the floor in every campaign

**The two codes are measured back to back with each other, not minutes apart.** Three campaigns
each, **interleaved** — old, new, old, new — in one session, with the **floor** in every campaign.

The **floor** is the instrument's own measure of what a memory-bound loop costs on this host: it is
**not** a property of the code, and that is exactly what makes it the control for the machine. The
context is the control for the **process**, and it is measured in both sets.

**And the instrument is byte-identical in both sets.** Only the engine's own file changes; a
comparison between two instruments is not a comparison of two codes.

### R6 — The answer does not change, and the query's cost does not change

**The join's rows, their order, their count and the aggregate's value are the same** — the same
rows in the same order, with the same count a key, and a NULL key that still never matches.

**And the query's cost does not change.** If R3's route is the plan output's parts, the clock it
adds is measured and published, and the change is to what the engine **says**, never to what it
**does**. A change that makes the query slower is not a change, and the figure says so.

The tests that already hold the behaviour keep passing, and they are named here so that a stranger
knows what "does not change" means:

| test | what it holds |
|---|---|
| `crates/columnar-engine/tests/join.rs` — *a build key more than 255 rows carry still counts every one* | the overflow map's semantics |
| `crates/columnar-engine/tests/join.rs` — *a probe key below the table's base matches nothing* | the range rule |
| `crates/columnar-engine/tests/join.rs` — *a NULL key never matches on either side* | the NULL rule |
| `crates/columnar-engine/src/operators/join.rs` — the **unit test** | the direct table's answer against the hash table's, key for key |

### R7 — Every figure that moved is published old-and-new, and the sets are preserved under their own names

**A figure that moved is published with the figure it replaced and the reason it moved.** The
increment moves figures in the places that publish the context and the stage's residual:

| where | the figure | where its old value lives |
|---|---|---|
| [`PERF.md`](../../PERF.md) §3o | the context, the residual, the ladder and the reconciliation | §3o's own "the old code" column |
| [`PERF.md`](../../PERF.md) §2h | the stage table's rows | §2h's own "the set this one replaces" table |
| [`README.md`](../../README.md) | the Next row's context | the row's previous values |
| [`STATUS.md`](../../STATUS.md) | the context's open question | the file's own open-questions section |

**And the sets are kept.** The repository's convention is that **every superseded set is preserved
under its own name**, so that every figure published old-against-new stays derivable. The sets
spec 029 published become `join-cost-spec029-run*.json` and `join-cost-spec029-before-run*.json`, and
this increment's sets take the current names — measured **back to back** with each other, on the
code this spec replaces and on the code it ships.

`scripts/medians.py` and `scripts/check-published-figures.py` are extended for the ladder's rungs and
the new sets, and the guard checks every published figure — the old set's and the new set's —
against the reports, **in the section that publishes it**.

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
  only if R3's route is taken and only if the clock it adds is published.
- **The plan's shape, the pipeline's stages and the stage's rows are not changed.** The stage
  reports the rows the probe **consumed**; that rule stands.
- **The comparison campaigns are not taken again.** This increment touches no query's cost, so no
  published query figure moves, and the comparison set in §2 stays the one spec 028 published.
- **The resident path's asymmetry stands.** The engine's resident path reads the build side
  **inside** its clock, where the incumbents read it before the clock starts. A resident-stage
  improvement is therefore **not** comparable with the incumbents' resident figures, and this spec
  does not claim it is.
- **The instrument's own key source is the instrument's**, and it does not become the engine's.
- **The engine's public API is not changed.**
- **The comparison campaigns' figures are not compared with the instrument's.** They are different
  measurements of the same work, and each is published with the set it came from.
- **No ARM64 figure is compared with an x86-64 one.** Every figure here is ARM64, on this host, with
  the load average the reports carry.
- **The `filter-range` gap, the `join-orders` gap, the morsel driver and the x86-64 absolute
  figures are not this spec's.** They are named in [`STATUS.md`](../../STATUS.md) and stay there.

## The instrument

`crates/columnar-engine/examples/join_cost_probe.rs`, committed in the tree by spec 026 and extended
by spec 029, gains the ladder of R1: the build side's gap, the probe's gap, the clock's bracket, the
memory's state and the host's floor.

**The engine's rows are checked against every rung's, in order, before anything is timed**, and the
aggregate's value is checked against the published one — the same rule spec 029's ladder held.

**And the instrument does not become the engine's.** Every rung is the instrument's own measurement,
in the instrument's own process; the engine is measured by the engine's own plan output. The two
are compared only where their brackets are the same, and R2 says where they are not.

## The measurement

**The plan, so that a stranger can run it:**

| what | how | where it lands |
|---|---|---|
| the commits | the code this spec writes, and the code it replaces — spec 029's — measured **back to back** with it | the spec's own history |
| the join probe's campaigns | `just probe-join "data/lineitem.parquet" "bench-results/join-cost-run1.json"`, three campaigns, five iterations each | `join-cost-run*.json`; the before-set, on the code this spec replaces and measured back to back with it, is `join-cost-before-run*.json`; the sets spec 029 published are preserved as `join-cost-spec029-run*.json` and `join-cost-spec029-before-run*.json` |
| the interleaved A/B | the two codes, **three campaigns each, interleaved**, in one session, with the **floor** in every campaign and the instrument **byte-identical** | the measurement R4 and R5 turn on — a set, not a diagnostic, because a verdict rests on it |
| the host | an Apple M4 Max, arm64, one thread, and the load average — the join probe's reports **carry** it | in every report |
| the guard | `scripts/medians.py` derives the tables; `scripts/check-published-figures.py` checks every published figure against the reports, in the section that publishes it | the CI job "published figures match the evidence" |

**And the order of the campaigns is part of the method.** The two codes are measured **interleaved**
— old, new, old, new — so that a drift in the host's state is shared rather than attributed. The
**code is committed before** the campaign: the reports carry the commit they were taken at, and an
uncommitted tree is not a measurement of anything.

**And the phases**, so that the order of the work is part of the method too:

1. **The brackets.** Read the engine's own clock — `pipeline.rs`'s `started.elapsed()`,
   `streamed.elapsed`, the instrument's `probe_over_column` and `probe_in_batches` — and write down,
   for each, what it starts before, what it stops after and what it does not include. The table
   above is the **hypothesis**; this phase confirms it against the code as it ships, and corrects
   it if the code says otherwise.
2. **The route.** Pick R3's route on the evidence: the plan output's parts, or the marginal query.
   If neither can be taken without changing the query's cost, publish that and stop.
3. **The ladder.** Measure the rungs, each differing from the last by exactly one thing, with the
   engine's rows checked first.
4. **The campaign.** Three campaigns each, interleaved, with the floor in every campaign, and the
   reconciliation: the rungs sum to the context, and what is left unexplained is named.
5. **The publication.** Every figure that moved, old-and-new, with the reason; the verdict in one
   of R4's words; the next question written down where this one was.

## The references

| | |
|---|---|
| [ADR-0035](../../docs/adr/0035-the-join-stage-taken-apart.md) | the join's stage, taken apart, and the residual this spec's context is measured against |
| [ADR-0036](../../docs/adr/0036-the-probes-remainder-taken.md) | the probe's remainder, taken: the map was never it |
| [ADR-0037](../../docs/adr/0037-the-lookups-shape-and-the-stage-that-moved.md) | the lookup's shape, the two artefacts of the instrument, and the context this spec takes up |
| [spec 023](../023-the-scan-that-feeds-the-join/spec.md) | the probe moved inside the scan, and the copy of the rows the join discards removed |
| [spec 024](../024-the-time-the-plan-output-does-not-name/spec.md) | the plan output's own account of the query, and the rule that a stage's rows are the rows it consumed |
| [spec 026](../026-the-join-stage-taken-apart/spec.md) | the join's stage taken apart on both paths, and the derivation the stage's residual comes from |
| [spec 028](../028-the-probes-remainder/spec.md) | the probe's remainder, and the context it named as the control for the process |
| [spec 029](../029-the-lookup-shape-and-the-stage-that-moved/spec.md) | the lookup's shape, the crate boundary, and the figures this spec takes up |
| [`PERF.md`](../../PERF.md) §2h, §3l, §3o | the published tables this spec's figures are re-derived into |
| [`STATUS.md`](../../STATUS.md) | where the project's open questions live |
