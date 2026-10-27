# Spec 023 — The scan, taken apart, and the scan that feeds the join — tasks

**Status**: done
**Spec**: [spec.md](spec.md)

## The instrument

- [x] `crates/columnar-engine/examples/scan_cost_probe.rs`, a new diagnostic that measures the
      join's scan in the shapes it is made of: the crate's read of each column alone and of both
      together, the engine's own scan of the same, and the build side's read.
- [x] The batch size is **asked of the engine**, not copied from a rule, so the probe's shapes are
      the engine's shapes.
- [x] The engine's copy is **derived**: the difference between the engine's shape and the crate's
      shape over the same two columns.
- [x] Three campaigns on the join's file, the reports committed as
      `bench-results/scan-cost-run{1,2,3}.json`.

## The separation

- [x] The scan is taken apart: the Parquet decode is **66.54 ms** and the engine's own work over
      it is **6.49 ms**.
- [x] The waste is named: the build side holds **216,345** keys, the probe side's **6,001,215**
      rows produce **216,783** output rows, and **96.4%** of what the engine copies is thrown away.
- [x] The engine's copy is **96 MB**; the copy of the rows that survive is **3.5 MB**.

## The change

- [x] The scan **hands every batch to the join before it copies anything**: the probe reads the
      key where it lies, in the reader's own Arrow array, and only the rows the join keeps are
      written into the dataset.
- [x] The probe is **one implementation** over one key interface — the engine's own column and the
      reader's Arrow array — so the streamed scan and the materialising scan cannot answer
      differently. The NULL rule and the NaN rule live in the shared lookup.
- [x] The join's table is built **before** the scan when the scan feeds the join: the probe has to
      happen before the copy, and the probe needs the table.
- [x] The join's two keys must have the same type, and the check happens **before** the table is
      built: a plan error is refused having read nothing at all.
- [x] The plan output says what happened: the scan's rows out is the rows the scan kept, and the
      join's line says the probe ran inside the scan.

## The fallbacks, named

- [x] A plan whose **probe side has a predicate of its own** takes the materialising scan: the
      predicate would have to be evaluated over every row of the file, and that trade is not
      measured.
- [x] A join key whose Arrow type the probe cannot read takes the materialising scan.
- [x] The build side is not streamed, and the probe side's rows are still materialised. Both are
      in the spec's scope, with their reasons.

## The answer does not change

- [x] A test holds the streamed scan and the materialising scan to the same answer, row for row,
      and asserts that the scan fed the join.
- [x] A join whose two keys have different types is a plan error on **both** paths, and the lazy
      path refuses it before it reads anything.

## The publication

- [x] `PERF.md` §3l: the scan taken apart, the waste, the change, and the before-and-after.
- [x] `bench-results/README.md`: the new reports, and the recipe that writes them.
- [x] `scripts/medians.py` derives the new tables; `scripts/check-published-figures.py` checks
      every published figure against the committed reports.
- [x] The comparison campaigns re-measured, for the code and for the code it replaced, back to back
      on the same host.
