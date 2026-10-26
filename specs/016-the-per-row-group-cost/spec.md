# Spec 016 — The per-row-group cost, and what it is

**Status**: done
**Started**: 2026-10-26
**ADR**: [ADR-0023](../../docs/adr/0023-the-per-row-group-cost.md)
**Predecessor**: [015-the-row-group-count-is-a-knob](../015-the-row-group-count-is-a-knob/spec.md)

## The problem

Spec 015 made the row-group count a knob and measured what it does. The cap is the file's:
`min(threads, row_groups)`, exactly, and the engine's speedup at fourteen threads rises with it —
4.95×, 5.76×, 6.44× on files with six, twelve and twenty-four row groups.

The same measurement found the other half, and it is a defect:

> **The engine's one-thread figure rises by 14.9 ms — 76% — across the three files, and the three
> incumbents' figures do not move at all.**

The engine pays **about 0.8 ms a row group** that `DuckDB`, `Polars` and the `DataFusion` peer do not
pay, reading the same bytes on the same machine. The rate is the same on a two-predicate query and a
three-predicate one, so it is a cost **per row group** and not per column.

The consequence is that **the more parallel file is the slower one**: the engine's best figure is
4.0 ms on the six-row-group file, and 5.4 ms on the twenty-four-row-group one. Unlocking the
parallelism costs more than the parallelism is worth.

Spec 015 wrote this down as the next spec's question. This is that spec.

## The requirements

### R1 — The mechanism, measured rather than guessed at

Three candidates, and the measurement says which:

- [x] **The parquet crate's per-row-group fetch.** The crate's reader is opened per row group, and
      the crate reads and decodes that row group's dictionary pages. The **raw reader** over the same
      projection is timed on its own, with no pushdown and no materialisation, so the crate's share of
      the cost is separated from the engine's.
- [x] **The dictionary read per column chunk.** The dictionary pages' byte counts are read off the
      file's metadata, per row group, so the bytes and the work can be compared.
- [x] **Something the engine does itself per row group.** The engine's own work — the mask, the
      materialisation, the builder's growth — is timed separately from the reader's.

- [x] The measurement runs on **all three files**, so the per-row-group cost is a **rate** and not a
      difference, and the three files' shapes are published side by side as in spec 015.

**The measurement's answer.** The crate's read is the whole of it: the engine's own work above the
crate is a constant, and the crate's read of `l_shipdate` alone carries the entire per-row-group
cost. The cause is **not** the dictionary's bytes and **not** the engine: it is the **partial
batch** at the end of a row group.

- [x] The crate's reader reads a row group at a time, and the reader's **batch size** decides how
      many rows a batch holds. When the batch size does not **divide** a row group's row count, the
      row group's last batch is **partial**.
- [x] **A partial batch costs 1.32 ms**, and the engine's batch size of 65,536 rows divides
      none of the three files' row-group sizes — 1,000,000, 500,000 and 250,000 — so the engine paid
      it once per row group, which is the whole of the cost spec 015 found.
- [x] Every batch size that **does** divide the row count is faster, and the size that does not is
      catastrophically slower: at 65,536 rows the twenty-four-row-group file's scan costs **54.48 ms**
      and at 125,000 rows, which divides all three row counts, it costs **24.01 ms**.
- [x] The ladder is **eight batch sizes on each of the three files**, in one session, and the seven
      that divide the row count all cost 0.06–0.10 ms a row group. The mechanism is the *shape* of the
      number and not its size.

- [x] The **limit** is measured over both shapes the lazy path serves — one materialised column
      with one predicate, and the real Q6's predicate set — and it is **131,072**, which derives
      **125,000 rows** for the published file: the best measured batch on the one-column shape, and
      within 2% of the best on the Q6 shape. The limit 262,144 was measured and rejected.

### R2 — The fix, in the read path

- [x] The reader's batch size is now the **largest divisor of the row groups' row count** that fits
      under the limit, and the limit is a limit on the batch, not a batch size.
- [x] The **eager** read gets the same rule, because it has the same defect.
- [x] The engine's **answer does not change**: the same rows, in the same order, and the same value.
      That is checked, not asserted.
- [x] The read path's **structure** does not change for any other reason: no new stage, no new
      dependency, no change to the plan.

### R3 — The measurement of the fix

