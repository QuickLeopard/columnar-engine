# ADR-0036 — The probe's remainder, taken

**Status**: accepted
**Date**: 2026-10-28
**Spec**: [spec 028](../../specs/028-the-probes-remainder/spec.md)
**Predecessor**: [ADR-0035](0035-the-join-stage-taken-apart.md)

## Context

Spec 026 took the join's stage apart, and [ADR-0035](0035-the-join-stage-taken-apart.md) recorded three
findings: the stage is the **build side's read** (**11.83 ms** of the cold stage's **20.74 ms**) and the
**probe** (**10.05 ms**); the build side's read is Arrow's decode and was refused; and **the probe is not
at the floor** — it costs **3.095 ms** more than the same lookup written out by hand over the same six
million rows and the same table.

That third finding was **published as a finding and not taken**, because the table's shape was chosen by
measurement in spec 021 and a different table is a different spec with its own measurement to earn its
place. The map that was named is not the table's shape, though: the engine's per-row lookup consults the
**overflow map** once a probe row, and the map holds **nothing** for this data, because every key's count
fits in its byte.

**And the review found that the prize was an assumption.** Three things about the finding turned out to
be wrong, or unmeasured:

- **The consult is already hash-free.** `hashbrown`'s `get` returns before it hashes when the table is
  empty, so what is paid per probe row is one load of the map's emptiness and one branch — not a hash,
  not a bucket probe. The requirement that said "the lookup must not hash the key" was describing the
  status quo.
- **The remainder was measured on the wrong path.** The **3.095 ms** is the **resident** path's probe over
  the engine's materialised column, and the **cold** path's probe — the one inside the reader's batch
  loop, the one the query actually pays — had **no candidate** to be measured against.
- **The candidate differs from the engine's lookup in more than the map.** The engine's `count` is a
  method over `&self` with an `Option` chain and a saturating add; the candidate is a loop over a local
  slice. So the remainder's **attribution** was unmeasured: how much of the **3.095 ms** is the map, and
  how much is the rest, nobody knew.

**And the prize that reaches the query is smaller than the remainder.** The stage charges its own probe
**8.876 ms** cold and **7.357 ms** resident — less than the standalone probe's **10.051 ms** — so the
figure that moves the cold query is the stage's probe, and the prize in the query is **at most** the
remainder, and probably less.

## The decision

### 1. The measurement comes first

The instrument gains **two** parts, and they exist because the review found the old ones could not
answer the question:

- the **candidate with the overflow consult** — the same hand-written lookup, over the same rows and the
  same table, with the map's consult in it. The map's cost becomes a **measured** number.
- the **cold candidate** — the same hand-written lookup inside the reader's batch loop, so that the cold
  path's remainder is measurable at all.

The attribution is published as **the map's share and the rest**, not as one number, and **if the probe
does not get faster, that is the finding** and it is published as it is.

### 2. The table carries whether its overflow map holds anything

The map fills **during** the build — a key whose byte fills writes to it — so the fact is captured
**after** the build loop, and a flag set on the fill path alone would leave it set for every table. When
nothing overflows, the map is **not allocated at all**.

The lookup does not **call** the map when the map holds nothing: no `get`, and no hash of the key. The map
is an `Option`, and the lookup reads the option and nothing else — so it does not read a field of the map
either, and there is no map to read: when nothing overflows, **the map is not allocated at all**. The map's
key and value types do not change, its **hasher** does not change, and its **semantics** do not change: a key
with more than 255 build rows still counts exactly.

> **The wording is amended, and the reason recorded.** It said "and no read of any field of the map", which
> the review rejected: reading the map's **emptiness** *is* reading the map's own state, so the clause
> forbade the natural implementation. What is wanted is narrower and falsifiable — the lookup does not
> **call** the map at all — and the `Option` satisfies it, because there is no map to call.

**And the hasher is named, because there are two.** The **hashed** table uses the engine's own
`JoinHasher`, a hand-written hash chosen by measurement in spec 021. The **overflow map** is a plain
`std::collections::HashMap<i64, u32>`, and std's default hasher is `RandomState`, which is
**SipHash-1-3**. Neither is changed here, and the residual cost of that hash — one SipHash a probe row,
in the case where the map is **not** empty — is on the record rather than left implicit.

### 3. The answer does not change, and the test that holds it is named

`DirectTable` and `HashedTable` are private to `operators/join.rs`, and the public API picks the table
from the file's own statistics, so the same keys cannot be built both ways from outside. The comparison
is therefore a **unit test inside that module**: a `DirectTable` over a key that carries 300 build rows
and a key that carries three, against a `HashedTable` over the same keys, compared key by key.

The tests that already hold the behaviour are named in the spec and keep passing: the overflow map's
semantics, the range rule — which is also the test that catches a fact captured before the build loop —
and the NULL rule.

### 4. The estimate does not move

