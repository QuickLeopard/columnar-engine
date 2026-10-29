# Spec 029 — The lookup's shape, and the stage that moved

**Status**: in progress
**Started**: 2026-10-29
**Amended**: 2026-10-29 — the functional decomposition's findings, bottom-up and top-down, with the reasons recorded below.
**Predecessor**: [028-the-probes-remainder](../028-the-probes-remainder/spec.md)

## The problem

Spec 028 took the probe's remainder apart, and it left two things open. This spec takes both.

### The shape is a name, not a measurement

**The map was never the remainder, and what is left has a name but no number.** The candidate **with** the
overflow map's consult in it costs **7.106 ms** against the candidate without it at **7.157 ms**: the map's
share is **0.020 ms** of a **3.337 ms** remainder. Each set's own three campaigns are judged against **its
own** candidate spread — **−0.221 ms … 0.406 ms** against **0.709 ms** in the old set, **−0.278 ms …
0.071 ms** against **0.168 ms** in the new one — so the share is **at or below the noise** in both. What is
left is **the lookup's shape**, published as "the method over `&self`, the `Option` chain, the `u32` width
and the saturating add".

**That is a name, not a measurement**, and the decomposition of the code says the name is **stale**. The
engine's lookup, as it ships:

```rust
fn count(&self, key: i64) -> u32 {
    let index = usize::try_from(key.saturating_sub(self.base)).unwrap_or(usize::MAX);
    let byte = self.counts.get(index).copied().map_or(0, u32::from);
    let Some(overflow) = &self.overflow else {
        return byte;
    };
    let extra = overflow.get(&key).copied().unwrap_or(0);
    byte.saturating_add(extra)
}
```

against the candidate it is measured against:

```rust
let offset = usize::try_from(key.saturating_sub(base)).unwrap_or(usize::MAX);
let count = counts.get(offset).copied().unwrap_or(0);
```

The two do the same arithmetic over the same table, and they differ in exactly four places — and **two of
the four names published for the shape are not in the code that ships**:

- the **saturating add** is **not on the fast path**: the `Option` returns the byte before it, so a lookup
  that overflows nothing never adds. The name survives in the code, off the path the probe takes.
- the **map's consult** — the `get` that spec 028 measured at **0.020 ms** — is **not on the fast path
  either**: the same early return skips it. What the fast path still pays is the **map's `Option`
  discriminant** and the branch on it, and **that has never been measured**.
- the **`u32` width** is on the fast path: the byte is widened through `map_or(0, u32::from)`, and the
  candidate keeps it a `u8`. Unmeasured.
- the **method over `&self`** is on the fast path: the candidate's state is a local slice, the engine's is
  reached through `self`. Unmeasured.

**And the `Option` chain is two chains, not one.** The byte's read is a chain in both — `map_or` against
`unwrap_or` — and the map's is a second one, on the fast path, that nobody has priced. The four names
cannot be a decomposition until each is a number.

### The movement is in the stage's residual

**The change that removed the map's consult bought nothing the query can see.** The probe's own measurement
is **0.950 ms** faster, but the **floor** — the table's own load, one a probe row, which the change cannot
touch — moved **1.514 ms slower** in the same two sessions, and the **resident** join stage moved the other
way. In this increment's two campaigns — the code it replaces and the code it writes, back to back on this
host — the stage's own parts read:

| | the old code | the new code | the difference |
|---|---|---|---|
| the stage, cold | 25.271 ms | 26.550 ms | **+1.279 ms** |
| the build side, whole | 12.289 ms | 11.903 ms | **−0.386 ms** |
| the build side's open | 0.030 ms | 0.029 ms | **−0.001 ms** |
| the residual, cold | 12.952 ms | 14.618 ms | **+1.666 ms** |
| the stage, resident | 19.797 ms | 22.106 ms | **+2.309 ms** |
| the residual, resident | 7.478 ms | 10.174 ms | **+2.696 ms** |
| the probe, over the column | 11.373 ms | 10.423 ms | **−0.950 ms** |
| the probe, in the batches | 14.826 ms | 13.921 ms | **−0.905 ms** |
| the candidate, written out | 7.159 ms | 7.157 ms | **−0.002 ms** |
| the floor the probe cannot go under | 4.753 ms | 6.267 ms | **+1.514 ms** |

