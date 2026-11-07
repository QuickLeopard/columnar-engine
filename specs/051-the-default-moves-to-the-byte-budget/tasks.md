# Spec 051 — The default moves to the byte budget — tasks

**Status**: planned
**Started**: 2026-11-07
**Spec**: [spec.md](spec.md)

The order is the house's: **the rule is decided, the statement is built, and now the default moves.** The knob is
there, the price is measured, the statement is checked against the measurement -- and the engine still reads with
a row count.

## The engine

- [ ] The default's rule is a **byte budget of 16 MiB**, and the batch size is **the largest divisor of the row
      group's row count** whose bytes fit it.
- [ ] The budget is divided by the **columns the read fetches**, from the **width statement** -- so two different
      widths get two different batches.
- [ ] The batch still **divides the row group's rows**, so that no row group ends with a partial batch.
- [ ] **The knob still overrides it**: `with_reader_batch_rows` names a size in rows, and it is not the default.
- [ ] The estimate charges **the batch the read will use**, and the two come from the same place.
- [ ] `MAX_READER_BATCH_ROWS` leaves the public surface and `DEFAULT_READER_BATCH_BYTES` takes its place, and the
      semver baseline moves.

## The campaigns

- [ ] Every campaign that was taken with the engine's own batch size is **taken again**, three campaigns each.
- [ ] **And the figures that do not move are not re-taken** -- the sweeps, the width statement and the memory
      campaigns name their own batch sizes, and the eager path has its own.
- [ ] **And the default's batch size is published** for each of the three widths, by the width probe.

## The published figures

- [ ] Every figure that moves is published **old-and-new**, with the reason -- **the default moved** -- and
      **re-measured, not recomputed**.
- [ ] The **guard** checks the new figures **inside the section that publishes them**, and the old ones are
      checked against the **before** reports.

## The publication

- [ ] A **new section** in `PERF.md`, the decision in an **ADR**, the status's next question rewritten, and the raw
      reports **committed**.
- [ ] `bench-results/README.md` gains a row per new set, and `docs/status.html` is rebuilt.

## The gate

- [ ] `cargo fmt`, clippy, the whole test suite, `cargo deny`, the wasm32 check, the published-figures guard, the
      links check, the package check and the dashboard check all green.
- [ ] CI green, and the spec's status is `done` with the date it finished.
