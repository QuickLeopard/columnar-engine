# ADR-0061: The batch size's price — the rule is a byte budget

**Status**: accepted
**Date**: 2026-11-07
**Related**: [ADR-0060](0060-the-probe-measured-twice.md), [ADR-0023](0023-the-per-row-group-cost.md)

## Context

Spec 048 made the reader's batch size a knob — `ParquetSource::with_reader_batch_rows`, named in rows — and measured
what it **buys**: a batch of a row group, **1,000,000** rows, is **4.926 ms** cheaper than the engine's own
**125,000**, faster in **24 of 24** paired samples, and the curve is monotone.

**And it did not measure what it costs**, and the engine's own memory estimate does not model the batch at all.
Three things follow from that, and each of them is a question:

- **The bytes a batch holds scale with the query's width.** The reader holds a batch's columns at once: the join
  query reads two, and the file has sixteen. **And the knob is named in rows, so it cannot see the width.**
- **The engine's estimate does not model the batch.** It charges the file's rows for the columns the plan
  materialises, and nothing for the batch — so the budget cannot see the knob's price, and the estimate is wrong in
  **both** directions.
- **And the optimum is width-dependent**, so a row count cannot be the right rule for both shapes.

## The measurement

**A counting global allocator**, in the increment's own example, so that the **peak** a read holds is measured
rather than estimated — and the engine's own estimate printed beside it. **And the time is measured in a second
probe**, in one session, **back to back**, with the **order alternated**, so the join's win is priced against what
the other shape pays. Three campaigns each, on one host.

**The bytes, at the engine's own batch size and at a whole row group:**

| the shape | the estimate | the peak at 125,000 | the peak at 1,000,000 |
|---|---|---|---|
| the two predicate columns | 1,500,016 | 2,169,504 | 13,719,490 |
| the two materialised columns | 99,000,016 | 133,241,835 | 147,294,195 |
| the file's sixteen columns | 1,549,500,016 | 1,157,291,375 | 1,307,720,087 |

**Between the 62,500-row batch and the 1,000,000-row one, the peak grows by 13.39 bytes a row on the first shape,
by 16.03 on the second, and by 173.88 on the third.** A whole row group is therefore **12.8 MiB** on a narrow query
and **165.8 MiB** on a wide one.

**The time, against the engine's own batch size, paired:**

| the shape | 62,500 | 250,000 | 1,000,000 |
|---|---|---|---|
| the join's part | +0.345 ms (10 of 24) | −0.899 ms (16 of 24) | **−3.717 ms (24 of 24)** |
| the filter-range's scan | −0.026 ms (13 of 24) | −0.204 ms (16 of 24) | −0.108 ms (13 of 24) |
| the wide read | −0.442 ms (12 of 24) | +2.335 ms (10 of 24) | **+14.455 ms (1 of 24)** |

## Decision

1. **The reader's batch size is a byte budget, and the budget is 16 MiB.** The number is bounded by the measurement
   **from both sides**: the narrow shapes' whole row group costs **12.8 MiB** and **15.3 MiB**, so the budget must
   be above that or the shapes that win would lose; and the wide shape's own batch costs **20.7 MiB**, and the wide
   read is **0.442 ms** cheaper at **62,500** rows than at the engine's own, so the budget must be at or below that
   or the shape that loses would lose more. **The budget is between 12.8 MiB and 20.7 MiB, and it is 16 MiB.**

2. **The rule gives every shape at least what it has today.** The narrow shapes get the whole row group —
   **1,000,000** rows — and the wide shape gets **62,500**, the largest divisor of a row group's row count under
   **16 MiB**: **80,000 does not divide a million**, and the divisors are coarse. That is a **smaller** batch than
   today for the wide shape, and by the measurement it is **not a loss** — the wide read is **0.442 ms** cheaper at
   **62,500** rows than at the engine's own.

3. **The default does not move in this increment.** Moving it moves **every** published figure the reader's batch
   size touches — the headline table, the thread sweeps, the ladder, the join's parts — and a moved figure is
   published old-and-new and **re-measured**, not recomputed. **A rule that is decided and a default that moves are
   two different acts**, and this increment does the first.

4. **The estimate is measured against, and not changed.** The estimate is **9.15× low** on the narrow shape at a
   whole row group and **25% high** on the wide one, and the wide shape is **refused by the default budget at every
   batch size**, because the estimate does not depend on the batch size at all. **The budget cannot see the knob's
   price.** The refusal itself is right — the read holds **1,139,591,586** bytes at every size — but it is right by
   accident, and a plan that *would* fit is refused for the same reason. Fixing the estimate moves its own published
   figures — including a sentence about a query the budget **refuses** — and that is **spec 050's** work.

5. **The estimate's document and the estimate's code disagree, and that is recorded here.** The module says a
   dictionary-encoded string column's *"per-row cost is the index"*; the code charges a `String` header, **24 bytes a
   row**, for a column whose index is **four**. **Both paths do it**: the eager path's estimate *has* the chunk's
   encoding in hand and charges the header anyway, and the lazy path's estimate cannot see the encoding at all.
   **The next increment gives the engine one honest statement of what a row costs**, and the estimate and the
   batch's rule share it.

## Consequences

**What became knowable. The knob's price, in bytes and in milliseconds.**

The batch's price is **13.39 bytes a row** on a two-column query and **173.88 bytes a row** on a sixteen-column one,
and the engine's own limit is a **row count** — so the limit cannot see either. **And the batch's optimum is
width-dependent**: a whole row group is worth **3.717 ms** to the join's part and **14.455 ms** against the wide
read. **So the rule is a byte budget, and the budget is 16 MiB.**

**And the engine's memory estimate is wrong in both directions, and nothing checked it.** It is **9.15× low** where
the batch is the read, **25% high** where the file's columns are, and the budget **refuses a plan it could have run**
— at every batch size, because the refusal does not depend on one. **The estimate's own doc and the estimate's own
code disagree** about what a dictionary column costs a row.

**What is not known, and is the next question.** **Why a shorter run of the probe costs more per row**, when the
table is equally cold either way and the reader's work is the same — and now with **the batch's bytes** named as
what a bigger batch buys and what the wide shape pays for.

**And the next increment** builds what this increment measured: the estimate and the rule share one honest statement
of what a row costs, the reader's batch size becomes a byte budget, and **every figure that moves is published
old-and-new and re-measured**.

**And the engine's diff in this increment is empty.** No engine behaviour changed, and the public surface does not
move — the knob is already there, and the rule is a decision, not a change.