**The parts got faster and the residual moved the other way**, on both paths: **+1.666 ms** cold and
**+2.696 ms** resident, while the probe inside the residual got **0.950 ms** and **0.905 ms** faster and the
build side's whole got **0.386 ms** faster. The residual — the stage less the build side's whole and the
open — is where the movement is, and it is not a part anybody measured.

**And the movement is not one measurement.** It appears in three independent campaign pairs, all in the same
direction: the comparison campaigns' resident join stage **22.2 ms → 23.6 ms**, the join probe's campaigns'
resident stage **19.797 ms → 22.106 ms**, and the same-moment A/B **21.7 ms → 23.2 ms**. The scan's stage —
the control — moved **0.3 ms** the other way.

### The residual is less than a measurement of part of itself

**The residual is not the probe, and the difference between them is not a constant.** The residual and the
standalone probe of the same work disagree in **every** set published so far:

| set | the residual | the standalone probe | the difference |
|---|---|---|---|
| the code before spec 026, resident | 7.445 ms | 10.270 ms | **−2.825 ms** |
| spec 026's code, resident | 7.357 ms | 10.051 ms | **−2.694 ms** |
| the code spec 026 replaced, cold | 8.744 ms | 10.550 ms | **−1.806 ms** |
| spec 026's code, cold | 8.876 ms | 10.221 ms | **−1.345 ms** |
| the code spec 028 replaced, resident | 7.478 ms | 11.373 ms | **−3.895 ms** |
| the code spec 028 replaced, cold | 12.952 ms | 14.826 ms | **−1.874 ms** |
| spec 028's code, resident | 10.174 ms | 10.423 ms | **−0.249 ms** |
| spec 028's code, cold | 14.618 ms | 13.921 ms | **+0.697 ms** |

**A residual that is less than a measurement of the probe inside it is not a decomposition**, and the
difference changes **sign** on the cold path between the two codes of this increment. The parts are
**bounds**, not a decomposition: the stage is the authority.

**And there is a third thing the parts do not see.** The instrument measures its parts in a **quiet
process**, one after another, and the stage is measured **inside a query** that has already read a file
and touched the memory. The residual's gap to the standalone probe is the price of that context, and it is
worth **3.895 ms** on the old code and **0.249 ms** on the new one. **Nobody has measured the context**, and
it is a candidate for the movement: the machine's memory behaviour is not the same in the two sessions, and
the floor — the instrument's own measure of what a memory-bound loop costs on this host — moved **1.514 ms**
slower, in the same direction and the same order as the residual's growth.

### The decisive experiment was not run

**The floor and the stage are measured together, but the two codes are not.** The join probe's campaigns
carry the **floor** and the **stage** in the same campaign, so the floor is not a separate measurement: it
is the instrument's own figure, in the same report, on the same rows. What is missing is the **gap**. The
two codes' campaigns are **minutes apart**, and the floor moved **1.514 ms** across that gap — so the
floor's movement is measured **across a gap**, and a drift in the host's state is attributed to the code
that happened to be measured second.

**And the one pair that puts the two codes in the same minutes carries no floor.** The same-moment A/B —
the old code and the new, minutes apart on the same host — carries the **plan output's** stages and **not**
the instrument's parts. So the measurement that could separate the machine from the code was never taken:
it is the A/B **with** the floor, taken **interleaved**, three campaigns each.

**And the prize is bounded, not the remainder.** The stage charges its **own** probe, and the parts are
measured in a different process. A win on the standalone probe does not arrive at the query one for one,
and the figure that moves a published query is the stage's.

**So there are two questions.** What is the remainder made of, part by part, so that a change to the lookup
is measured rather than guessed? And what moved the resident stage — the machine, the context, the code, or
nothing that can be named yet?

## What the review found

The spec was decomposed twice — **bottom-up**, from the obligations to the functions that carry them, and
**top-down**, from the goal to the obligations that reach it — and the two do not agree. The differences
are what the review changed.

**Bottom-up**, the spec's obligations are these, and each one has a home:

