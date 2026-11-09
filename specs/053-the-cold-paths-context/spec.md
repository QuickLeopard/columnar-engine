# Spec 053 — The cold path's context

**Status**: planned
**Started**: 2026-11-09
**Predecessor**: [spec 052 — the reader does not own the decode](../052-the-reader-owns-the-decode/spec.md)

## The problem

**The context is the engine's own account of itself, minus the same work measured on its own.** The join stage reports its
**parts** in the plan output, so the in-query cost is the engine's account of itself and not an inference; the standalone
probe measures the same work in a quiet process; and the **context** is the difference between the two.

**On the cold path the context is 0.152 ms, and the verdict is unattributed.** The gap is inside the path's yardstick of
**0.514 ms**, with its own spread of **0.787 ms** over it ([§3q](../../PERF.md#3q-the-drifts-own-parts),
[§3r](../../PERF.md#3r-the-file-state-floor)). The resident path's verdict is unattributed too, and the house has said so
since spec 030.

**And the instrument that would attribute it is in place.** Spec 036 built the **file-state floor** — the build side's read
through the engine's own reader, measured **in the iteration**, right after the machine's own measure, so the two floors
are back to back and the file's state is a measured series rather than an argument. §3r publishes the ladder for **both**
paths with the file-aware yardstick.

**And the gap's own two sides are still far apart.** This is the same defect spec 036 found in the floors, one
level up:

- the **query's account** — the engine's own account of its join stage, in the query's process;
- the **standalone's account** — the probe measuring the same work in a quiet process.

They are in the same iteration, but the report's own **separation** says how far apart they were taken: **266.705 ms**
on the cold path and **318.993 ms** on the resident one, where the two floors are **8.019 ms** apart. **So the file's
state and the machine's movement over those hundreds of milliseconds are not common to both sides, and they do not
cancel in the difference.** The floors were moved into the iteration, back to back; **the gap's own sides were not.**

**And that is what leaves the question open.** The file's state is the obvious candidate — the cold path reads the file
and the resident path does not, and the file's state moves more on a cold read than on a warm one. **But a candidate that
is measured hundreds of milliseconds away from the thing it is supposed to explain cannot be the answer**, whatever the
day's numbers say. Spec 036's own lesson, published: *a measurement that says "no" is worth more than the increment it
refuses* — **and a measurement taken far from the thing is not a measurement of the thing at all.**

## The requirements

### R1 — The context's **two sides** are measured **back to back**

- [ ] The **query's account** and the **standalone's account** of the same work are taken **back to back**, with the
      **order alternating** between the iterations — the way the floors are taken now.
- [ ] **And each path's account carries its own build side's work**, so that the two sides of every leaf of that path's
      ladder are one account apart rather than one campaign apart.
- [ ] **And the separation is published** — how far apart the two sides of each difference were taken — and it is
      **compared with the old separation**, so the reader can see the pairing move.

### R2 — The **verdict** is given, on the cold path, with the **file-aware yardstick**

- [ ] The cold path's context is published against the **file-aware yardstick** — the larger of the machine's own drift
      and the file-state floor's own drift — beside the yardstick the old verdict used.
- [ ] **And the file state's share of the context** is published: what is left of the context once the file's state is
      taken out.
- [ ] **And the resident path's verdict is published beside it**, so the two paths are judged the same way.

### R3 — The answer is published **either way**

- [ ] If the file state is **the whole of it**, the cold path's context is **attributed** to it, and the open question is
      **closed**.
- [ ] And if it is **not**, the question closes as **unattributable at this spread** — with the measurement published,
      the yardstick beside it, and the reason named.
- [ ] **And the old figures are published beside the new ones**, with the reason: **the pairing moved**, and the old
      pairing's defect is named.

### R4 — The **published sentences** that rest on the old pairing are **corrected where they were published**

- [ ] [§3q](../../PERF.md#3q-the-drifts-own-parts) and [§3r](../../PERF.md#3r-the-file-state-floor)'s account of the
      context's verdict, and [STATUS.md](../../STATUS.md)'s question 1.
- [ ] And the correction is **recorded in the ADR**, as the house records every correction for an in-flight increment.

### R5 — The answer is published

- [ ] A **new section** in `PERF.md`, the decision in an **ADR**, the status's question rewritten, and the raw reports
      **committed**.
- [ ] And the **guard** checks the published figures **inside the section that publishes them**.

## The scope

- **In**: the cold path's **context**, its two sides, the pairing, and the verdict.
- **Out**: **the file-state floor's own instrument** — it is in place, and it is not what this increment changes.
- **Out**: **the wide shape's plain string's copy** — that is the wide shape's gap, and the fix wants its own
  measurement.
- **Out**: the crates.io publish. The crates stay unpublished while this work lands, so a break that the work implies
  costs nothing.

## The instrument

**`crates/columnar-engine/examples/join_cost_probe.rs`** — the join's stage, taken apart: the stage on both paths, the
parts, the two floors in the iteration, and the ladder. **Its blocks are the instrument's**, and **this increment moves
the context's two sides out of theirs and into the iteration.**

**And `crates/columnar-engine/examples/join_parts_probe.rs`**'s campaign — the query's own account of its parts — is the
side that moves.

## The references

- [PERF.md §3q](../../PERF.md#3q-the-drifts-own-parts) — the drift's own parts, and the yardstick every part is judged
  against.
- [PERF.md §3r](../../PERF.md#3r-the-file-state-floor) — the file-state floor, and the pairing that is published now.
- [PERF.md §3p](../../PERF.md#3p-the-join-s-stage-measured-again) — the join's stage, measured again.
- [ADR-0042](../../docs/adr/0042-the-file-state-floor.md) — the floor, and the lesson: *the sides were measured in the same
  iteration, back to back, with the order alternating. **The floor was not***.
- [ADR-0038](../../docs/adr/0038-the-context-taken-apart.md) — the context, taken apart, and the first verdict.
- [constitution.md](../../constitution.md), principle **IV — Evidence Over Claims**: a number is a claim about a
  measurement, and a claim without its evidence is not published.
- [constitution.md](../../constitution.md), principle **VII — Explicit Scope, No Silent Deferral**: the wide shape's
  string copy and the crates.io publish are **named** as out of scope, rather than left as omissions.

## The amendments

### A1 — The **gap's two sides** move into one account, not the floors

**Why**: spec 036 moved the **floors** into the iteration and corrected the sentence that had claimed they were already
there. **The gap's own sides were left where they were** — the query's account of the stage and the standalone's account
of the same work — and the two are **266.705 ms apart** on the cold path and **318.993 ms** on the resident one, where the
two floors are **8.019 ms** apart. **So the file's state and the machine's movement over those hundreds of milliseconds
were not common to the sides of the difference**, and could not cancel in it.

**So the iteration becomes two path accounts**, each holding the path's query and the path's standalone work **back to
back**, with the order alternating between the iterations and the two accounts' order alternating with it. **And each
account carries its own build side's work**, so that every leaf of that path's ladder is a difference of two measurements
taken inside one account.
**And the separation is published**, old and new, because a pairing that moves has to say how far it moved.

### A2 — The verdict is **published either way**, including "unattributable"

**Why**: the house has said "unattributed" since spec 030, on a set whose two sides were hundreds of milliseconds apart.
**A verdict that survives a proper pairing is worth publishing, and so is one that does not survive it** — the second is
the receipt that the question was asked properly.

**So the verdict is published either way**, and the question closes either way. **A question that is asked properly and
answered "not attributable" is closed.**
