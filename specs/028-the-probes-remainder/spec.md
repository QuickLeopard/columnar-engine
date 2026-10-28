# Spec 028 — The probe's remainder

**Status**: in progress
**Started**: 2026-10-28
**Amended**: 2026-10-28 — the bottom-up and top-down review's findings, with the reasons recorded below.
**Predecessor**: [026-the-join-stage-taken-apart](../026-the-join-stage-taken-apart/spec.md)

## The problem

Spec 026 took the join's stage apart, and the third of its three findings is this spec's question.

The stage is the **build side's read** — **11.83 ms** of the cold stage's **20.74 ms**, more than half of
it — and the **probe**, **10.05 ms** over six million rows. The build side's read is Arrow's decode of
`orders.parquet`: measured three times, and **refused every time**.

**The probe is not at the floor.** The engine's probe over its own column costs **10.051 ms**; the same
lookup, written out by hand over the same six million rows and the same table, costs **6.959 ms**. The
difference is **3.095 ms** — the probe's remainder. The probe's own **10.051 ms** is three pieces: the
**floor**, **4.418 ms** (the table's own load, one a probe row); the **output rows**, **2.541 ms** (the
candidate over the floor: what the 216,783 rows the join produces cost); and the **remainder**, the
**3.095 ms** the candidate does not pay.

**And the code names one thing the engine's per-row lookup does that the candidate's does not** — though
it is not the only difference between them, and the measurement has never said how much of the remainder
it is. The candidate reads the table's byte and stops:

```rust
let count = counts.get(offset).copied().unwrap_or(0);
```

The engine's `DirectTable::count` reads the same byte **and consults the overflow map**:

```rust
let byte = self.counts.get(index).copied().map_or(0, u32::from);
let extra = self.overflow.get(&key).copied().unwrap_or(0);
byte.saturating_add(extra)
```

The map exists for one case: a key with more than **255** build rows does not fit in its byte, so the byte
holds 255 and the map holds the rest. The map is consulted on **every** probe row — six million times over
the probe side — whether it holds anything or not, and for this data it holds **nothing**: every key's
count fits in its byte.

**And the prize is unmeasured.** Three things are known and three are not, and the difference matters:

- **Known**: the engine's probe over its column is **3.095 ms** more than the same lookup written by hand,
  over the same six million rows and the same table.
- **Known**: the consult is already **hash-free**. `hashbrown`'s `get` returns before it hashes when the
  table is empty, so what is paid per probe row is one load of the map's emptiness and one branch — not a
  hash, not a bucket probe.
- **Known**: the remainder was measured on the **resident** path's probe over the engine's materialised
  column. The **cold** path's probe — the one inside the reader's batch loop, the one the query actually
  pays — has **no candidate** to be measured against, so the cold path's remainder cannot be measured with
  the committed instrument at all.
- **Not known**: how much of the **3.095 ms** is the map's consult. The candidate and the engine's lookup
  differ in more than the map: the engine's is a method over `&self` with an `Option` chain and a
  saturating add, and the candidate is a loop over a local slice.
- **Not known**: how much of the remainder reaches the **query**. The stage charges its own probe
  **8.876 ms** cold and **7.357 ms** resident — less than the standalone probe's **10.051 ms** — so the
  figure that moves the cold query is the stage's probe, and the prize in the query is **at most** the
  remainder, and probably less.
- **Not known**: whether removing the consult makes the probe **faster** at all. A flag read through `&self`
  in the same loop can cost as much as the emptiness check it replaces.

**So the first job of this spec is a measurement, and the change comes after it.** The engine's cold
`join-orders` is **96.7 ms** against DuckDB's **95.0 ms** — level, **1.7 ms** apart, inside both spreads —
so the gap is real and the prize is worth measuring. It is not **3.095 ms**, and this spec does not claim
it is.

## The requirements

### R1 — The table carries whether its overflow map holds anything, and the lookup does not touch the map when it does not

**The lookup must not call the map at all when the map is empty — no `get`, no hash of the key, and no
read of any field of the map.**

The fact is carried by the table, as the table's own state, and it is captured **after** the build loop:
the map fills *during* the build, so the fact is knowable only afterwards. Capturing it before the loop
makes every overflowing key count 255, and the test that holds that is named in R2.

The map's **key and value types** do not change, its **hasher** does not change (R6), and its **semantics**
do not change (R2). When nothing overflows, the map is **not allocated at all**.

