# Spec 047 — The build side's keys as a filter — tasks

**Status**: implemented
**Started**: 2026-11-06
**Spec**: [spec.md](spec.md)

The order is the house's: **measure, then publish.** Nothing in the engine changes until the measurement
has said what to change — and if the measurement says **no**, nothing in the engine changes at all.

## The measurement comes first

- [x] The **hypothesis** is written down before the instrument is built: a filter one bit a key, over the
      range the table already addresses, rejects a probe row for less than the table's own load costs,
      and the probe gets faster.
- [x] The instrument is built **against the hypothesis**, and the hypothesis is **not** built into it: both
      shapes are measured, and the measurement says which one wins — if either does.
- [x] **Two shapes** are measured, not one: the filter **in front of the table**, and the filter
      **compacting the rows** in front of the probe. The second is the shape in which the filter can keep
      the rows out of the probe's own work, and it is the shape the prize is about.
- [x] The **floors** are measured in the iteration: the table's own load, one a probe row, and the
      filter's own load, one a probe row.
- [x] The **constructions** are measured in the iteration: the table's own construction, and the filter's
      own construction. A filter's price is its test **and** the building of it.
- [x] The **prize** is measured: the probe over the survivors alone, against the probe as it is.
- [x] The **control**: the candidates are the engine's own probe, line for line, and the engine's own
      probe is measured **in the iteration**, beside them. A candidate that is not the engine's probe
      would price a shape the engine does not have.
- [x] The **cross-check**: the candidates' rows are compared with the engine's own probe's rows, **row
      for row, in order**, in every iteration, and the comparison is asserted before anything is timed.
- [x] The probe side's column is **materialised afresh, every iteration**: a column held warm across the
      iterations would be a column the engine does not pay for.
- [x] Every candidate is measured **in the iteration**, and the two sides of every difference are
      measured **back to back**, with the **order alternated** — and the six orders are chosen so that
      **every pair** is back to back in four of the six iterations, two in each direction.
- [x] The **three campaigns** are taken, the raw reports are **committed**, and the published table is
      derived from them by `scripts/medians.py`.

## The change

- [x] **No change to the engine.** The measurement decides what is built, and the decision belongs to the
      increment that follows this one.
- [x] The instrument lives in `columnar-engine`, beside the house's other probes, and it adds **no
      dependency**.

## The answer does not change

- [x] The engine's own answer is the published one, and the join's rows, their order and their count are
      untouched — nothing in the engine changed.
- [x] The filter's contract is checked by the instrument in every iteration, and not only by the rows:
      the filter's rows are the engine's rows, so **no key the build side holds was refused**.

## The publication

- [x] The parts are published in `PERF.md` under a **new** section, with **every figure that moved**
      published **old-and-new** and the reason.
- [x] The **hypothesis** is published as it came out — confirmed or **refused** — and a refused
      hypothesis is published with the numbers that refuse it.
- [x] The **next question** is written where this one was: `STATUS.md`'s last section.
- [x] `scripts/medians.py` derives the new table, and `scripts/check-published-figures.py` checks the
      published figures against the committed reports. A set with **fewer than three campaigns fails**,
      and a family is never skipped silently.
- [x] `bench-results/README.md` gains a row per new set.
- [x] This spec's ADR records the decision, and the raw reports are committed.

## The gate

- [ ] `cargo fmt`, clippy on arm64 and x86-64, the whole test suite, `cargo deny`, the wasm32 check, the
      published-figures guard, the links check, the package check and the dashboard check all green.
- [ ] CI green, and the spec's status is `done` with the date it finished.
