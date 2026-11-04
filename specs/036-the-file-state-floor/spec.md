# Spec 036 — The file-state floor

**Status**: done
**Started**: 2026-11-04
**Finished**: 2026-11-04
**Predecessor**: [035-the-drifts-own-parts](../035-the-drifts-own-parts/spec.md)

## The problem

Spec 035 took the **drift** apart and published the answer: the drift has three parts — the machine's
own movement, the file's state and the engine's own variance — and which of them is largest is a
property of the host's state on the day, not of the code. **And it named the resolution that would be
needed**: a yardstick for the cold path that sees the file's state as well as the machine's, because
the cold path's largest leaf **is** the file's state, and **no floor over a memory-bound loop can see
it**. The instrument measures the build side's file state **once per campaign** — the process's first
read of the build side's data — and not per iteration, so the excursions inside the iterations cannot
be attributed to it from the report.

**And the attribution that was published rests on two series that were measured in different blocks.**

The published set's build side's read is the series with the excursions, and in the middle campaign it
reads **11.713 ms**, **11.403 ms**, **22.093 ms**, **38.640 ms**, **24.708 ms**, **15.446 ms** — three
excursions against a baseline of about **11.5 ms**. Its three campaigns' medians are **11.763 ms**,
**22.093 ms**, **11.898 ms**, and its drift is **10.330 ms**. The machine's own floor over the same
campaign reads **5.926**, **5.808**, **5.728**, **5.773**, **5.809**, **5.989** — a drift of
**0.362 ms** and not one excursion.

**And those two series cannot be compared iteration by iteration, because they were not measured
together.** The build side's read is measured in the instrument's **build block**; the floor is
measured in the **probe block**, after the interleaved loop is over; and the ladder's leaves are
measured in the **interleaved loop**, a third place. So the comparison that carried the attribution —
the read excursed, the floor did not, therefore the excursions are the file's state — compared a
series from the build block with a series from the probe block, **minutes apart**, and the conclusion
was drawn from the fact that the machine's own measure was flat while the read was not.

**And the published text says something the instrument does not do.** PERF.md §3q and ADR-0041 both
say that the floor "is measured **in the same iteration, back to back, with the order alternating**",
and that the paired figure — the leaf's own series less the floor's own series, iteration by
iteration — is therefore "the leaf's own movement with the machine's common movement taken out". **The
sides are measured in the same iteration, back to back, with the order alternating. The floor is not:
it is measured in its own block, after the interleaved loop, and it is paired with the leaves by
index.** The pairing is a reasonable proxy — the machine's movement is slow-moving — but it is not what
the text says, and a published claim that the evidence does not carry is a defect whatever the
conclusion happens to be.

**And the file's state has no per-iteration measure at all.** The one figure that is the file's state
is the build side's file state, taken once per campaign, before anything else reads the build side's
data. So the question the resolution asks — *what does the file's state cost, in the iteration where
the excursions happened* — has no answer in the report, and cannot have one: the instrument does not
measure it there.

## The requirements

### R1 — The machine's own movement is measured in the iteration

The floor is the machine's own measure of a memory-bound loop, and it is a **side**: it is measured in
the same iteration as the parts it is judged against, **back to back**, with the **order alternating**
between the iterations, exactly as every other side is. So the published pairing — the leaf's own
series less the floor's own series, iteration by iteration — is literally what the instrument does,
and the machine's movement is taken out of the movement that was measured.

### R2 — The file's own read is measured in the iteration, and it is the file-state floor

The file's state is what the engine pays when it reads the file, so the file's own read — through the
engine's own reader, with the engine's own predicates, over the engine's own columns — is measured
**per iteration**, beside the parts, and it is named as the **file-state floor**. It is the same
measurement the instrument already takes, moved into the iteration, so the excursions and the machine's
own movement are **in the same iteration** and can be attributed to one or the other.

### R3 — The perturbation is measured, not assumed

Measuring the file's state inside the iteration means the iteration does more work, and the order of
the measurements inside it is not neutral. So the **order's own cost** is published for the new
series, the **added clock cost** is published, and the two floors' own drifts are published beside the
figures they are used to judge. **What it costs to measure the file's state is a published figure, not
a caveat.**

### R4 — The attribution is answered, per iteration

For every iteration: the file's own read, the machine's own movement, and the **paired** figure — the
file's read less the machine's own movement, iteration by iteration — which is the file's share with
the machine's common movement taken out. **The answer is whether the excursions are the file's state
or the machine's**, and it is published with the per-iteration figures that show it.

### R5 — The yardstick sees the file's state as well as the machine's

A leaf's yardstick is the sum of the drifts of the measurements it stands on, the machine's floor's own
drift, and the **file-state floor's own drift** — the larger of them. So the yardstick is wide enough
to cover what the machine **and the file** could have produced on their own, and the leaves' verdicts
are re-answered against it. The verdict that is **unattributed** under a yardstick that cannot see the
file's state is not the same verdict as the one under a yardstick that can.

