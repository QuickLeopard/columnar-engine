# ADR-0042 — The file-state floor

**Status**: accepted
**Date**: 2026-11-04
**Spec**: [036-the-file-state-floor](../../specs/036-the-file-state-floor/spec.md)

## Context

Spec 035 took the **drift** apart and published the answer: the drift has three parts — the machine's own
movement, the file's state and the engine's own variance — and which of them is largest is a property of
the host's state on the day, not of the code. **And it named the resolution that would be needed**: a
yardstick for the cold path that sees the file's state as well as the machine's, because the cold path's
largest leaf **is** the file's state, and **no floor over a memory-bound loop can see it**. The instrument
measured the build side's file state **once per campaign** — the process's first read of the build side's
data — and not per iteration, so the excursions inside the iterations could not be attributed to it from
the report.

**And the attribution that was published rested on two series measured in different blocks.** The build
side's read is measured in the instrument's **build block**; the machine's floor is measured in the
**probe block**, after the interleaved loop is over; the ladder's leaves are measured in the
**interleaved loop**. The published set's read excursed — **11.713**, **11.403**, **22.093**, **38.640**,
**24.708**, **15.446** against a baseline of about 11.5 ms — while the floor over the same campaign read
**5.926**, **5.808**, **5.728**, **5.773**, **5.809**, **5.989** and did not move. **The comparison that
carried the attribution compared two series minutes apart.**

**And the published text said something the instrument did not do.** PERF.md §3q and ADR-0041 both say
that the floor "is measured **in the same iteration, back to back, with the order alternating**". **The
sides are; the floor was not.** It was measured in its own block and paired with the leaves by index, so
the machine's movement it takes out is the machine's movement **in another block**. The pairing was a
reasonable proxy — the machine's movement is slow-moving — and it was not what the text said, and a
published claim the evidence does not carry is a defect whatever the conclusion happens to be.

## Decision

1. **Both floors are measured in the iteration.** The **machine's own measure** is the memory-bound loop
   the probe cannot go below, over the probe's own fresh column, **right after** the probe that used it,
   so it reads nothing of its own and perturbs nothing. The **file-state floor** is the build side's read
   through the engine's own reader, with the engine's own predicates, over the engine's own columns,
   taken **right after** the machine's own measure. The two are **8.565 ms**, **8.871 ms** and
   **8.793 ms** apart in the three campaigns — back to back, where the old pairing was minutes apart.

2. **The file's state is measured where the leaves are, and the yardstick sees it.** A leaf's yardstick
   is the sum of the drifts of the measurements it stands on, the machine's own drift, and the
   **file-state floor's** drift — the larger of them. On the increment's set the file-state floor drifts
   **0.361 ms** against the machine's **0.258 ms**, and the only leaf whose yardstick changes is the
   open's gap's, from **0.258 ms** to **0.361 ms**. **The verdicts stand**: unattributed and sound on the
   cold path, the memory's and sound on the resident path.

3. **The sentence §3q published is corrected where it was published.** §3q said the floor was measured in
   the same iteration, back to back, with the order alternating. **It was not**, and §3q now says so and
   points here. **This ADR is the record of the decision ADR-0041 made**, and the correction is ADR-0042's.

4. **The figures that moved are published old and new, with the reason.** The code before this increment
   was measured back to back with the code after it, on this host, in the same session: the machine's own
   measure **6.891 ms** → **5.168 ms** (**−1.723 ms**), because the column it reads is now the one the
   probe has just streamed over; and the file's own read **12.006 ms** → **12.479 ms** (**+0.474 ms**),
   because it is now measured in the iteration rather than six times in a row in a block. **Neither
   difference is the engine's work.**

5. **The placement of the file's state is shown to decide a verdict.** On the code before this
   increment's own numbers, the build block's read drifts **0.806 ms** against the resident path's
   probe's file state's yardstick of **0.778 ms**, and that leaf sits at **−0.788 ms**: outside the
   narrower ruler and inside the wider one. **A file's state measured in the block would have flipped the
   resident path's verdict from the memory's to unattributed — on a number measured in another block.**
   The file's state has to be measured where the leaves are, or it must not be in the yardstick at all.

6. **The attribution is published as it came out.** The excursions did not recur: over **nine more
   campaigns** of the same instrument, on a quieter host, the file's own read's largest spread within a
   campaign was **1.805 ms** and the machine's own movement's was **1.842 ms**. **So on this host, today,
   neither the file nor the machine excursed**, and the published set's excursion at **38.640 ms** — on a
   day whose load average was **3.66** to **4.34**, lower than any of the fifteen campaigns measured here
   — is not re-attributed. **What the increment makes true is that the next time the file's state moves,
   the two will be measured apart, in the same iteration.**

7. **The answer is published either way.** The instrument exists and every figure above is derived from
   the committed reports by the guard; the correction is published where the claim was; and the yardstick
   that sees the file's state as well as the machine's exists and is the yardstick §3r publishes.

## Consequences

- **A yardstick that sees the file's state is now a thing, not a resolution.** It is the machine's own
  drift, the file-state floor's drift, and the larger of them. **Which of them is larger is a property of
  the host's state on the day** — on this day they were **0.258 ms** and **0.361 ms**.
- **The published figures stand.** This increment changed no engine work. The figures that come from the
  instrument moved, and every one of them is published old and new with the reason, beside the set the
  file already publishes.
- **The attribution is still an argument, and the instrument is what makes it a measurement.** The
  published set's excursions were a day; this increment did not reproduce them, and it says so rather than
  publishing an attribution it cannot support.
- **The next question is written down where this one was**: the README's Next row and `STATUS.md`'s last
  section. It is **not** another floor: the measurement track's question is answered, and what is left is
  the work the repository cannot take on its own — the library's publish, and the `parquet` bump.
- **The instrument is now more expensive.** The campaign reads the build side once more, and the two
  floors' clock reads are the ones the build block's read and the probe block's floor no longer take:
  the campaign takes the same number of clock reads it took before.

## References

- [`constitution.md`](../../constitution.md) II — the method: measure first, and the figures are derived
  from the committed reports rather than typed.
- [`constitution.md`](../../constitution.md) V — the method's evidence rules: the medians of at least three
  campaigns, and every quoted figure agreeing with the committed report.
- [`constitution.md`](../../constitution.md) VII — explicit scope, no silent deferral.
- [`PERF.md`](../../PERF.md) §3q — the drift's own parts, and the claim this increment corrects.
- [`PERF.md`](../../PERF.md) §3r — the file-state floor, the yardstick that sees the file, and the
  attribution as it came out.
- [`ADR-0041`](0041-the-drifts-own-parts.md) — the drift's own parts, and the decision this one records
  the correction of.
- [`spec 035`](../../specs/035-the-drifts-own-parts/spec.md) — the drift's own parts, and the resolution
  this increment builds.
