# Spec 035 — The drift's own parts

**Status**: in progress
**Started**: 2026-11-04
**Predecessor**: [033-the-drift-taken-apart](../033-the-drift-taken-apart/spec.md)

## The problem

Spec 033 made the **drift** the yardstick every part is judged against, and it left one question
written down: **which of the drift's parts is the engine's, and how much of the engine's own cost
moves under the measurement**. The drift has never been taken apart, and the reason it has not is
structural, not an oversight.

**A drift is a range, and a range is not additive.** The drift of a measurement is the spread of its
medians over a session's campaigns — the largest minus the smallest. The drift of a **sum** is not
the sum of the drifts: two parts can each drift while the sum they make stays where it was. So the
drift of the engine's cost cannot be split into parts by arithmetic, and every attempt to read it as
a sum of its parts' drifts is wrong by construction.

**What is additive is the movement.** The difference between two campaigns' medians is additive, and
the parts tile: **per iteration**, the context's series is the sum of the ladder's leaves' series, and
the reconciliation is **0.000 ms** on both paths. So the movement of the context between two campaigns
is exactly the sum of the movements of its leaves — and the same holds for every other difference the
instrument takes, because every one of them is taken in one iteration, back to back, with the order
alternating.

**And the reports already carry what the movement needs.** Every part's **per-iteration series** is in
the committed report — the parts' (`series_ms`) and the ladder's leaves' (`ladder_series_ms`) — so the
movement and the drift of every part are derivable from a committed report, by a reader, without a new
measurement. **The drift's decomposition needs no new instrument.**

**And the decomposition already says something, and it says it in the wrong direction for the cold
path.** From the published set, the house's medians over the three campaigns:

| | the floor | the cold path | the resident path |
|---|---|---|---|
| the machine's own movement — the **floor** | **0.362 ms** | **0.362 ms** | **0.362 ms** |
| the **build side's read** — the file's state | — | **10.330 ms** | — |
| the **probe's row-number offset** | — | **1.700 ms** | **0.000 ms** |
| the **probe's own remainder** | — | **0.585 ms** | **0.219 ms** |
| the **probe's file state** | — | **0.000 ms** | **0.390 ms** |
| the **build side's gap** | — | **0.120 ms** | **0.564 ms** |
| the **context** | — | **0.667 ms** | **0.356 ms** |

**And the cold path's movement is not the machine's.** Pairing every part with the floor — the same
iteration, back to back, the order alternating — takes the machine's common movement out, and almost
none of it comes out: the cold stage's drift is **4.731 ms** and **4.416 ms** of it survives the
pairing; the build side's read's drift is **10.330 ms** and **10.611 ms** survives. The floor moves
**0.362 ms** while the cold path moves **4.731 ms**: the yardstick is an order of magnitude narrower
than the thing it measures, and the floor is **not** a valid yardstick for the cold path.

**And the movement is not the engine's either — most of it is the file's state.** The build side's
read's own series shows it: **11.763 ms**, **22.093 ms**, **11.898 ms** for the three campaigns, and
inside the middle campaign the read's own iterations are **11.713**, **11.403**, **22.093**, **38.640**,
**24.708**, **15.446** — three excursions against a baseline of about 11.7 ms. That is the file's
state, not the engine's work: the engine's read is the same code in every iteration.

**And the resident path — where nothing is read — is the opposite.** The context's drift there is
**0.356 ms** and the floor's is **0.362 ms**, so the floor **covers** it and the yardstick is sound
on that path; pairing takes the machine's common movement out of the resident stage's drift from
**0.340 ms** to **0.302 ms**. **The floor is a sound yardstick on the resident path and it is not one
on the cold path** — where the context's own drift is **0.667 ms** and the probe's row-number offset's
is **1.700 ms**, against the floor's **0.362 ms** — and the difference between the two paths is the
file's state.

**So the drift's parts are already named by the instrument, and what is missing is the ladder that
puts them in one column with the machine's share beside them.** The drift is not one number, it is
the **file's state**, the **machine's own movement**, and the **engine's own variance** — and the
share each has is a different number on each path. **How much of the engine's own cost moves under
the measurement** is the same question asked of the cost instead of the yardstick, and it has no
figure at all.

## The requirements

### R1 — The drift's decomposition closes exactly

The drift is a range and a range is not additive, so the decomposition is made of **movements**: the
difference between two campaigns' medians. Every difference the instrument takes is additive, because
the parts tile, and the decomposition **reconciles to 0.000 ms**: the movement of the engine's cost
between two campaigns is the sum of the movements of its parts, on both paths, for every pair of
campaigns.

### R2 — The machine's share is the floor, paired with the part it is compared against

The floor is the machine's own measure of a memory-bound loop, and it is measured **in the same
iteration, back to back, with the order alternating**, as every other side of a difference. So the
**paired** figure — the part's own series less the floor's own series, iteration by iteration — is the
part's own movement, with the machine's common movement taken out.

### R3 — The file's state is a part of its own, and it is published as one