| the obligation | the home | the requirement |
|---|---|---|
| a candidate that prices one thing at a time, over the same rows and the same table | the instrument | R1 |
| each candidate's rows checked against the engine's, in order, before anything is timed | the instrument | R1 |
| the parts reconciled against the remainder, with what is left unexplained named | `scripts/medians.py`, `PERF.md` §3o | R1, R6 |
| a change to the fast path, only if the measurement says so | `operators/join.rs` | R2 |
| the answer unchanged, key for key | the unit test in `operators/join.rs`, and the three named tests | R3 |
| the movement measured with the floor beside it, interleaved | the campaigns, and the instrument | R4 |
| the prize named as the stage's own probe | `PERF.md` §3o | R5 |
| every moved figure published old-and-new, and the documents corrected | `PERF.md`, the README, the ADRs, `scripts/` | R6 |
| the estimate unchanged | `memory.rs` | R7 |
| both answers in the open | the spec | R8 |

**Top-down**, the goal — *the remainder is measured part by part, and the stage that moved is explained* —
reaches those obligations only through four questions: **what is the remainder made of** (R1), **what
changes, and only that** (R2, R3, R7), **what moved the stage** (R4, R5), and **what is published** (R6,
R8).

**And the four differences between the two are the review's findings:**

1. **The bottom-up list had two obligations the top-down goal does not reach.** A candidate that prices one
   thing at a time and a reconciliation of the parts against the remainder — R1 as it is now written, a
   ladder and a sum — were not in the first draft. The first draft listed five candidates and no
   reconciliation, and a list of candidates **cannot** be summed: without the increments between the
   rungs, the parts are five numbers and the remainder is a sixth.
2. **The top-down goal reached an obligation the bottom-up list could not satisfy.** "What moved the stage"
   needs the floor **in the same minutes for the two codes**, and the first draft said the floor and the
   stage had never been measured in the same minutes — which is **false**: the campaigns carry both in the
   same report. The gap is the **gap between the two codes' campaigns**, and the measurement that closes it
   is the interleaved A/B **with** the floor.
3. **The verdict rested on a diagnostic.** The first draft asked for the A/B the way spec 028 ran it — one
   campaign each — and put the spec's answer on it. A diagnostic cannot carry a verdict: the house method
   publishes the median of at least three runs.
4. **The published documents state four things the code and the reports do not support**, and they are
   listed in R6. The shape list is the largest: it names **two costs the fast path does not pay** and
   omits the one it does.

## The requirements

### R1 — The remainder is decomposed into named parts, and each part is measured by a candidate that differs in exactly one thing

**The published figure is the parts, not the difference.**

The instrument gains a **ladder** of candidates over the same rows and the same table, each one adding
**exactly one** thing to the one before it, so that the **increment** between two rungs is the price of that
one thing:

| the rung | what it is | the increment it prices |
|---|---|---|
| **C0 — the plain candidate** | `counts.get(offset).copied().unwrap_or(0)` — the byte, as a `u8` | — |
| **C1 — the chain** | the same read, folded through `map_or` instead of `unwrap_or` | the **`Option` chain** of the byte's read |
| **C2 — the width** | the byte widened to `u32`, the way the engine's is | the **`u32` width** |
| **C3 — the state over `&self`** | the same read, but the table is a struct and the count comes from a method over `&self` | **reaching the state through `self`** |
| **C4 — the map's option** | the same, with the overflow map's `Option` read and branched on | **the map's discriminant** |
| **C5 — the engine's own lookup** | the engine's `count`, over the same rows and the same table, with the instrument's own key source | **everything the engine's lookup has that the candidate does not** |
| **the engine's probe** | the engine's probe over the engine's own key column | **the key source** |

**And the remainder is reconciled against the ladder.** The published sum is:

