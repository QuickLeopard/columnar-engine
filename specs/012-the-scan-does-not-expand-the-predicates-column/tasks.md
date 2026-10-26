# Spec 012 — Tasks

**Spec**: [012-the-scan-does-not-expand-the-predicates-column](spec.md)
**ADR**: [ADR-0018](../../docs/adr/0018-the-scan-reads-the-parquet-decode.md) · [ADR-0020](../../docs/adr/0020-the-expansion-is-not-the-decode-s-cost.md)
**Status**: done

## T1 — The measurement, before anything was written

- [x] The scan was decomposed: **the Parquet decode 10.4 ms**, the engine's own work **5.4 ms**, of a 15.7 ms scan.
- [x] The hypothesis — *the engine should read the file into its own column format, without Arrow* — was
      **prototyped as a full read path** and measured: the parquet crate's own typed column reader, the engine's
      kernel over the decoded buffer, the engine's own compaction into its own column.
- [x] The four measurements, on the same file, in the same process, warm, median of five: **A** the parquet crate's
      own decode **10.30 ms** · **B** Arrow's decode **9.86 ms** · **C** the whole shape without Arrow
      **15.73 ms** · **D** the engine's scan today **15.74 ms**.
- [x] **C is D.** The shape without Arrow is equal to the engine's scan. Arrow's layer is worth 0.3 ms.
- [x] The file's columns are **dictionary-encoded** — `l_quantity` is 1.67 bytes a row and `l_discount` is 0.50 —
      which says the decode is the **expansion** of six million RLE indices into six million values.
- [x] **The predicate's column is expanded, compared and thrown away**, and the engine already knows how to compare a
      column without expanding it: the dictionary path compares one distinct value at a time.

## T2 — The increment was refused, and the receipt published

- [x] **ADR-0018** records the rejected shape, the four measurements, and the reason: a rewrite in the read path for a
      measured zero is not a trade this repository makes.
- [x] The finding is published in `PERF.md` §3f and the README's "Next" row, so the next contributor reads the
      measurement rather than repeating the increment.

## T3 — The next increment

- [x] The share of the decode that is the **expansion** is measured, against the real file and the real predicates.
      The probe is `crates/columnar-engine/examples/rle_expansion_probe.rs`; it owns the Parquet format's RLE hybrid,
      and it is **checked against the parquet crate's own reader bit for bit** over all six million values of every
      row group before it measures anything.
- [x] The expansion is **1.83 ms** of the engine's **4.06 ms** decode of the predicate's column — **45%** of it, and
      **4.2%** of the **43.95 ms** query. The same column written **without a dictionary** decodes in **4.63 ms**
      against the dictionary's **3.94 ms**: the decode's cost is the bytes, and the expansion costs less than the
      forty-five megabytes it saves.
- [x] The shape the hypothesis proposes was **prototyped and measured anyway**, because a refusal without a measured
      alternative is an argument: the predicate compared once per distinct value, and the mask applied to the
      indices. It costs **7.79 ms** against the engine's own **5.42 ms** scan.
- [x] **It is not the cost**, and the increment is refused. The mask the hypothesis would build from the indices
      costs **1.80 ms** against the **1.36 ms** the vectorised predicate costs today — a float comparison vectorises
      and a bit test at a variable index does not — and the parquet crate does not expose the indices at all, so
      capturing the expansion means **owning the Parquet reading**: this spec's own non-goal, and ADR-0018's refusal.
      The probe's own decoder of the format is **0.99 ms slower** than the crate's, and that cancels most of what
      the increment would win. **The net is 0.40 ms on a 43.95 ms query.**
- [x] **ADR-0020** records the second refusal, the eleven measurements, and the three facts that decide it.
- [x] The measurement is published in `PERF.md` §3g, the raw reports are committed
      (`bench-results/rle-expansion-run{1,2,3}.json`), the table is derived by `scripts/medians.py`, and
      `scripts/check-published-figures.py` guards the figures that are quoted.
- [x] The probe's decoder has **nine tests**, including a dictionary whose values are **not sorted** and whose
      indices are **not monotonic**, and including the fact that **every row group has its own dictionary** — two row
      groups hold the same eleven values in a different order, which the probe's first attempt at the mask got wrong
      by 3,671 rows. **Ten mutations were run and each one was caught.**

## What T3 did not do, and why

T3's middle box — *the scan reads the predicate column's dictionary and indices* — was not implemented, because the
measurement on either side of it says the increment is not worth making. The shape was prototyped **in the probe**,
measured, and published; the engine's scan is unchanged, and it is unchanged for a reason a reader can check.