The reads' movement is the file's state, and it is **not** the engine's work: the same code runs in
every iteration. So the file's state is named beside the machine's share and the engine's own share,
and the **build side's read** and the **probe's batches** are the parts that carry it.

### R4 — The yardstick's validity is answered per path

The floor is a valid yardstick for a path when the path's movement is at the floor's own movement.
It is **not** valid for the cold path, and the increment says so with the numbers. **And the
resolution that would be needed is published**: what a yardstick for the cold path would have to
measure, and what the resolution would be.

### R5 — How much of the engine's own cost moves is published

For both paths: the cost's drift, the drift as a share of the cost, and the share of the drift that
the machine's own movement accounts for. That is the figure the increment exists to produce, and it
is published **whether it is large or small**.

### R6 — The engine's own variance is the residue, and it is named

What the machine's share and the file's state do not account for is the engine's own variance, and it
is published as its own figure — not as "the rest", but as the named remainder of an exact
reconciliation.

### R7 — The increment changes no engine work, and the published figures stand

The instrument is not changed, and no engine path is touched. **The published figures stand**, and the
drift's decomposition is published **beside** them: the standing set keeps its name, and the
decomposition is derived from the same committed reports. If the campaigns are re-taken, the change is
explained and published old-and-new.

### R8 — The answer is published either way

A part that separates is an answer; a part that does not separate is an answer too, and it is
published with the numbers that show it and with the resolution that would be needed. The raw reports
are committed, the figures are derived from them by the guard, and the ADR records the decision.

## The plan's shape

Seven phases, and the edges between them:

1. **The ladder** — the movement machinery: every measurement's movement between every pair of
   campaigns, the reconciliation, and the machine's paired share. It reads the committed reports and
   writes nothing.
2. **The leaves** — the ladder applied to the context's leaves, on both paths, with the floor's share
   beside each one, and the engine's own variance as the named remainder.
3. **The file's state** — the reads' movement named as the file's state, and the cold path's own
   figure: the excursion, the baseline it broke away from, and what it means for the yardstick.
4. **The reproducibility** — the cost's drift, the drift as a share of the cost, and the machine's
   share of it, per path.
5. **The publication** — `PERF.md` publishes the ladder, the guard checks every figure **inside the
   section that publishes it**, and `scripts/medians.py` prints the decomposition beside the others.
6. **The ADR and the questions** — the ADR records the decision, and the next question is written down
   where this one was: the README's Next row and `STATUS.md`'s last section.
7. **The closure** — the spec's status is `done` with the date it finished, and CI is green on the
   publish commit.

The edges: 1 → 2 → 3 → 4 → 5 → 6 → 7, and 1 → 4 (the reproducibility needs the ladder), 2 → 5, 3 → 5.

## The scope

| | |
|---|---|
| The engine's own work | **This increment changes none of it.** R7 holds the published figures still: the instrument is not changed, and the decomposition reads the committed reports. |
| The instrument | **Not changed.** Every part's per-iteration series is already committed, so the movement of every part is derivable. A new measurement would be a new instrument, and it would move the published figures; if the decomposition turns out to need one, that is a **later** increment, and the reason is recorded. |
| The measurement track | **This is it.** The drift's own parts, and the reproducibility of the engine's cost, are the measurement track's remaining question. |
| The library's publish | **Not this increment's.** It is a separate act, with its own account, and it is written down as an open question. |
| The `parquet` bump | **Not this increment's.** The fix is upstream and available, but it moves the published figures and wants its own measurement, and it is written down as an open question. |

## The measurement

1. **The ladder is derived, not typed.** Every figure the increment publishes comes from the
   committed reports by the guard's machinery, and the guard looks for it **inside** the section that
   publishes it.
2. **The campaigns are not re-taken**, because the instrument is not changed: the decomposition reads
   the set the file publishes now — `bench-results/join-cost-run*.json` — and the set spec 034
   measured beside it — `bench-results/join-cost-spec034-run*.json`. **Both are published**, because
   the decomposition is a property of the code, and the two sets disagree about how much the machine
   moved on the day they were taken.
3. **The resolution is published** with the answer: what a yardstick for the cold path would have to
   measure, and what the resolution would be.

## The references

| | |
|---|---|
| [`constitution.md`](../../constitution.md) II, III, V | the method: measure first, the figures are derived, medians of three campaigns |
| [`constitution.md`](../../constitution.md) VII | explicit scope, no silent deferral |
| [`PERF.md`](../../PERF.md) §3o | the drift, the ladder and the verdicts this increment reads |
| [`PERF.md`](../../PERF.md) §3p | the re-measurement, and the second set the ladder is taken over |
| [`spec 033`](../033-the-drift-taken-apart/spec.md) | the drift as the yardstick, and the question this increment answers |
| [`spec 030`](../030-the-context-taken-apart/spec.md) | the ladder, and the reconciliation that closes exactly |
| [`ADR-0039`](../../docs/adr/0039-the-drift-taken-apart.md) | the drift's own decision |
| [`STATUS.md`](../../STATUS.md) | where the project's open questions live |