- [x] The same three-file sweep and probe are re-run, three campaigns each, interleaved.
- [x] The **cost per row group** is published before and after.
- [x] The **incumbents are the control**, unchanged: they are on the same three files, and their
      figures must not move.
- [x] The engine's **best figure** is published for each file, because "the more parallel file is the
      slower one" is the claim that has to change.

### R4 — The publication

- [x] Every figure is the median of **three full campaigns**, with the spread, the raw reports
      committed, and the tables derived by `scripts/medians.py`.
- [x] `scripts/check-published-figures.py` guards the figures that are quoted.
- [x] [`PERF.md` §3j](../PERF.md#3j-the-row-group-count-and-what-it-costs) is updated: the defect
      moves from *"a defect, and the mechanism is the next spec's question"* to what it is now.
- [x] ADR-0023 records what was measured, whichever way it went.

## What the measurement found

- **The cost is the parquet crate's, and it is a partial batch.** The crate's reader reads a row
  group at a time; the reader's batch size decides where a batch ends, and a batch that ends in the
  middle of a row group is a **partial batch**, and a partial batch costs **1.32 ms**. The
  engine's batch size of 65,536 rows divides none of the three files' row-group sizes, so the engine
  paid it once per row group — which was the whole of what spec 015 found.
- **The engine's own work is not it.** Above the crate's read the engine's work is a constant: the
  pushdown's mask and the materialisation's copy do not scale with the row-group count. The engine's
  own scan, at the derived batch, is flat at **0.096 ms a row group**.
- **One column carries it.** The crate's read of `l_discount` and of `l_extendedprice` is flat across
  the three files — 0.010 and 0.015 ms a row group — and the crate's read of `l_shipdate` is
  **1.373 ms** a row group. The dictionary's bytes are not it: eleven kilobytes a row group against
  1.373 ms. Which of that column's properties makes it the one that pays is **not separated** by
  this measurement.
- **The fix is one line of arithmetic**: the reader's batch size is the largest divisor of the row
  groups' row count that fits under the limit, so no row group ends with a partial batch. The limit
  is **131,072**, measured over both shapes the lazy path serves, and it derives **125,000 rows**
  for the published file.
- **The engine's scan, at one thread**, across the three files, with the two batch sizes measured
  back to back in one session: **30.79 → 23.22 ms** (six row groups), **38.53 → 23.31 ms**
  (twelve) and **54.48 → 24.01 ms** (twenty-four). The per-row-group cost is gone: **1.32 ms →
  0.04 ms**, and the three files are within 3.4% of each other where they were 76% apart.
- **The incumbents do not move**, which is the control: they read the same bytes and their readers
  do not leave a partial batch behind.
- **The absolute figures moved because the host did.** The campaign ran on a machine under another
  tenant's memory pressure, so every number in this set is larger than the set before it, for every
  system. The fix is therefore measured **back to back**, with the two batch sizes alternating round
  by round, and the *ratio* — **24% faster on the real Q6's predicate set** — is the claim.

See [ADR-0023](../../docs/adr/0023-the-per-row-group-cost.md) and
[`PERF.md` §3j](../PERF.md#3j-the-row-group-count-and-what-it-costs).

## The non-goals

- **The generator's writer does not change.** The dead dictionary pages are a fact about the file,
  not about the engine — the same call ADR-0021 and ADR-0022 made.
- **The plan does not change.** No new predicate, no new stage, no new plan shape.
- **No new query.** The same `q6` and the same `q6-revenue` as spec 015, so the two campaigns are
  comparable.
- **The parallelism cap does not move.** `min(threads, row_groups)` is the file's and it is published
  as such.
- **Why the crate charges for a partial batch is not established.** It is in a dependency. The
  *cost* is measured and published; the *reason* is not, and it is not guessed at.
- **The residual per-row-group cost on the three-predicate shape is not separated.** The real Q6's
  three predicates still rise by about 0.5 ms a row group, and the third predicate's column,
  `l_quantity`, is the one the two shapes differ by. Whether the crate's per-row-group dictionary
  build or the engine's per-row-group comparison of the distinct values is the cost is not settled
  by this measurement, and it is written down as the next question.
- **Which of `l_shipdate`'s properties makes it the one column that pays is not separated.** The
  read of the other two columns is flat and the string column's read is not; the encoding, the
  dictionary, the page count and the size are not told apart by this measurement.