### R6 — The engine's own work is unchanged, and every moved figure is published old-and-new

No engine path is touched. The instrument changes, so the figures that come from it **move**, and every
one of them is published **old and new with the reason** — including the floor's own figure, which
moves because it is now measured in the iteration, and the build side's read, which moves because it
is now measured there too.

### R7 — The correction is published

The published text claimed that the floor is measured in the same iteration, back to back, with the
order alternating. **It was not**, and the correction is published with the reason and with both
figures: the old pairing — across the blocks — and the new one — inside the iteration. A published
claim that the evidence did not carry is corrected in the place that carries it.

### R8 — The answer is published either way

A file-state floor that settles the attribution is an answer; one that does not is an answer too, and
it is published with the numbers that show it and with the resolution that would be needed. The raw
reports are committed, the figures are derived from them by the guard, and the ADR records the
decision.

## The plan's shape

Seven phases, and the edges between them:

1. **The instrument** — the build side's read and the machine's floor move into the iteration, beside
   the parts, and the report carries their per-iteration series.
2. **The pairing** — the paired series (the file's read less the machine's own movement), the
   attribution figures, and the order's own cost for the new series.
3. **The yardstick** — the file-aware yardstick, and the leaves' verdicts re-answered against it.
4. **The campaigns** — three campaigns of the new instrument, the raw reports committed.
5. **The publication** — `PERF.md` publishes the new set's ladder beside the standing set's, the guard
   checks every figure **inside the section that publishes it**, and `scripts/medians.py` prints it.
6. **The ADR and the questions** — the ADR records the decision and the correction, and the next
   question is written down where this one was: the README's Next row and `STATUS.md`'s last section.
7. **The closure** — the spec's status is `done` with the date it finished, and CI is green on the
   publish commit.

The edges: 1 → 2 → 3 → 4 → 5 → 6 → 7, and 1 → 2 (the pairing needs the new series), 3 → 5, 4 → 5.

## The scope

| | |
|---|---|
| The engine's own work | **This increment changes none of it.** No engine path is touched: the file-state floor is an instrument's measurement, not a change to the engine. |
| The instrument | **Changed, and the reason is published.** The build side's read and the machine's floor move into the iteration, so the file's state and the machine's movement are measured where the parts are. The figures that come from the instrument move, and every one of them is published old and new with the reason. |
| The published set | **The standing set does not slide.** The set the file publishes keeps its name and its figures, and the new set is published **beside** it under its own name. Sliding it would publish a changed instrument's numbers as the old instrument's, which is what spec 034 already refused. |
| The measurement track | **This increment answers the last named resolution** — the file-state floor. What it leaves is named in `STATUS.md`. |
| The library's publish | **Not this increment's.** It is a separate act, with its own account, and it is written down as an open question. |
| The `parquet` bump | **Not this increment's.** The fix is upstream and available, but it moves the published figures and wants its own measurement, and it is written down as an open question. |

## The measurement

1. **Three campaigns of the new instrument**, on this machine, the raw reports committed under their
   own names. The medians are of three campaigns, and the spread is published.
2. **The standing set is not re-taken**: the old instrument's reports are committed and they stay
   where they are. The new set is published beside them, and the figures they share are compared with
   the reason named.
3. **The correction is measured, not asserted**: the old pairing (across the blocks) and the new one
   (inside the iteration) are both published, from the new set's own report, so that the difference
   the correction makes is a figure.
4. **The host's state is published** with every campaign — the load average, as every report carries
   it — because the answer depends on whether the file's state moved on the day.

## The references

| | |
|---|---|
| [`constitution.md`](../../constitution.md) IV | evidence over claims: every published number in a checked-in benchmark, and the machine it was measured on |
| [`constitution.md`](../../constitution.md) V | the method's evidence rules: the medians of at least three campaigns, and every quoted figure agreeing with the committed report |
| [`constitution.md`](../../constitution.md) VII | explicit scope, no silent deferral |
| [`constitution.md`](../../constitution.md) — Performance Standards | repeated runs and published medians; a gate that can say "I could not tell"; a published number must be checkable where it was taken |
| [`PERF.md`](../../PERF.md) §3o | the drift, the ladder and the verdicts this increment re-answers |
| [`PERF.md`](../../PERF.md) §3q | the drift's own parts, the yardstick's validity, and the claim this increment corrects |
| [`spec 033`](../033-the-drift-taken-apart/spec.md) | the drift as the yardstick |
| [`spec 035`](../035-the-drifts-own-parts/spec.md) | the drift's own parts, and the resolution this increment builds |
| [`ADR-0039`](../../docs/adr/0039-the-drift-taken-apart.md) | the drift's own decision |
| [`ADR-0041`](../../docs/adr/0041-the-drifts-own-parts.md) | the drift's own parts, and the claim this increment corrects |
| [`STATUS.md`](../../STATUS.md) | where the project's open questions live |
