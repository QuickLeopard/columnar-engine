# Spec 051 — The default moves to the byte budget — tasks

**Status**: done
**Started**: 2026-11-07
**Finished**: 2026-11-07
**Spec**: [spec.md](spec.md)

The order is the house's: **the rule is decided, the statement is built, and now the default moves.** The knob is
there, the price is measured, the statement is checked against the measurement -- and the engine still reads with
a row count.

## The engine

- [x] The default's rule is a **byte budget of 16 MiB**, and the batch size is **the largest divisor of the row
      group's row count** whose bytes fit it.
- [x] The budget is divided by the **columns the read fetches**, from the **width statement** -- so two different
      widths get two different batches.
- [x] The batch still **divides the row group's rows**, so that no row group ends with a partial batch.
- [x] **The knob still overrides it**: `with_reader_batch_rows` names a size in rows, and it is not the default.
- [x] The estimate charges **the batch the read will use**, and the two come from the same place.
- [x] `MAX_READER_BATCH_ROWS` leaves the public surface and `DEFAULT_READER_BATCH_BYTES` takes its place, and the
      semver baseline moves.
- [x] **And the rate is in eighths of a byte** (amendment **A3**): a whole-byte rate charged the validity bit a
      whole byte, which was eight times the truth, and it cost the materialised shape half its batch.

## The campaigns

- [x] Every campaign that was taken with the engine's own batch size is **taken again**, three campaigns each.
- [x] **And the figures that do not move are not re-taken** -- the sweeps, the width statement and the memory
      campaigns name their own batch sizes, and the eager path has its own. **Amended by A4**: those three were
      taken again as well, because the engine's own batch is one of their points.
- [x] **And the default's batch size is published** for each of the three widths, by the width probe.
- [x] **And the old code was measured back to back with the new one**, three campaigns each, **interleaved**, on
      this host -- so the difference between the two sets is the code's and not the machine's.
- [x] **And the campaigns ran the binary the tree builds**: the first landing of the width, sweep and memory
      families carried a report without the shape's own name, because the release binary was built before the
      probe changed. Those three campaigns were taken again.
- [x] **And the campaign's own sets were corrected where the guard caught them** (amendment **A5**): the four
      thread-sweep families were taken again with their **six-thread point**, and the old code's join-parts set
      was taken again so that the report names **the code it measured**.
- [x] **And the sweep's "before" was attributed by measurement**: the same instrument against both codes, three
      campaigns each, interleaved, on this host. The code's share is under **0.2 ms**, and the set this file
      replaces reads **7 ms** more slowly than either -- **so the movement between the two published sets is the
      host's**, and it is published in [§3af](../../PERF.md#3af-the-batch-sizes-price).

## The published figures

- [x] Every figure that moves is published **old-and-new**, with the reason -- **the default moved** -- and
      **re-measured, not recomputed**.
- [x] The **guard** checks the new figures **inside the section that publishes them**, and the old ones are
      checked against the **before** reports.
- [x] **And the regression is published**: the engine's cold `q6` is **16.6 ms** where the old code's was
      **13.9 ms**, and the yardstick and the machine's own floor were flat -- so the movement is the code's. The
      reason is named: the rule gives the `q6`'s predicate set **1,000,000** rows at one worker, and that shape's
      scan curve turns up at a whole row group, with its minimum at **250,000** rows.

## The publication

- [x] A **new section** in `PERF.md`, the decision in an **ADR**, the status's next question rewritten, and the raw
      reports **committed**.
- [x] `bench-results/README.md` gains a row per new set, and `docs/status.html` is rebuilt.

## The gate

- [x] `cargo fmt`, clippy, the whole test suite, `cargo deny`, the wasm32 check, the published-figures guard, the
      links check, the package check and the dashboard check all green.
- [x] CI green, and the spec's status is `done` with the date it finished.

## What is not done

- **The byte budget's number.** It is **16 MiB**, and the rule's output is published for every width and every
  worker count, but **the measurements disagree**: the `q6`'s predicate set's scan curve turns up at a whole row
  group, and the join's probe path says the whole row group is the curve's minimum. A smaller budget would move
  the wide shape's batch below anything measured, and the scan's curve changes again when the reader owns the
  Parquet decode. **So the number is named, measured and left open** ([§3ah](../../PERF.md#3ah-the-default-moved)).
