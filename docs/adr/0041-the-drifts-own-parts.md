# ADR-0041 — The drift's own parts

**Status**: accepted
**Date**: 2026-11-04
**Spec**: [035-the-drifts-own-parts](../../specs/035-the-drifts-own-parts/spec.md)

## Context

Spec 030 took the **context** apart and the decomposition closed exactly: the join stage's plan output
reports its parts, the parts tile the context in every iteration, and the reconciliation is **0.000 ms**
on both paths. Spec 033 then took the **drift** — the spread of a measurement's medians over a
session's three campaigns — and made it the yardstick every part is judged against, with a verdict
taken over the ladder's leaves. **And it left one question written down: which of the drift's parts is
the engine's, and how much of the engine's own cost moves under the measurement.**

**And the question had a structural difficulty, not a lazy one.** A **drift** is a range, and a range
is **not additive**: the drift of a sum is not the sum of the drifts, and two parts can each drift
while the sum they make stays where it was. So the drift of the engine's cost cannot be split into
parts by arithmetic, and every attempt to read it as a sum of its parts' drifts is wrong by
construction. What **is** additive is the **movement** — the difference between two campaigns —
because **per iteration** the ladder's leaves sum to the context and the reconciliation is **0.000 ms**.

**And the reports already carried what the movement needed.** Every part's **per-iteration series** is
in the committed report — the parts' (`series_ms`) and the ladder's leaves' (`ladder_series_ms`) — so
the movement and the drift of every part are derivable from a committed report, by a reader, **without a
new measurement**.

## Decisions

1. **The ladder is built on the movement, and the movement is taken on the means.** The median of a sum
   is not the sum of the medians, and the mean is the statistic the parts tile. The **drift** itself
   stays the range of the medians — it is the yardstick's own statistic — and it is published beside
   the movement. The ladder is taken over **the pair of campaigns that make the context's own drift**,
   so the movement is the one the yardstick is about.

2. **The ladder's leaves are the tiling leaves**: the build side's gap, the open's gap, the probe's
   file state **less** its row-number offset, and the probe's own remainder. The part in no gap is the
   fifth leaf and it is always zero. **And the ladder closes exactly**: the movement of the context is
   the sum of the movements of its leaves, and the reconciliation is **0.000000 ms** on both paths and
   on both sets. **And every leaf carries its paired figure** — the leaf's own series less the
   floor's own series, iteration by iteration — because the floor is the machine's own measure and it
   is measured **in the same iteration, back to back, with the order alternating**: the paired figure
   is the leaf's own movement with the machine's common movement taken out.

3. **The instrument is not changed.** The file's state is measured **once per campaign** — the
   process's first read of the build side's data — and not per iteration, so the excursions inside the
   iterations cannot be attributed to it from the report. **A file-state floor** — the file's own read,
   measured in the iteration, paired with the parts — is what would settle it, and it is a **new
   instrument**, which wants its own measurement and its own increment.

4. **Both sets are published, beside each other.** The decomposition is a property of the code; the
   shares are a property of the **day**. The set the file publishes and the set spec 034 measured are
   published side by side, and the difference between them is published with the reason.

5. **The yardstick's validity is answered leaf by leaf, and the answer flips between the two sets.**
   A leaf's yardstick is its own drift unless the floor's is wider. On the published set's cold path
   the probe's file state less its row-number offset drifts **1.700 ms** and the probe's own remainder
   **0.585 ms** against the floor's **0.362 ms**: the floor does not reach them. On the increment's set
   the cold path's context drifts **0.184 ms** against the floor's **0.275 ms** — covered — and the
   resident path's drifts **0.371 ms** — not covered. **Which of the drift's parts is largest is a
   property of the host's state on the day, not of the code.**

6. **How much of the engine's own cost moves under the measurement is published as a figure and as a
   share**: the cold path's drift is **18.72%** of its cost in the published set and **6.76%** in the
   increment's; the resident path's is **1.75%** and **4.52%**. **The engine's cost moves by about two
   per cent on the path that holds the data and by about a fifth on the path that reads the file**,
   and the difference between the two is the file's state, not the engine's work.

7. **The answer is published either way.** It is: the drift has three parts — the machine's own
   movement, the file's state and the engine's own variance — and the two sets say which of them is
   largest on the day they were taken.

## Consequences

- **The yardstick is not a property of the code.** The floor is a sound yardstick on the resident path
  of one set and not on the cold path of the same set, and the other set says the opposite on the other
  path. **So a published verdict that says "at the noise" carries the host's state with it**, and the
  drift's ladder says how much.
- **A drift cannot be split into parts, and a publication that says otherwise is wrong.** The ladder
  is the way to say it, and the reconciliation is what makes it honest: **0.000000 ms**.
- **The next question is written down where this one was**: the README's Next row and `STATUS.md`'s
  last section. It is **a file-state floor** — the file's own read, measured in the iteration, paired
  with the parts — so that the cold path's yardstick sees the file's state and not only the machine's.
- **The published figures stand.** This increment changed no engine work and no instrument, and the
  decomposition is derived from the same committed reports.

## References

- [`constitution.md`](../../constitution.md) II — the method: measure first, and the figures are
  derived from the committed reports rather than typed.
- [`constitution.md`](../../constitution.md) V — the method's evidence rules: the medians of at least
  three campaigns, and every quoted figure agreeing with the committed report.
- [`PERF.md`](../../PERF.md) §3o — the drift, the ladder and the verdicts this increment reads.
- [`PERF.md`](../../PERF.md) §3q — the drift's own parts, and the ladder this increment published.
- [`ADR-0038`](0038-the-context-taken-apart.md) — the context's decomposition, and the reconciliation
  that closes exactly.
- [`ADR-0039`](0039-the-drift-taken-apart.md) — the drift's own decision.
- [`spec 033`](../../specs/033-the-drift-taken-apart/spec.md) — the drift as the yardstick, and the
  question this increment answers.