> **The requirement is rewritten, and the reason recorded.** It was written as "the lookup, when the map is
> empty, must not **hash** the key, must not **probe** a bucket, and must not read the map's own state".
> The first two clauses were **already true** of the code as it stood — `hashbrown`'s `get` returns before
> it hashes when the table is empty — so they could not fail, and they described the status quo rather
> than the change. The third clause was the only one that named a change, and it forbade the natural
> implementation: reading the map's emptiness **is** reading the map's own state. What is wanted is
> narrower and falsifiable: **the lookup does not call the map at all** when the map holds nothing.

### R2 — The answer does not change, and the tests that hold it are named

**The same rows, in the same order, with the same count a key.**

The direct table's answer must be the hash table's answer, bit for bit, for keys that overflow and keys
that do not. `DirectTable` and `HashedTable` are private to `operators/join.rs`, so the comparison is a
**unit test inside that module**: a `DirectTable` over a key that carries 300 build rows and a key that
carries three, against a `HashedTable` over the same keys, compared key by key.

The tests that already hold the behaviour must keep passing, and they are named here so that a stranger
knows what "does not change" means:

| test | what it holds |
|---|---|
| `crates/columnar-engine/tests/join.rs` — *a build key more than 255 rows carry still counts every one* | the overflow map's semantics: a key's count is not lost when its byte fills — and the fact captured **after** the build loop, because a fact captured before it would count the key 255 times |
| `crates/columnar-engine/tests/join.rs` — *a probe key below the table's base matches nothing* | the range rule |
| `crates/columnar-engine/tests/join.rs` — *a NULL key never matches on either side* | the NULL rule, which the map does not touch |

The **hashed** path is untouched: a table that is not direct-addressed never consults a map. The direct
table's **slow path** — a NULL-bearing or dictionary-encoded integer probe column — shares
`DirectTable::count` with the fast path, and therefore changes with it; its **semantics** do not.

> **The requirement is rewritten, and the reason recorded.** It was written as "a test holds the two to
> that", as if the test were a matter of taste and of the public API. It is neither: the two tables are
> private, the public API picks the table from the file's own statistics and cannot build the same keys
> both ways, and the test that exists today compares against a brute-force loop rather than against the
> hash table. The requirement now says **where** the test lives and **what** it compares.

### R3 — The remainder is measured, part by part, and the map's share is separated from the rest

**The published figure is the one the measurement gives, not the one the difference suggests.**

The instrument measures, on the same host, back to back against the code it replaces, **before and after**:

- the engine's probe over its **column** — the resident path's probe;
- the engine's probe in the **batches** — the cold path's probe, inside the reader's own batch loop;
- the engine's own **stage** on both paths, taken from the plan output;
- the **build side's parts**, so that the stage's own probe can be derived: the stage, less the build
  side's whole and the open;
- the **candidate**: the same lookup, written out by hand, over the same rows and the same table;
- the **candidate with the overflow consult**: the same lookup, written out by hand, **with** the map's
  consult — so that the map's cost is measured rather than assumed;
- the **cold candidate**: the same hand-written lookup inside the reader's batch loop, so that the cold
  path's remainder is measurable at all;
- the **floor**: the table's own load, one a probe row.

and publishes **what is left of the remainder**, with the map's share **separated from the rest**. If the
remainder does not shrink to nothing, the figure says so and the rest is **named**, not rounded away.

**And the probe must get faster.** If the change does not make the probe faster, that is the finding, and
it is published as it is.

> **The requirement is rewritten, and the reason recorded.** It was written as a list of three parts — the
> probe over the column, the candidate, the floor — and every one of them belongs to the **resident**
> path. The two figures the prize rests on — the **cold** query's time and the **cold** stage — were not
> in the list at all, and the instrument had **no part** that would say what the map costs: the
> remainder was a difference between two loops that differ in more than the map. Without the third
> candidate, R3 would publish an unattributed number. The requirement now names the parts, and the
> **no-regression** clause is added, because nothing in the old requirement forbade a change that made
> the probe slower.

### R4 — Every figure that moved is published old-and-new, in every place it is published

**A figure that moved is published with the figure it replaced and the reason it moved.**

The change moves figures in **five** places and not in one, and the old value of each is published in a
different table from the new one:

| where | the figure | where its old value lives |
|---|---|---|
| [`PERF.md`](../../PERF.md) §2b | the cold `join-orders` | §3l's old-and-new table, and §2h's four-column table |
| [`PERF.md`](../../PERF.md) §2a | the resident `join-orders` | the same two tables |
| [`PERF.md`](../../PERF.md) §2h | the query table and the stage table | §2h's own "the set this one replaces" table |
| [`PERF.md`](../../PERF.md) §3l | the old-and-new table | the table's own previous column |
| [`PERF.md`](../../PERF.md) §3o | the stage, the probe, the remainder | the table's own "the old code" column |
| [`README.md`](../../README.md) | the status table's row, the prose, and the next question | the row's previous values |

