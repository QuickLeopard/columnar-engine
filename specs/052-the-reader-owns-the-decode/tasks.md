# Spec 052 — The reader owns the decode — tasks

**Status**: done
**Started**: 2026-11-09
**Finished**: 2026-11-09
**Spec**: [spec.md](spec.md)

The order is the house's: **the shape is measured before the reader is built.** ADR-0018 decided this question with a
measurement, and that measurement was taken on a tree that no longer exists -- so the measurement is taken again, and the
reader is built **only if the measurement says it is worth building**.

**And the measurement said no.** The engine's own reader is **4.957 ms slower** than the engine's own scan, and the idea
it carries is worth **1.141 ms** inside it. **So the reader is not built, and the question is closed.**

## The measurement

- [x] The probe's **A, B, C and D** are taken again, **three campaigns each**, on this host, and the figures are
      **re-measured, not recomputed**.
- [x] **And the probe reads the batch sizes the rule now produces** -- the whole row group among them, which the old
      probe never read.
- [x] **And the comparison is paired**: the engine's own scan is measured **in the same iteration** as the shape it is
      compared against, and every difference is the **median of the campaigns' own paired differences**.
- [x] **And the old figures are published beside the new ones**, with the reason named: **the code moved**, and the
      `parquet` crate's bump and the default's rule are the two reasons.
- [x] **And the shape ADR-0018 did not measure is measured too**: **the engine's own reader**, with the page walk, the
      dictionary and the indices, and **no expansion at all** -- the predicate's column compared through its
      **dictionary**, and the rows selected **by the indices**. **And in the row group's own shape**, so the whole file's
      mask and the two passes are measured rather than assumed.

## The instrument

- [x] The probe writes a **JSON report**, the way every other published figure's instrument does, with the host's load
      average and the batch sizes named.
- [x] **And the report is checked against the printed run**, so that a figure cannot be published from a report the
      probe did not write.
- [x] **And every shape's survivors are compared, value by value, with the engine's own scan's** -- **2,178,861 rows**
      -- and the probe stops if they disagree. **The engine's own reader is not trusted to answer about itself.**

## The verdict

- [x] The reader is **dearer**, so it is **not built**, the refusal is **re-published with today's numbers**, and **the
      question is closed**.
- [x] **And the byte budget's number is settled against the measurement**: the decode is not going to move, so the
      number is settled, and it stays **16 MiB**.
- [x] **And the mechanism is named**: the parquet crate's decode is **one pass**, the engine's own reader is **three**,
      and **the map that would avoid the expansion is dearer than the expansion**.

## The published figures

- [x] Every figure that moves is published **old-and-new**, with the reason -- **the code moved** -- and **re-measured,
      not recomputed**.
- [x] The **guard** checks the new figures **inside the section that publishes them**, and the old ones are checked
      against the **before** reports.

## The publication

- [x] A **new section** in `PERF.md`, the decision in an **ADR**, the status's next question rewritten, and the raw
      reports **committed**.
- [x] `bench-results/README.md` gains a row per new set, and `docs/status.html` is rebuilt.
- [x] **And the sentences that promised the reader are corrected where they were published** -- [PERF.md §3ah](../../PERF.md#3ah-the-default-moved)
      and [STATUS.md](../../STATUS.md)'s question 6 -- and the correction is recorded in the ADR.

## The gate

- [x] `cargo fmt`, clippy, the whole test suite, `cargo deny`, the wasm32 check, the published-figures guard, the links
      check, the package check and the dashboard check all green.
- [x] CI green, and the spec's status is `done` with the date it finished.

## What is not done

- **The reader itself** -- the measurement refused it, and **a measurement that says "no" is worth more than the
  increment it refuses**.
- **A file with a page index.** That is what is left of the `filter-range` question, and it wants its own file.
- **The plain string's copy**, which is still not fixed: the conversion copies every string out of the Arrow batch, one
  allocation a row, and the dictionary path borrows instead. **That is the wide shape's gap**, and the fix wants its own
  measurement.
- **The crates.io publish**, which is a separate act and needs a token this repository does not have.
