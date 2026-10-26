# Spec 016 — the per-row-group cost: tasks

**Spec:** [`spec.md`](spec.md) · **ADR:** [`docs/adr/0023-the-per-row-group-cost.md`](../../docs/adr/0023-the-per-row-group-cost.md)

Spec 015 measured the row-group count as a knob and published two findings: the
engine's parallelism cap is `min(threads, row_groups)`, exactly — and the
engine paid about **0.8 ms a row group** that DuckDB, Polars and DataFusion did
not pay, so the file with the most parallelism was the slowest one in absolute
terms. Spec 015 wrote the *mechanism* down as this spec's question rather than
guessing at it.

This spec answers the question and fixes the defect.

---

## R1 — Measure what the per-row-group cost actually is

**Requirement.** The mechanism is measured before anything is changed: which
column carries the cost, whether it is the dictionary's bytes, whether it is the
per-value expansion, and whether it is the reader's batch size.

**Done.**

| | what was measured | what it ruled out |
|---|---|---|
| The crate's read, one column at a time | `l_shipdate` alone: 14.64 / 23.96 / 39.35 ms on the 6 / 12 / 24-row-group files; `l_discount` and `l_extendedprice` alone: flat at 5.05–6.03 ms | the cost is not spread over the columns |
| The per-row-group cost | (39.35 − 14.64) / 18 = **1.373 ms a row group** | — |
| The dictionary's bytes | 66,239 → 264,572 bytes over the three files — eleven kilobytes a row group | it is not the dictionary's size |
| The engine's own work | the engine's scan at the derived batch is flat: **0.096 ms a row group** | it is not the pushdown's mask and not the materialisation's copy |
| The engine's scan, by reader batch size | every batch size that **divides** the row count is fast — seven of the eight cost 0.06–0.10 ms a row group — and 65,536, which divides none of 1,000,000, 500,000 or 250,000, costs **1.32** | it is the **partial batch** |

The instrument is `crates/columnar-engine/examples/row_group_cost_probe.rs`,
and the recipe is `just probe-cost`. The raw reports are
`bench-results/row-group-cost-{6,12,24}-run{1,2,3}.json`, and `python3 scripts/medians.py`
derives every figure above from them.

**Evidence.** ADR-0023 § *What the measurement found*; the probe's output.

---

## R2 — Derive the reader's batch size from the file's row groups

**Requirement.** The reader's batch size has to divide a row group's row
count, so that no row group ends with a partial batch. The size is derived
from the file, not chosen as a constant.

**Done.**

- `crates/columnar-engine/src/source.rs`
  - `MAX_READER_BATCH_ROWS` is now a **limit**, not a batch size.
  - `ParquetSource::batch_rows_for` takes the **greatest common divisor** of
    the row groups' row counts and rounds it down to the largest divisor under
    the limit.
  - `gcd` and `largest_divisor_at_most` are new, with unit tests.
  - `ParquetSource::batch_rows_under` is the public seam: it reports the batch
    the reader will use under a limit.
  - `BatchPlan` splits the two uses: `Derived` for the engine's own read,
    `Fixed` for the measurement seam, `read_with_batch_rows`.
- `crates/columnar-engine/src/dataset.rs` — the **eager** path gets the same
  rule, through `eager_batch_rows`.
- Six new unit tests: the gcd, the largest divisor under the limit, the
  divisor always divides the row count, the batch ends a row group without a
  partial batch, the batch is the one the row groups agree on, and the floor.

**The floor.** A divisor under 4,096 rows is not used: a batch of a handful
of rows costs more in per-batch overhead than it saves. When the row counts
are awkward — a prime, say — the limit bounds the batch instead, and the row
group ends with a partial batch. That is the cost of an awkward row count, and
it is paid once per row group.

**Evidence.** ADR-0023 § *The fix*; the unit tests.

---

## R3 — Measure the limit, for the shapes the lazy path serves

**Requirement.** The limit is a tuning constant, and a constant has to be
measured for the shape of the work it is used on. The lazy path serves two
shapes, and they were measured separately.

**Done.** `crates/columnar-engine/examples/lazy_batch_size.rs` was rewritten:
it now walks the limits, prints the **batch each limit derives** for the file,
measures both shapes, and A/Bs the old constant against the new batch with the
order alternating so the machine's drift cannot bias the result.

| shape | best | the limit in the tree |
|---|---|---|
| one materialised column, one predicate | 125,000 rows | 125,000 rows — the best measured |
| the real Q6's predicate set (three predicates, one a dictionary-encoded string) | 250,000 rows | 125,000 rows — within 2% of the best |

The two shapes pull in opposite directions, so the limit is the one that is
best on one and within 2% of the best on the other: **131,072**, which derives
**125,000** rows for the published file's million-row row groups. The limit
262,144 was measured and **rejected**: it derives 250,000 rows, which is
3.7–8.5% off the one-column shape's best.

**The interleaved A/B is the figure that decides it** — the batch the tree used against the batch it
derives now, the order alternating on every round so that a drifting machine cannot bias the
result: **−0.3%** on the one-column shape and **−24.1%** on the real Q6's predicate set.

**Evidence.** `PERF.md` §3e; `bench-results/batch-size-run{1,2,3}.json`; the probe's output.

---

## R4 — Republish every figure that moved

**Requirement.** Every published number that the fix moved is re-measured, and
the old figure is stated next to the new one.

**Done.** Three campaigns of each, interleaved, with the raw reports
committed: the comparison, the cold sweep on the three files, the load sweep,
the materialised probe on the three files, the size curve, the per-row-group
cost probe and the reader's batch limit. `scripts/medians.py` derives the
tables — including the ladder and the attribution, which are now script-derived like every other
published table — and `scripts/check-published-figures.py` guards them.

**The old figure and the new one are both published.** The per-row-group cost was **0.83 ms** in
spec 015's set and is **0.04 ms** in this one; the defect's own cost, measured on the probe's
back-to-back ladder, is **1.32 ms**. The absolute figures in the new set are larger than the old
set's for every system, because the host was under another tenant's memory pressure, and the reason
is named in [`PERF.md` §The conditions](../../PERF.md#the-conditions).

**Evidence.** `PERF.md` §2b, §2e, §2g, §3e, §3h, §3i, §3j, §4, §5;
`README.md`; `bench-results/README.md`.

---

## What was not done, and why

| | why |
|---|---|
| **The parquet crate's reader was not changed.** | The crate is a dependency the engine does not own, and the cost it charges for a partial batch is inside it. The engine's own rule makes the partial batch not happen; patching the crate would make the fix a patch, not a rule, and it would not survive an upgrade. |
| **Why the crate charges for a partial batch is not established.** | It is in a dependency. The *cost* is measured and published; the *reason* is not, and it is not guessed at. |
| **The generator's writer was not changed.** | The fix is in the reader, and the reader has to cope with the files other people write. A writer that only emits row counts the engine likes would hide the defect rather than fix it. |
| **The plan was not changed.** | The defect is in the read path. The plan is right. |
| **No new page-level pruning, no morsel driver.** | Both were already refused on measurement (specs 012 and 014), and nothing here changes that. |
| **The AVX2 ratios were not measured.** | There is no x86-64 machine. Correctness is verified under Rosetta 2; performance is not estimated. |
| **The three portfolio files were not removed, and no LICENSE was added.** | That is the repository's publicity work, not this spec's, and it is still outstanding. |