**The figures the guard does not yet check are checked now.** The guard checks the cold path's stages
and §3o's own resident stage, but **not** the published comparison campaigns' **resident** join stage —
**21.3 ms** — which this change moves. It is added to the guard, with the set it is measured from.

**And the resident _query_ figure is not added to the guard, and that is not an oversight.** The
engine's resident path reads the build side **inside** its clock and the incumbents read it before the
clock starts, so the two numbers are not the same measurement: that is spec 020's rule, and the figure is
named in the prose rather than published beside a number measured differently. What moves is the
engine's own resident **stage**, and that one is checked.

**The reports are kept, and the guard follows them.** The repository's convention is that **every
superseded set is preserved under its own name**, so that every figure published old-against-new stays
derivable. The sets this increment supersedes — the comparison campaigns and the join probe's campaigns —
are preserved under the name of the spec that took them, and the new sets take the current names:

| what | the name it takes now | the name it is preserved under |
|---|---|---|
| the comparison campaigns | `compare-run*.json` | `compare-spec026-run*.json` |
| the comparison campaigns, on the code spec 026 replaced | `compare-before-run*.json` | `compare-spec025-run*.json` |
| the join probe's campaigns, on spec 026's code | `join-cost-run*.json` | `join-cost-spec026-run*.json` |
| the join probe's campaigns, on the code before spec 026's ten fixes | `join-cost-before-run*.json` | `join-cost-spec025-run*.json` |
| the join probe's campaigns, on spec 026's code, with this increment's instrument | `join-cost-before-run*.json` | — |

**§3o's "old code" column becomes the code this spec replaces** — spec 026's code — measured **back to
back** with this spec's set and with the same instrument, and its "new code" column is this spec's set.

> **The requirement is amended, and the reason recorded.** The table named **three** sets where there are
> **five**, because it listed only the sets the increment supersedes and not the sets those supersede in
> turn. The repository's rule is that **every** superseded set is preserved under the name of the spec whose
> code it measured: spec 026's campaigns superseded two sets of their own — the code spec 026 replaced — and
> those are now `compare-spec025-run*.json` and `join-cost-spec025-run*.json`. The comparison campaigns' row
> for them was missing altogether.
>
> The **last row** is the new one, and it is the reason the third row's name is free: this spec measures the
> code it replaces **back to back** with the code it writes, with the same instrument, so its "before" set is
> a new measurement of spec 026's code — and the name it takes is the one the row above frees. Two sets
> therefore measure spec 026's code: the one spec 026 took, with the instrument of that day, and this spec's,
> with the instrument as it stands. The section says which is which, and the difference between them is the
> host's.

`scripts/check-published-figures.py` and `scripts/medians.py` are extended for the third set, and the
guard checks every published figure — the old set's and the new set's — against the reports, **in the
section that publishes it**.

**And the documents that state the finding say what the code now does.** The finding — that the map is
consulted once a probe row — is stated in [`PERF.md`](../../PERF.md) §3o, in
[ADR-0035](../../docs/adr/0035-the-join-stage-taken-apart.md) and in the README's next question. All
three are amended by this increment, and the successor ADR says so.

> **The requirement is rewritten, and the reason recorded.** It was written for **one** publication site —
> §3o — and the change moves figures published in **five** others, among them the resident
> `join-orders`, which the guard does not check today. It named two report sets where this increment
> needs **three**, because the set the change supersedes is kept under its own name, and it did not list
> the ADR that states the finding. The requirement now enumerates the sites, says which table carries
> each old value, names the sets and the globs, and puts the **resident** figures into the guard.

### R5 — The estimate does not move

**The build side's memory estimate is unchanged, and the reason is recorded.**

The estimate is made **before a row is read**, so it cannot know whether anything will overflow: it
charges the direct table's bytes **and** the overflow map's **worst case** — one entry in every 256 of the
rows that survive. The flag this spec adds is one byte in a struct, and the map's worst case is already
charged, so neither the number nor its rule moves, and the estimate's tests pass unchanged.

### R6 — The map's hasher is named

**The overflow map is hashed with std's default hasher, not with the engine's.**

