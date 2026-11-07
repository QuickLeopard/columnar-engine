# Spec 048 — The probe, twice — tasks

**Status**: done
**Started**: 2026-11-07
**Spec**: [spec.md](spec.md)

The order is the house's: **the knob, then the measurement, then the publication.** The measurement decides
what is built, and the knob is a **measurement seam**: the engine's default does not move, so every
published figure stays reproducible by the command that made it.

## The knob, first

- [x] The **reader's batch size** is a knob on `ParquetSource`: `with_reader_batch_rows` names a size in
      rows, and the engine's own is what nothing named.
- [x] **The default does not move**: `MAX_READER_BATCH_ROWS` stays the default, so every published figure
      stays reproducible by the command that made it.
- [x] The knob reaches the **query** — `--reader-batch-rows` on the command that runs a query, and on the
      one that takes the query's parts.
- [x] The knob's doc says what a named size costs when it leaves a **partial batch** at the end of a row
      group.
- [x] **The public surface does not break**: the knob is an addition on a type with no public fields,
      `cargo-semver-checks` is clean, and the baseline does not move.
- [x] A **test** that the knob's rows arrive in batches of that size, and that the answer does not move.

## The measurement, then

- [x] The **two paths** are measured **in the same iteration**, **back to back**, with the **order
      alternated**: the probe over a materialised column, and the probe inside the reader.
- [x] Both paths are **the engine's own `probe_rows`**, and the rows are compared with the engine's own
      probe's rows, **row for row, in order**, in every iteration.
- [x] The **reader's path is swept** over four batch sizes, including the engine's own, and the points are
      named in rows. The points run in a **rotation of four twice over**, so every point takes every
      position exactly twice.
- [x] The **query's own parts** are taken at the swept batch sizes, so the published figure is the
      query's and not only the probe's.
- [x] The **three campaigns** are taken, the raw reports are **committed**, and the published table is
      derived from them by `scripts/medians.py`.
- [x] The **same probe is measured three times in the ladder's path** — cold, resident, and after the
      reader's work — so that the build table's residency is **measured** and not assumed.

## The answer either way

- [x] **A real gap** is published with its parts and its mechanism, and the open question is rewritten.
- [x] The parts are published in `PERF.md` under a **new** section, with **every figure that moved**
      published **old-and-new** and the reason — §3ac's join part is not corrected, and the spread is
      published with it.
- [x] `scripts/check-published-figures.py` checks the published figures against the committed reports, and
      the family fails on fewer than three campaigns.
- [x] `bench-results/README.md` gains a row per new set, and `docs/status.html` is rebuilt.
- [x] The spec's ADR records the decision, and the raw reports are committed.

## The gate

- [x] `cargo fmt`, clippy, the whole test suite, `cargo deny`, the wasm32 check, the published-figures
      guard, the links check, the package check and the dashboard check all green.
- [ ] CI green, and the spec's status is `done` with the date it finished.