> the remainder = (the engine's probe − C5) + (C5 − C4) + (C4 − C3) + (C3 − C2) + (C2 − C1) + (C1 − C0)

published with **each increment's spread**, and with what is left of the remainder **unexplained** named, not
rounded away. A part that measures at or below the noise is published as such.

**And the candidates are checked before anything is timed.** Every rung's rows are compared against the
engine's rows, **in order**, and the aggregate's value is checked against the published one.

> **The requirement is rewritten, and the reason recorded.** It was written as a list of five candidates,
> each a thing the engine's lookup has that the plain one does not — the method over `&self`, the `Option`
> chain, the `u32` width, the saturating add, the engine's own lookup. Two of those five are **not on the
> fast path** in the code this spec starts from: the saturating add is behind the early return, and so is
> the map's `get` — the very consult spec 028 priced at **0.020 ms**. A list of five would have measured
> two costs the probe does not pay, and would have left the one it does pay — the map's `Option`
> discriminant — unmeasured. The requirement now names a **ladder** whose rungs differ by exactly one thing,
> so that each **increment** is a price and the sum reconciles against the remainder.

### R2 — The lookup is changed only in what the measurement supports, and not at all if the measurement says nothing

**The change follows the measurement, and the measurement is allowed to say no.**

If the increments say the shape is worth changing, the change is to the direct table's lookup — the fast
path — and to nothing else. If they say the shape is free, **the code does not change**, and that is the
finding and it is published as it is.

**The yardstick is the remainder's own spread, in the set that measures it.** The saving is measured as a
difference of remainders, so the band it has to clear is **the spread of the remainder across the
campaigns of that set**, published with the saving. That is the yardstick spec 028 used for the map's share
— where the candidate's own spread was **0.709 ms** in the old set and **0.168 ms** in the new one, and the
share's own three values ranged **−0.221 ms … 0.406 ms** and **−0.278 ms … 0.071 ms** against them. A
saving inside that band is not a saving, and **each set is judged against its own band**: the six-campaign
range that spec 028 published spans two sets, and a range that spans them is not a yardstick.

**And the change must not make the probe slower.** A change that makes the probe slower is not a change,
and the figure says so.

> **The requirement is rewritten, and the reason recorded.** It said the change is made "if the measurement
> says the shape is worth changing", and named the yardstick as "the spread of the figure it is measured
> against" — which named no figure. The saving is a difference of **remainders**, so the band that matters
> is the **remainder's** spread, and the requirement now says so and requires that spread to be published
> with the saving. The yardstick also has to name **the set it came from**: the figure spec 028 published —
> **0.709 ms** — is the **old** set's candidate spread, and it was used to judge a range that spans both
> sets. The new set's own spread is **0.168 ms**.

### R3 — The answer does not change, and the tests that hold it are named

**The same rows, in the same order, with the same count a key.**

The direct table's answer must be the hash table's answer, bit for bit, for keys that overflow and keys
that do not. `DirectTable` and `HashedTable` are private to `operators/join.rs`, so the comparison is a
**unit test inside that module**, and that test **exists**: a `DirectTable` over a key that carries 300
build rows and a key that carries three, against a `HashedTable` over the same keys, compared key by key.
This spec changes the lookup the test exercises, so the test must keep holding it — and it must be **read
again after the change**, not only run.

The tests that already hold the behaviour must keep passing, and they are named here so that a stranger
knows what "does not change" means:

| test | what it holds |
|---|---|
| `crates/columnar-engine/tests/join.rs` — *a build key more than 255 rows carry still counts every one* | the overflow map's semantics: a key's count is not lost when its byte fills |
| `crates/columnar-engine/tests/join.rs` — *a probe key below the table's base matches nothing* | the range rule |
| `crates/columnar-engine/tests/join.rs` — *a NULL key never matches on either side* | the NULL rule, which the lookup does not touch |
| `crates/columnar-engine/src/operators/join.rs` — the **unit test** | the direct table's answer against the hash table's, key for key, for a key that overflows and one that does not |

The **hashed** path is untouched: a table that is not direct-addressed never consults a map. The direct
table's **slow path** — a NULL-bearing or dictionary-encoded integer probe column — shares
`DirectTable::count` with the fast path, and therefore changes with it; its **semantics** do not.

### R4 — The movement is measured with the floor and the context beside it, in the same minutes

**The verdict is published in one of three words: the host's, the code's, or unattributed.**

The decisive measurement is the **same-moment A/B with the instrument**, not only with the plan output: the
code this spec replaces and the code it writes, **three campaigns each, interleaved** — old, new, old, new
— in one session, with the join probe's own parts, the **floor** among them, in each campaign. The **cold**
stage and the **scan's** stage are the controls for the code.

**And the instrument is the same in both sets.** Only the engine's own file changes; the instrument is
**byte-identical**, because the control has to be the control. A comparison between two instruments is not
a comparison of two codes.

**And the two candidates are named.** The **floor** is the instrument's own measure of what a memory-bound
loop costs on this host: it is **not** a floor of the code's work and **not** a property of the code at
all, which is exactly what makes it the control for the machine. The **context** — the gap between the
stage's residual and the standalone probe, which is the price of running inside a query rather than in a
quiet process — is the control for the process, and it is measured in both sets.

**And "unattributed" is an allowed answer.** If the movement does not track the floor, and the context does
not account for it, and the parts do not either, that is published with the numbers that show it, and the
next question is written down where this one was.

> **The requirement is rewritten, and the reason recorded.** It asked for "one campaign each, minutes
> apart" — which is the **diagnostic** spec 028 ran, and a diagnostic cannot carry a verdict: the house
> method publishes the median of at least three runs, and the verdict here is the spec's answer. The
> requirement now asks for **three campaigns each, interleaved**, so that a drift in the host's state is
> shared by the two codes rather than attributed to one of them, and it adds the **instrument identity**
> rule, without which the floor would not be the same control in the two sets.
>
> It also said the floor and the stage "have never been measured in the same minutes", which is **not
> true**: the join probe's campaigns carry the floor and the stage in the **same report**. What has never
> been measured is the floor and the stage **in the same minutes for the two codes** — the A/B carries the
> stages and no floor, and the campaigns that carry the floor are minutes apart. The requirement now says
> that, which is the gap the measurement closes.

### R5 — The prize is the stage's own probe, and the standalone probe's remainder is published as a bound

**The figure that reaches the query is the stage's own probe, and the publication says which figure is the
prize.**

The stage's residual is **less** than the standalone probe of the same work, and the difference changes
sign between sets. A win on the standalone probe is therefore published as a **bound** on what the query can
gain — never as what it gained, and never with the two compared as if they were the same measurement.

### R6 — Every figure that moved is published old-and-new, and the documents state the code that ships

**A figure that moved is published with the figure it replaced and the reason it moved.**

The change moves figures in **six** places and not in one:

| where | the figure | where its old value lives |
|---|---|---|
| [`PERF.md`](../../PERF.md) §2b | the cold `join-orders` | §3l's old-and-new table, and §2h's four-column table |
| [`PERF.md`](../../PERF.md) §2a | the resident `join-orders` | the same two tables |
| [`PERF.md`](../../PERF.md) §2h | the query table and the stage table | §2h's own "the set this one replaces" table |
| [`PERF.md`](../../PERF.md) §3l | the old-and-new table | the table's own previous column |
| [`PERF.md`](../../PERF.md) §3o | the stage, the residual, the probe, the ladder, the attribution | the table's own "the old code" column |
| [`README.md`](../../README.md) | the status table's row, the prose, and the next question | the row's previous values |

**And the shape list is corrected where it is published.** The four names — "the method over `&self`, the
`Option` chain, the `u32` width and the saturating add" — are published in [`PERF.md`](../../PERF.md) §3o,
in [ADR-0036](../../docs/adr/0036-the-probes-remainder-taken.md) and in the README, and they name **two
costs the fast path does not pay** and **omit the one it does**. They are corrected to the code that ships,
and the correction says which names are off the fast path and which have never been measured.

**The reports are kept, and the guard follows them.** The repository's convention is that **every superseded
set is preserved under its own name**, so that every figure published old-against-new stays derivable. The
sets this increment supersedes are preserved under the name of the spec that took them, and the new sets
take the current names: the comparison campaigns become `compare-spec028-run*.json`, the join probe's
campaigns become `join-cost-spec028-run*.json`, and this increment's "before" set takes the name
`compare-before-run*.json` and `join-cost-before-run*.json` again — measured back to back with this
increment's set, on the code this spec replaces.

`scripts/check-published-figures.py` and `scripts/medians.py` are extended for the ladder's rungs and the
new sets, and the guard checks every published figure — the old set's and the new set's — against the
reports, **in the section that publishes it**.

**And the resident _query_ figure is not added to the guard, and that is not an oversight.** The engine's
resident path reads the build side **inside** its clock and the incumbents read it before the clock starts,
so the two numbers are not the same measurement: that is spec 020's rule, and the figure is named in the
prose rather than published beside a number measured differently. What moves is the engine's own resident
**stage**, and that one is checked.

**And the documents that state the finding say what the code now does — and what the code was.** The review
found **four** statements published that the code and the reports do not support. Each is corrected, and the
correction says what it was:

| what is published | where | what the code and the reports say |
|---|---|---|
| "the remainder is the lookup's shape: the method over `&self`, the `Option` chain, the `u32` width and the saturating add" | [`PERF.md`](../../PERF.md) §3o, [ADR-0036](../../docs/adr/0036-the-probes-remainder-taken.md), the README's next question, [spec 028](../028-the-probes-remainder/spec.md) | the saturating add and the map's consult are **off the fast path** in the code that ships, and the **map's `Option` discriminant** — which is on it — was never measured |
| "the resident join stage moved **22.2 ms** to **23.6 ms**, reproducibly, in a same-moment A/B" | the README's next question, [ADR-0036](../../docs/adr/0036-the-probes-remainder-taken.md), [ADR-0035](../../docs/adr/0035-the-join-stage-taken-apart.md) | **22.2 ms** and **23.6 ms** are the **comparison campaigns'** medians; the A/B's own figures are **21.7 ms** and **23.2 ms** |
| "the scan's stage, the control, did not move at all" | the README's next question | it moved **0.3 ms** the other way (**70.5 ms** to **70.2 ms**) |
| "the candidate's own spread across three campaigns is **0.709 ms**", used against a share that ranges across **six** campaigns | [`PERF.md`](../../PERF.md) §3o, [ADR-0036](../../docs/adr/0036-the-probes-remainder-taken.md), the README's next question | **0.709 ms** is the **old** set's spread; the **new** set's is **0.168 ms**, and each set's share must be judged against **its own** |

The finding itself — that the map was never the remainder, and that the stage moved — is stated in
[`PERF.md`](../../PERF.md) §3o, in [ADR-0036](../../docs/adr/0036-the-probes-remainder-taken.md) and in the
README's next question. All three are amended by this increment, and this spec's ADR records the decision.

### R7 — The estimate does not move

**The build side's memory estimate is unchanged, and the reason is recorded.**

The estimate is made **before a row is read**, so it cannot know what will overflow: it charges the direct
table's bytes **and** the overflow map's **worst case** — one entry in every 256 of the rows that survive.
This spec adds no state to the table and removes none, so neither the number nor its rule moves, and the
estimate's tests pass unchanged.

### R8 — Both questions are answered in the open

**The spec ends with two paragraphs: what the remainder is made of, and what moved the resident stage.**

The first names the ladder's increments and their measured cost, and says what the change bought or did not
buy. The second says whether the movement was **the host's**, **the code's**, or **unattributed**, and it
says it in those words. A spec that ends with "further investigation is needed" is not an answer, and the
house rule is that a negative result is published as a negative result.

## The scope

**What this spec does not do, and why.**

- **The table's shape is not changed.** One byte a count over the key's range was chosen by measurement in
  spec 021 and beat every candidate in that campaign. This spec changes the **lookup**, not the table.
- **The overflow map's semantics are not changed.** A key with more than 255 build rows still counts exactly,
  and the map is still an `Option` and still **not allocated** when nothing overflows.
- **Both hashers are untouched.** The hashed table uses the engine's own `JoinHasher`; the overflow map is
  std's `HashMap`, whose default hasher is `RandomState` (**SipHash-1-3**).
- **The hashed path is not changed.** A table that is not direct-addressed never consults a map.
- **The build side's read is not changed.** It is Arrow's decode of `orders.parquet` — **11.615 ms** of the
  cold stage's **26.550 ms** — and it has been measured three times and refused every time. Its pruning
  idea is dead for this file: `orders.parquet` has two row groups and **both** carry `o_orderdate` from
  1992-01-02 to 1998-12-01, so neither can be excluded.
- **The scan's stage is the control, and it is not touched.** The scan's elapsed is the reader's own work
  **less** the probe's, so the probe's saving cancels out of it.
- **The resident path's asymmetry stands.** The engine's resident path reads the build side **inside** its
  clock, where the incumbents read it before the clock starts. A resident-stage improvement is therefore
  **not** comparable with the incumbents' resident figures, and this spec does not claim it is.
- **The plan is not changed.** No new plan, no new operator, no new join type.
- **The probe's key source is not changed.** The fast path needs a plain integer column with no NULLs, and
  the general path — dictionary-encoded, or NULL-bearing — is what it is. The NULL rule and the dictionary
  rule stay as they are. The instrument's **own** key source is the instrument's, and it does not become the
  engine's.
- **The engine's public API is not changed.** `KeySource` is already public, so the instrument can implement
  it over a plain slice without the crate giving up anything.
- **The comparison campaigns' own figures are not compared with the instrument's.** They are different
  measurements of the same work, and each is published with the set it came from.
- **No ARM64 figure is compared with an x86-64 one.** Every figure here is ARM64, on this host, with the
  load average the reports carry.

## The instrument

`crates/columnar-engine/examples/join_cost_probe.rs`, committed in the tree by spec 026, extended with the
ladder of R1: the **chain**, the **width**, the **state over `&self`**, the **map's option**, and the
**engine's own lookup over the instrument's key source**. Each rung is the plain candidate plus one thing,
over the same rows and the same table.

The instrument's own key source is a `KeySource` over a plain `&[i64]` — the trait is already public, and
implementing it in the instrument changes nothing the crate publishes.

The engine's **216,783** rows are checked against every rung's, in order, before anything is timed, and the
aggregate's value is checked against the published one.

## The measurement

**The plan, so that a stranger can run it:**

| what | how | where it lands |
|---|---|---|
| the commits | the code this spec writes, and the code it replaces — spec 028's — measured **back to back** with it | the spec's own history |
| the join probe's campaigns | `just probe-join "data/lineitem.parquet" "bench-results/join-cost-run1.json"`, three campaigns, five iterations each | `join-cost-run*.json`; the before-set, on the code this spec replaces and measured back to back with it, is `join-cost-before-run*.json`; the sets spec 028 published are preserved as `join-cost-spec028-run*.json` and `join-cost-spec026-run*.json` |
| the comparison campaigns | `just compare "data/lineitem.parquet" "3" "1" "bench-results/compare-run1.json"`, three campaigns | `compare-run*.json`; the before-set is `compare-before-run*.json`; the sets spec 028 published are preserved as `compare-spec028-run*.json` and `compare-spec026-run*.json` |
| the same-moment A/B **with the instrument** | the two codes, **three campaigns each, interleaved**, minutes apart in one session, with the join probe's parts in both | the measurement R4 turns on — and a set, not a diagnostic, because a verdict rests on it |
| the host | an Apple M4 Max, arm64, one thread, and the load average — the join probe's reports **carry** it, the comparison campaigns' reports do **not**, and for those it is recorded by hand | in every report |
| the guard | `scripts/medians.py` derives the tables; `scripts/check-published-figures.py` checks every published figure against the reports, in the section that publishes it | the CI job "published figures match the evidence" |

**And the order of the campaigns is part of the method.** The two codes are measured **interleaved** — old,
new, old, new — so that a drift in the host's state is shared rather than attributed. The **code is
committed before** the campaign: the reports carry the commit they were taken at, and an uncommitted tree
is not a measurement of anything. Only **the engine's own file** changes between the two codes; the
instrument is byte-identical, so the floor is the same control in both.

**And the figures themselves**, filled from the campaigns: the stage on both paths, the residual the stage
leaves after the build side, the probe on both paths, the **ladder** and the **reconciliation**, the floor,
the candidates, and the comparison campaigns' query times, with every figure that moved published
old-and-new.

## The references

| | |
|---|---|
| [ADR-0028](../../docs/adr/0028-the-hot-paths-taken-apart.md) | the join's probe taken apart the first time, and the instrument that took it apart |
| [ADR-0035](../../docs/adr/0035-the-join-stage-taken-apart.md) | the join's stage, taken apart, and the remainder this spec measures |
| [ADR-0036](../../docs/adr/0036-the-probes-remainder-taken.md) | the probe's remainder, taken: the map was never it, and the movement it left is this spec's second question |
| [spec 020](../020-the-engine-joins-and-sorts/spec.md) | the join and the sort, and the rule that the resident figure is not the incumbents' measurement |
| [spec 021](../021-the-hot-paths-taken-apart/spec.md) | the join's probe measured against its floor, and the table's shape chosen by measurement |
| [spec 026](../026-the-join-stage-taken-apart/spec.md) | the join's stage taken apart, on both paths, and the derivation the stage's residual comes from |
| [spec 028](../028-the-probes-remainder/spec.md) | the probe's remainder, the map's share, and the movement this spec takes up |
| [`PERF.md`](../../PERF.md) §2a, §2b, §2h, §3l, §3o | the published tables this spec's figures are re-derived into |