The **hashed** table uses the engine's own `JoinHasher` — a hand-written hash chosen by measurement in
spec 021. The **overflow map** is a plain `std::collections::HashMap<i64, u32>`, and std's default hasher
is `RandomState`, which is **SipHash-1-3**. This spec changes neither, and the residual cost of that
hash — one SipHash a probe row, in the case where the map is **not** empty — is recorded rather than left
implicit.

### R7 — The fact is captured after the build loop

**The emptiness is a fact about the finished table, not about the table being built.**

The map fills **during** the build: a key whose byte fills writes to it. The fact must therefore be
captured **after** the loop, and a flag set on the fill path alone would leave it set for every table. The
test that holds this is named in R2.

## The scope

**What this spec does not do, and why.**

- **The table's shape is not changed.** One byte a count over the key's range was chosen by measurement in
  spec 021 and beat every candidate in that campaign. The struct gains the fact R1 carries, and the map
  is **not allocated** when nothing overflows; the byte a key, and the key's own range as the index, stay
  as they are.
- **The overflow map's semantics are not changed.** A key with more than 255 build rows still counts
  exactly, and the map still holds what the byte cannot.
- **Both hashers are untouched.** The hashed table's `JoinHasher` and the overflow map's `RandomState`
  (SipHash-1-3) are named in R6 and neither is changed.
- **The hashed path is not changed.** A table that is not direct-addressed never consults a map.
- **The scan's stage must not move, and it is published as the control.** The scan's elapsed is the
  reader's own work **less** the probe's, so the probe's saving cancels out of it: the scan's stage is
  what says the decode did not move.
- **The build side's read is not changed.** It is **11.83 ms** of the stage, and it is Arrow's decode of
  `orders.parquet`. A row group whose statistics exclude the date predicate could be skipped, and the
  engine's estimate already uses those statistics — but **`orders.parquet` has two row groups, and both of
  them carry `o_orderdate` from 1992-01-02 to 1998-12-01**. Neither can be excluded, so the build side
  reads every row of the file and there is nothing to skip. That is the file's own statistics saying so,
  and it is a fact about **this** file: a file with narrower groups would be a different measurement, and
  the reader's pruning is not this spec's business.
- **The build side's estimate is not changed.** R5 says why.
- **The resident path's asymmetry is not changed.** The engine's resident path reads the build side
  **inside** its clock, where the incumbents read it before the clock starts. A resident-stage
  improvement is therefore **not** comparable with the incumbents' resident figures, and this spec does
  not claim it is.
- **The plan is not changed.** No new plan, no new operator, no new join type. The join stage's
  `elapsed` moves, and it is a published plan-output number.
- **The probe's hash table is not changed, and neither is the general path's semantics.** Strings,
  dictionaries and columns with NULLs keep their answers.

## The instrument

`crates/columnar-engine/examples/join_cost_probe.rs`, committed in the tree by spec 026, extended with:

- the **candidate with the overflow consult** — the same hand-written lookup, over the same rows and the
  same table, with the map's consult in it, so that the map's cost is a **measured** number;
- the **cold candidate** — the same hand-written lookup inside the reader's batch loop, so that the cold
  path's remainder is measurable at all.

The engine's **216,783** rows are checked against the candidate's, in order, before anything is timed, and
the aggregate's value is checked against the published one.

## The measurement

**The plan, so that a stranger can run it:**

| what | how | where it lands |
|---|---|---|
| the commits | the code this spec writes — `725c091` — and the code it replaces — `be154bb`'s engine, which is the code spec 026 shipped, unchanged since `d75c0e8` | the spec's own history |
| the join probe's campaigns | `just probe-join "data/lineitem.parquet" "bench-results/join-cost-run1.json"`, three campaigns, five iterations each | `join-cost-run*.json`; the before-set, on the code this spec replaces and measured back to back with it, is `join-cost-before-run*.json`; the sets spec 026 published are preserved as `join-cost-spec026-run*.json` and `join-cost-spec025-run*.json` |
| the comparison campaigns | `just compare "data/lineitem.parquet" "3" "1" "bench-results/compare-run1.json"`, three campaigns | `compare-run*.json`; the before-set, on the code this spec replaces and measured back to back with it, is `compare-before-run*.json`; the sets spec 026 published are preserved as `compare-spec026-run*.json` and `compare-spec025-run*.json` |
| the same-moment A/B | the two codes, one campaign each, minutes apart on the same host | `compare-ab-old-run1.json` and `compare-ab-new-run1.json` — a diagnostic, not a set |
| the host | an Apple M4 Max, arm64, one thread, and the load average the reports carry | in every report |
| the guard | `scripts/medians.py` derives the tables; `scripts/check-published-figures.py` checks every published figure against the reports, in the section that publishes it | the CI job "published figures match the evidence" |