The estimate is made **before a row is read**, so it cannot know whether anything will overflow: it
charges the direct table's bytes **and** the map's **worst case**, one entry in every 256 of the rows
that survive. The flag is one byte in a struct, and the map's worst case is already charged, so neither
the number nor its rule moves.

### 5. Every figure that moved is published old-and-new, in every place it is published

The change moves figures in **five** places and not in one: `PERF.md` §2a, §2b, §2h, §3l and §3o, and
the README's status table, its prose and its next question. Each of them is published with the figure it
replaced and the reason it moved, and each table carries the old value it replaces — §2a and §2b have no
old-and-new column of their own, so their old values live in §3l and in §2h's four-column table.

**The guard grows.** The repository's convention is that every superseded set is preserved under its own
name, so the comparison campaigns and the join probe's campaigns this increment supersedes are preserved
as `compare-spec026-run*.json` and `join-cost-spec026-run*.json`, the new sets take the current names,
and `scripts/medians.py` and `scripts/check-published-figures.py` follow all three. The **resident**
`join-orders` figures are added to the guard, which today checks the cold one only.

**And this ADR is the successor to ADR-0035's "published, not taken".** The finding is now taken, and the
documents that state it — `PERF.md` §3o, ADR-0035 and the README — say what the code now does.

## Consequences

**And the measurement says what the prize was: nothing.** The candidate **with** the map's consult in
it costs **7.106 ms** against the candidate without it at **7.157 ms** — the map's share is
**0.020 ms** of a **3.337 ms** remainder. In the code it replaces the share was **0.183 ms**. Across
the six campaigns the share ranges from **−0.278 ms** to **0.406 ms**, and the candidate's own spread
across three campaigns is **0.709 ms**: the share is **at or below the noise**. **The map was never the
remainder.** What is left is the lookup's **shape** — the method over `&self`, the `Option` chain, the
`u32` width and the saturating add — and none of it is the map's consult.

**The change is taken, and it is a simplification rather than a win.** The table's overflow map is an
`Option` now: when nothing overflows, **the map is not allocated at all**, and the lookup does not call
it. The semantics do not change, the estimate does not move, and the probe's own measurement is
**0.950 ms** faster — but the **floor**, which this change cannot touch, moved **1.514 ms** slower in
the same two sessions, and the **resident** join stage moved **23.6 ms** against **22.2 ms** —
reproducibly, in a same-moment A/B — while the **cold** stage (**26.0 ms** to **26.4 ms**) and the
**scan's** stage, the control, did not move. **The parts cannot account for the resident movement, and
this ADR does not claim what it is.** It is the next question.

**The prize is a measurement, not a promise.** At the time of the decision the cold `join-orders` was
**96.7 ms** against DuckDB's **95.0 ms** — level, **1.7 ms** apart, inside both spreads — and the
remainder was **3.095 ms** of the probe. The prize in the query is **at most** the remainder, and the figure that moves it is the stage's
own probe, which is smaller than the standalone one. If the map's consult is not the remainder's bulk,
the finding says so, and the next question is somewhere else.

**The scan's stage is the control.** The scan's elapsed is the reader's own work **less** the probe's, so
the probe's saving cancels out of it: the scan's stage is what says the decode did not move.

**The resident path's asymmetry stands.** The engine's resident path reads the build side **inside** its
clock, where the incumbents read it before the clock starts. A resident-stage improvement is therefore
**not** comparable with the incumbents' resident figures, and this ADR does not claim it is.

**What this does not do.** The table's shape — one byte a count over the key's own range — is unchanged,
as it was in spec 021. Both hashers are unchanged. The hashed path is unchanged. The build side's
estimate is unchanged. The build side's read is unchanged, and the pruning that would have made it
cheaper is **dead for this file**: `orders.parquet` has two row groups and **both** carry `o_orderdate`
from 1992-01-02 to 1998-12-01, so neither can be excluded. The reader's pruning is a different
measurement, on a different file.

## References

| principle | how this ADR serves it |
|---|---|
| I. One Predicate, Six Implementations, One Answer | The direct table's answer is the hash table's answer, key by key, for a key that overflows and one that does not, and the test lives where the two tables are both reachable. |
| III. NULL Is an Absent Bit, and It Never Matches | The NULL rule is named among the tests that keep passing, and the direct table's slow path — the one a NULL-bearing column takes — is named as sharing the lookup. |
| IV. Evidence Over Claims | The prize is **unmeasured** until the instrument says so, and the attribution is published as **the map's share and the rest**. A requirement that was already true of the code was rewritten, because a requirement that cannot fail is not a requirement. |
| V. The Comparison Is Reproducible by a Stranger | The commits, the commands, the report names, the iteration count and the host are written down in the spec, and the superseded campaigns are preserved under their own names so that every old-against-new figure stays derivable. |
| VII. Explicit Scope, No Silent Deferral | The table's shape, both hashers, the hashed path, the estimate, the scan's decode and the build side's read are named as out of scope, with the reason for each. |
