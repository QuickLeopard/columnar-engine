# ADR-0063: The default moves to the byte budget

**Status**: accepted
**Date**: 2026-11-07
**Related**: [ADR-0061](0061-the-batch-sizes-price.md), [ADR-0062](0062-the-width-statement.md), [ADR-0023](0023-the-per-row-group-cost.md)

## Context

[ADR-0061](0061-the-batch-sizes-price.md) priced the reader's batch size and decided the rule's shape: **a byte
budget**, bounded by the measurement from both sides -- above the narrow shapes' whole row group (**12.8 MiB**) and
at or below the wide shape's own batch (**20.7 MiB**) -- so the budget is **16 MiB**.

[ADR-0062](0062-the-width-statement.md) built the statement the budget divides by: **what a row costs** in the
engine's own storage, with the **signal** being the schema's own Arrow type.

**And the default still divided a byte budget by a row count.** The engine's reader handed the rows over in batches
of **125,000** -- the largest divisor of a million-row row group under a limit of **131,072** rows -- and the limit
could not see the query's width, because **the same million-row batch is 12.8 MiB on a two-column query and 165.8 MiB
on a sixteen-column one**.

**So the rule was decided and the statement was built, and the default did not move.** The published figures were
all taken with the row limit's batch, and **moving the default moves every published figure the reader's batch size
touches**.

## Decision

**The default's rule is a byte budget of 16 MiB, and the batch is the read's own.**

- **The budget is divided by the columns the read fetches**, from the **width statement**. So the two narrow shapes
  get the whole row group -- **1,000,000** rows -- and the wide shape gets **62,500**, which the measurement says is a
  small win and not a loss.
- **The budget is the read's, and the workers share it.** A parallel read holds one batch per worker, and the
  engine's memory estimate charges it that way, so a read on six workers gives each of them a sixth of the budget.
  **At one worker -- the measurement's setting, and the headline's -- the rule is exactly the published one.**
- **The batch is still a divisor of the row group's row count**, so no row group ends with a partial batch.
- **The knob is not the default.** `with_reader_batch_rows` names a size in rows: it is the **measurement seam**,
  and every published figure that names one stays reproducible by the command that made it.
- **The public surface says so.** `MAX_READER_BATCH_ROWS` leaves it -- the row limit is not the rule any more -- and
  `DEFAULT_READER_BATCH_BYTES` takes its place.

## Why the workers share it

**The budget is per batch, and the measurement is per batch -- and the memory model is per read.** The engine's
estimate charges the batch **once per worker**, because a parallel read holds one batch per worker.

**So a per-worker budget of 16 MiB is 96 MiB of batch memory on a six-worker read**, and the built-in queries do not
fit the 256 MiB budget any more: **the join-orders query's estimate is 259.1 MiB at four workers** under a
per-worker budget, and **235.8 MiB** under the shared one. **The baseline is a first-class citizen**, and a built-in
query that is refused at four threads is a regression.

**So the workers share the budget, and the read's batch memory never exceeds it.** That is the same number the
measurement bounded, read the way the memory model reads it.

## Consequences

- **Every published figure the reader's batch size touches moves**, and each one is **re-measured** and published
  **old-and-new**. The old code is measured **back to back** with the new one, on this host, three campaigns each,
  **interleaved**, so that the code's share of every difference is separated from the machine's.
- **The estimate's verdict changes with the default**, and the estimate reads the **same** batch the read will hold.
- **The semver baseline moves**: `MAX_READER_BATCH_ROWS` left the public surface and `DEFAULT_READER_BATCH_BYTES`
  took its place. **The crates are unpublished**, so no embedder depended on the old name -- and the move is
  recorded in `scripts/semver-baseline.txt` and in the spec.
- **The published rule in [§3af](../../PERF.md#3af-the-batch-sizes-price) is corrected where it was published**: the
  narrow shapes get the whole row group **at one worker**, which is the setting the measurement was taken in. The
  correction is recorded here.
- **The knob is still there**, and it is still the measurement seam.