**And the figures themselves**, filled from the campaigns: the stage on both paths, the probe on both
paths, the remainder's **attribution** — the map's share and the rest — the floor, the candidates, and
the comparison campaigns' query times, with every figure that moved published old-and-new.

### What the campaigns said

The campaigns were taken in one session on this host, at a load average of **8.09–8.81** for the join
probe's and **7.58–13.77** for the comparison's. The code this spec replaces is `be154bb`'s — the
instrument's commit, whose engine is the code spec 026 shipped — and the code it writes is `725c091`.

**The map's share is at or below the noise, and the map was never the remainder.** The candidate
**with** the map's consult in it costs **7.106 ms** against the candidate without it at **7.157 ms**:
the share is **0.020 ms** of a **3.337 ms** remainder. In the code it replaces the share was
**0.183 ms**. Across the six campaigns the share ranges from **−0.278 ms** to **0.406 ms**, and the
candidate's own spread across three campaigns is **0.709 ms** — so the share is at or below the noise.
**The remainder is the lookup's shape**: the method over `&self`, the `Option` chain, the `u32` width
and the saturating add, none of which the hand-written candidate has.

**The change is taken, and it bought nothing the query can see.** The probe's own measurement is
**0.950 ms** faster (the two sets' ranges do not overlap), but the **floor** — the table's own load,
which this change cannot touch — moved **1.514 ms** slower in the same two sessions, and the
**output rows'** cost moved with it. The **stage** is the engine's own account and the authority: the
**cold** stage moved **26.0 ms** to **26.4 ms**, inside both spreads, and the **scan's** stage, the
control, did not move at all (**70.5 ms** to **70.2 ms**). The **resident** stage moved **22.2 ms** to
**23.6 ms**, and a **same-moment A/B** — one campaign each, the old code and the new, minutes apart on
the same host, committed as `bench-results/compare-ab-old-run1.json` and
`bench-results/compare-ab-new-run1.json` — reads the old code's resident stage at **21.7 ms** and the
new code's at **23.2 ms**. So the movement is real and it is not the machine's drift, and **the parts
cannot account for it**: the probe's own measurement is faster and the build side's read is **0.275 ms**
faster. **This spec does not claim what the resident movement is**; it is published as it is, and it is
the next question.

**The estimate did not move, and the semantics did not change.** The unit test in
`crates/columnar-engine/src/operators/join.rs` holds both: the direct table's answer is the hash
table's answer, key for key, and the overflow map is `None` when nothing overflowed and `Some` when a
key did — and that key counts **300** times, not 255.

> **The requirement is met, and what it cost is recorded.** R1 asked for the lookup not to call the map
> when the map holds nothing, and it does not: the map is an `Option`, it is **not allocated at all**
> when nothing overflows, and the lookup reads the option and nothing else. R3 asked for the probe to
> get faster, and says that if it does not, that is the finding and it is published as it is. The
> probe's own measurement did get faster — **0.950 ms** — and the query did not, because the parts are
> bounds and the stage is the authority. **The map's share of the remainder was the prize, and the
> prize was not there.**

## The references

| | |
|---|---|
| [ADR-0021](../../docs/adr/0021-the-materialised-column-and-a-rust-peer.md) | the peer the engine is compared against, and how the two are held to the same conditions |
| [ADR-0028](../../docs/adr/0028-the-hot-paths-taken-apart.md) | the join's probe taken apart the first time, and the table's shape chosen by measurement |
| [ADR-0030](../../docs/adr/0030-the-scan-that-feeds-the-join.md) | the scan that feeds the join, and the rule that the rows are read where they lie |
| [ADR-0035](../../docs/adr/0035-the-join-stage-taken-apart.md) | the join's stage, taken apart, and the remainder this spec takes up |
| [ADR-0036](../../docs/adr/0036-the-probes-remainder-taken.md) | this spec's decision, and the successor to ADR-0035's "published, not taken" |
| [spec 021](../021-the-hot-paths-taken-apart/spec.md) | the join's probe measured against its floor, and the table's shape chosen by measurement |
| [spec 026](../026-the-join-stage-taken-apart/spec.md) | the join's stage taken apart, and the **3.095 ms** remainder this spec measures |
| [`PERF.md`](../../PERF.md) §2a, §2b, §2h, §3l, §3o | the published tables this spec's figures are re-derived into |
