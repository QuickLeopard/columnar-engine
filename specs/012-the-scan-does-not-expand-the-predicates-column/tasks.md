# Spec 012 — Tasks

**Spec**: [012-the-scan-does-not-expand-the-predicates-column](spec.md)
**ADR**: [ADR-0018](../../docs/adr/0018-the-scan-reads-the-parquet-decode.md)
**Status**: in progress

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

- [ ] The share of the decode that is the **expansion** is measured, against the real file and the real predicates.
- [ ] If the expansion is the cost, the scan reads the predicate column's **dictionary and indices** and compares the
      dictionary — and the mask is applied to the indices.
- [ ] If it is not, the hypothesis is refused and the measurement is published, the way this one was.
