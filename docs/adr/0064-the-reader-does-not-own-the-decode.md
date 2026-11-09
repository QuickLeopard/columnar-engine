# ADR-0064: The reader does not own the decode

**Status**: accepted
**Date**: 2026-11-09
**Related**: [ADR-0018](0018-the-scan-reads-the-parquet-decode.md), [ADR-0063](0063-the-default-moves-to-the-byte-budget.md), [spec 052](../../specs/052-the-reader-owns-the-decode/spec.md)

## Context

**The house promised the reader.** [PERF.md §3ah](../../PERF.md#3ah-the-default-moved) says that *the scan's curve is
about to change anyway, because the next increment makes the reader own the Parquet decode*, and it left **the byte
budget's number** open until that happens. [STATUS.md](../../STATUS.md)'s question 6 says the same. **And the promise was
made twice, on the strength of a decision taken before the code it describes existed.**

That decision was [ADR-0018](0018-the-scan-reads-the-parquet-decode.md), and it said **no**: *the scan's read is the
Parquet decode, and owning it is not worth it*. The rewrite was **prototyped** — the parquet crate's own typed column
reader, the engine's kernel over the decoded buffer, the engine's own compaction into its own column — and it came out
**equal** to what the engine already did. Arrow's whole layer was worth **0.3 ms** of a **15.7 ms** scan, and the decode
was the parquet crate's and **the floor**.

**And the refusal was stale.** It was taken on **2026-10-26**, against a tree that no longer exists, and **three things
moved since**, each of which touches the shape it measured:

- **the `parquet` crate bumped** from 54 to **58.4.1**, and the bump moved the decode;
- **the default's rule moved** from a row limit to a **byte budget**, so the engine's own batch for the narrow shapes is
  now a **whole row group** — and the old probe read batches of **4,096**, **16,384**, **65,536** and **262,144** rows:
  **it never read one**, and the whole row group is where the rule now puts the engine;
- **the pushdown gained a dictionary path**, and the shape ADR-0018 measured compared the **expanded** values.

**And the shape that could win had never been measured.** The idea ADR-0018 named and did not build: the predicate's
column is **dictionary-encoded**, so a reader that owns the decode would compare the **dictionary** — eleven values —
once, and select the rows **by the indices**, and the expansion of six million floats would never happen.

## The measurement

**Six shapes**, on the same file, in the same process, warm, three campaigns each, at a load average of **6.02**. The
full tables are in [PERF.md §3ai](../../PERF.md#3ai-the-reader-owns-the-decode).

| | the median | against the engine's own scan |
|---|---|---|
| **D** — the engine's own scan | **12.435 ms** | *the reference* |
| **E3** — the engine's own reader, **row group by row group** | **17.392 ms** | **+4.957 ms** |
| **E4** — the same, **with the expansion** | **18.500 ms** | +6.244 ms |
| **E** — the engine's own reader, **the whole file's** | **17.812 ms** | +5.529 ms |
| **E2** — the same, **with the expansion** | **19.202 ms** | +6.633 ms |

**And every difference is the median of the campaigns' own paired differences**: the two sides are measured in the same
campaign, so the difference is the shape's and not the machine's.

**So the reader is not worth building, and the reason is priced.** The reader is **4.957 ms slower** than the engine's
own scan. The idea it carries — **never expanding the predicate's column** — is worth **1.141 ms** inside it (E4 − E3).
**The shape's own handicap is 0.453 ms** (E − E3): the whole file's mask, and the file read in two passes instead of one.
**So the reader costs more than three times the prize it carries.**

**And the mechanism is named.** The parquet crate's decode is **one pass**: it reads the indices and gathers the values
in the same loop, and hands the engine an expanded array. **The engine's own reader is three passes**: the indices, the
dictionary's answer mapped through them, and then the survivors gathered. **And the map is a scalar lookup a row where
the expansion is a vectorised comparison** — which is why the engine's own dictionary path exists for strings, and why
the house's own history says it *cost 41 ms before it was fixed*. **The expansion is not the expensive part; the map that
would avoid it is dearer.**

**And ADR-0018's refusal holds.** The old shape C is **13.108 ms** where the engine's own scan is **12.291 ms**, at the
batch the rule now produces — **0.840 ms** of Arrow's layer, paired over the campaigns, where the old ADR measured the
same thing on a different day and a different crate.

## Decision

**Rejected. The engine keeps Arrow, and the decode stays the parquet crate's.**

The rewrite is **not** built, because it is **4.957 ms slower** than what the engine already does, and the prize it
carries is **1.141 ms**. Paying a large rewrite, in the read path, for a measured loss is not a trade this repository
makes.

**And the instrument is checked against the reference it measures against.** Every shape's survivors — **2,178,861
rows** — are compared, value by value, with the engine's own scan's, and the probe stops if they disagree. **The engine's
own reader is not trusted to answer about itself.**

## Consequences

- **The byte budget's number is settled, and it stays 16 MiB.** It was left open *until the reader owns the decode*, and
  the decode **is not going to move**: the reader is **4.957 ms slower**, and the parquet crate's decode is what the
  engine reads through. **The number is not waiting for the reader — it is settled.**
- **The sentences that promised the reader are corrected where they were published.** [PERF.md §3ah](../../PERF.md#3ah-the-default-moved)'s
  *the next increment makes the reader own the Parquet decode* is **corrected**, and [STATUS.md](../../STATUS.md)'s
  question 6 with it. **The correction is recorded here.**
- **The decode is still the floor, and the engine's own work is still the only part the engine can move** — which is
  ADR-0018's finding, now measured on the code that is here now, at the batch the rule now produces.
- **The plain string's copy is not fixed.** The conversion copies every string out of the Arrow batch, one allocation a
  row, and the dictionary path borrows instead. **That is the wide shape's gap**, and the fix wants its own measurement.
- **The reader is not built, and the crates.io publish is not waiting on it.** The crates stay unpublished while the
  remaining work lands.
