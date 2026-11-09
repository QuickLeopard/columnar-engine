# Spec 052 — The reader owns the decode — tasks

**Status**: planned
**Started**: 2026-11-09
**Spec**: [spec.md](spec.md)

The order is the house's: **the shape is measured before the reader is built.** ADR-0018 decided this question with a
measurement, and that measurement was taken on a tree that no longer exists -- so the measurement is taken again, and the
reader is built **only if the measurement says it is worth building**.

## The measurement

- [ ] The probe's **A, B, C and D** are taken again, **three campaigns each**, on this host, and the figures are
      **re-measured, not recomputed**.
- [ ] **And the probe reads the batch sizes the rule now produces** -- the whole row group among them, which the old
      probe never read.
- [ ] **And the comparison is paired**: the engine's own scan is measured **in the same iteration** as the shape it is
      compared against, and the machine's own floor is published beside them.
- [ ] **And the old figures are published beside the new ones**, with the reason named: **the code moved**, and the
      `parquet` crate's bump and the default's rule are the two reasons.

## The instrument

- [ ] The probe writes a **JSON report**, the way every other published figure's instrument does, with the host's load
      average and the batch sizes named.
- [ ] **And the report is checked against the printed run**, so that a figure cannot be published from a report the
      probe did not write.

## The verdict

- [ ] If the shape is **dearer or equal**, the reader is **not** built, the refusal is **re-published with today's
      numbers**, and **the question is closed**.
- [ ] And if the shape is **cheaper**, the reader **is** built, and the build is the increment's second half.
- [ ] **And the byte budget's number is settled against the measurement either way**.

## The published figures

- [ ] Every figure that moves is published **old-and-new**, with the reason -- **the code moved** -- and **re-measured,
      not recomputed**.
- [ ] The **guard** checks the new figures **inside the section that publishes them**, and the old ones are checked
      against the **before** reports.

## The publication

- [ ] A **new section** in `PERF.md`, the decision in an **ADR**, the status's next question rewritten, and the raw
      reports **committed**.
- [ ] `bench-results/README.md` gains a row per new set, and `docs/status.html` is rebuilt.
- [ ] **And the sentences that promised the reader are corrected where they were published** -- [PERF.md §3ah](../../PERF.md#3ah-the-default-moved)
      and [STATUS.md](../../STATUS.md)'s question 6 -- and the correction is recorded in the ADR.

## The gate

- [ ] `cargo fmt`, clippy, the whole test suite, `cargo deny`, the wasm32 check, the published-figures guard, the links
      check, the package check and the dashboard check all green.
- [ ] CI green, and the spec's status is `done` with the date it finished.

## What is not done

- **The reader itself**, unless the measurement says it is worth building. **And the crates.io publish**, which is a
  separate act and needs a token this repository does not have.
