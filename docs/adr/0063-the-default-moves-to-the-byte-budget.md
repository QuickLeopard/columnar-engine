# ADR-0063: The default moves to the byte budget

**Status**: accepted
**Date**: 2026-11-07
**Related**: [ADR-0061](0061-the-batch-sizes-price.md), [ADR-0062](0062-the-width-statement.md), [ADR-0023](0023-the-per-row-group-cost.md)

## Context

[ADR-0061](0061-the-batch-sizes-price.md) priced the reader's batch size and decided the rule's shape: **a byte
budget**, bounded by the measurement from both sides -- above the narrow shapes' whole row group (**12.8 MiB** and
**15.3 MiB**) and below the wide shape's next batch (**20.7 MiB**, **125,000 rows**) -- so the budget is **16 MiB**.
**And the measurement behind the upper bound was taken again**, after the default moved, and it says the wide shape's
cheapest batch is the **100,000** rows the rule gives it: **125,000** reads **1.763 ms** more slowly and a whole row
group **7.795 ms** more slowly still. **So the bound still holds, and it is tighter**: the budget must be above the
narrow shapes' whole row group and below **20.7 MiB**, and **16 MiB** is between them.

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
  get the whole row group -- **1,000,000** rows -- and the wide shape gets **100,000**.
- **And the rate is carried in eighths of a byte**, because a column's rows carry a **validity bit** and a bit is an
  eighth of a byte: a rate in whole bytes rounds it up to a byte, which is **eight times the truth**, and the rule
  then gives every shape less than the budget allows. **The estimate works in totals and charges the bit**, so the
  two read **the same statement**.
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

**So a per-worker budget of 16 MiB is 16 MiB a worker -- 96 MiB of batch memory on a six-worker read** -- and the
estimate would grow with the thread count while the budget does not. **And the rule's own output is published,
per worker count**: at six workers the file's sixteen columns get **15,625** rows where they get **100,000** at
one ([§3ah](../../PERF.md#3ah-the-default-moved)), so the workers share the budget and the read's batch memory
never exceeds it.

**And the numbers this section used to carry are withdrawn.** It said that *the join-orders query's estimate is
**259.1 MiB** at four workers under a per-worker budget, and **235.8 MiB** under the shared one*. **Both were
computed by hand**, under the rule as it was before the rate was corrected to eighths of a byte, and **no
instrument publishes that estimate**, so neither was ever a measurement. **The correction is recorded here**, and
the question the numbers asked is **open**: *what the built-in queries' estimates are at the thread counts the
file publishes*. **The decision does not wait for it** -- a built-in query that is refused at four threads is a
regression, and the shared budget is the reading that cannot produce one.

## Consequences

- **Every published figure the reader's batch size touches moves**, and each one is **re-measured** and published
  **old-and-new**. The old code is measured **back to back** with the new one, on this host, three campaigns each,
  **interleaved**, so that the code's share of every difference is separated from the machine's.
- **The estimate's verdict changes with the default**, and the estimate reads the **same** batch the read will hold.
- **The semver baseline moves**: `MAX_READER_BATCH_ROWS` left the public surface and `DEFAULT_READER_BATCH_BYTES`
  took its place. **The crates are unpublished**, so no embedder depended on the old name -- and the move is
  recorded in `scripts/semver-baseline.txt` and in the spec.
- **The published rule in [§3af](../../PERF.md#3af-the-batch-sizes-price) is corrected where it was published**: the
  narrow shapes get the whole row group **at one worker**, which is the setting the measurement was taken in, and
  the wide shape's batch is **100,000** where it published **62,500** -- because the rule divides the budget by the
  **statement**, and the statement is **9% low** on the wide shape
  ([§3ag](../../PERF.md#3ag-the-width-statement-paid)). The correction is recorded here.
- **And the rate was wrong at first**: the rule charged a **whole byte** a column a row for the validity bit,
  where the statement charges an **eighth** of a byte. That made the rule eight times dearer than the truth on
  that term, and the two materialised columns got **500,000** rows where the rule publishes **1,000,000**. The rate
  is carried in eighths of a byte now, and the estimate and the rule read the same statement. The correction is
  recorded here, and the amendment in the spec records it.
- **The knob is still there**, and it is still the measurement seam.
- **And the sweep's own set was a bad-host measurement, and that is published.** The sweep is the **only** family whose
  "before" was the set this file replaces -- the others have their old code measured **back to back with the new one**,
  interleaved, so that the difference is the code's. **So the sweep's difference is attributed by measurement**: the
  same instrument, the two codes, three campaigns each, interleaved, on this host, puts the code's share under
  **0.2 ms** on the join's part, while the set this file replaces reads **7 ms** more slowly than either code reads
  it now. **The movement between the two published sets is the host's**, and the reports are committed under their own
  names (`bench-results/batch-sweep-ab-oldcode-run<n>.json` and its new-code sibling) so that the attribution is
  derived rather than asserted ([§3af](../../PERF.md#3af-the-batch-sizes-price)).
- **And the default has moved, and it cost the two queries that read a narrow shape.** The engine's cold `q6` is
  **16.6 ms** where the old code's was **13.9 ms** -- **2.7 ms slower** -- and its cold `sort-topk` **39.2 ms**
  against **37.0 ms**, while the machine's own floor and `DuckDB`'s `q6` were flat. **The cause is named**: the
  rule gives the `q6`'s predicate set **1,000,000** rows at one worker, and that shape's scan curve turns up at a
  whole row group -- its minimum is at **250,000** rows, which is what the rule gives at **four** workers.
  **The budget's number is not changed on a guess**: a smaller budget would move the wide shape's batch below
  anything measured, and the scan's curve is about to change again when the reader owns the Parquet decode.
  **So the number is published, measured and left open** -- *the byte budget's number, once the reader owns the
  decode* ([§3ah](../../PERF.md#3ah-the-default-moved)).
