# Spec 052 — The reader owns the decode

**Status**: done
**Started**: 2026-11-09
**Finished**: 2026-11-09
**Predecessor**: [spec 051 — the default moves to the byte budget](../051-the-default-moves-to-the-byte-budget/spec.md)

## The problem

**The house has promised the reader.** [PERF.md §3ah](../../PERF.md#3ah-the-default-moved) says that *the scan's curve
is about to change anyway, because the next increment makes the reader own the Parquet decode*, and it leaves **the byte
budget's number** open until that happens. [STATUS.md](../../STATUS.md)'s question 6 says the same. **And the promise was
made twice**, in two documents, on the strength of a decision that was taken before the code it describes existed.

**The decision was ADR-0018, and it said no.** *The scan's read is the Parquet decode, and owning it is not worth it.*
The rewrite was **prototyped** — the parquet crate's own typed column reader, the engine's kernel over the decoded
buffer, the engine's own compaction into its own column — and it came out **equal** to what the engine already did:

| | | |
|---|---|---|
| **A** | the parquet crate's own decode of the two columns, into plain Rust buffers | **10.30 ms** |
| **B** | Arrow's decode of the same two columns | **9.86 ms** |
| **C** | A, plus the engine's predicate over the decoded buffer, plus the compaction of the survivors | **15.73 ms** |
| **D** | the engine's own scan, as it is today | **15.74 ms** |

**C is D.** Arrow's whole layer is worth **0.3 ms** of a **15.7 ms** scan, and the decode itself is the parquet crate's
and is **the floor**. **So the rewrite was rejected** — and the half of it that was worth doing anyway became spec 012,
*the scan does not expand the predicate's column*, which was measured too and **also refused**: the expansion of six
million RLE indices costs **1.77 ms** where the set the bump replaces had **1.79 ms**.

**And the refusal is stale.** It was taken on **2026-10-26**, against a tree that no longer exists. **Three things moved
since**, and every one of them touches the shape ADR-0018 measured:

- **The `parquet` crate bumped** from 54 to **58.4.1** (spec 041), and the bump moved the decode: Arrow's own decode of
  the column is **4.26 ms** now, where the set the bump replaces had **4.21 ms**.
- **The default's rule moved** from a row limit to a **byte budget** (spec 051), so the engine's own batch for the narrow
  shapes is now a **whole row group** — **1,000,000** rows — and **the batch is what the decode's cost turns on**. ADR-0018's
  probe read batches of **4,096**, **16,384**, **65,536** and **262,144** rows: **it never read a whole row group**, and
  the whole row group is where the rule now puts the engine.
- **The engine's pushdown gained a dictionary path**, so the predicate's column is compared through the dictionary's
  values — a path the old probe's shape **C** did not have, because C compared the **expanded** values.

**So the question is open again, and the instrument that answers it is already in the tree.**
`crates/columnar-engine/examples/own_decode_probe.rs` measures A, B, C and D on the same file, in the same process, warm.
**It is the shape ADR-0018 was decided on, and it has never been run against the code that is here now.**

## The requirements

### R1 — The refusal is **re-measured**, on the code that is here now

- [x] The probe's **A, B, C and D** are taken again, **three campaigns each**, on this host, and the figures are
      **re-measured, not recomputed**.
- [x] **And the probe reads the batch sizes the rule now produces** — including the **whole row group**, which the old
      probe never read, because that is where the default now sits.
- [x] **And the old figures are published beside the new ones**, with the reason named: **the code moved**, and the
      `parquet` crate's bump and the default's rule are the two reasons.

### R2 — The **shape** is measured against the engine's own scan, **at the engine's own batch**

- [x] The comparison is **paired**: the engine's own scan is measured **in the same iteration** as the shape it is
      compared against, so the difference is the shape's and not the machine's.
- [x] **And the machine's own floor is published beside them**, so that a difference inside the noise is called what it
      is.

### R3 — The **verdict** is published either way, and **the budget's number is settled against it**

- [x] If the shape is **dearer or equal**, the reader is **not** built, the refusal is **re-published with today's
      numbers**, and **the question is closed** — the decode is the parquet crate's, and it is the floor. **And it was
      dearer**: the engine's own reader is **4.957 ms slower** than the engine's own scan, and the idea it carries is
      worth **1.141 ms** inside it.
- [x] And if the shape is **cheaper**, the reader **is** built, and the build is the increment's second half.
- [x] **And the byte budget's number is settled against the measurement either way**: a budget that is chosen against a
      decode that is not going to move is a settled number, and one that is chosen against a decode that **is** going to
      move is not.

### R4 — The **published sentences** that promised the reader are **corrected where they were published**

- [x] [PERF.md §3ah](../../PERF.md#3ah-the-default-moved)'s *the next increment makes the reader own the Parquet
      decode* is **corrected** if the reader is not built, and [STATUS.md](../../STATUS.md)'s question 6 with it.
- [x] And the correction is **recorded in the ADR**, as the house records every correction for an in-flight increment.

### R5 — The answer is published

- [x] A **new section** in `PERF.md`, the decision in an **ADR**, the status's next question rewritten, and the raw
      reports **committed**.
- [x] And the **guard** checks the published figures **inside the section that publishes them**.

## The scope

- **In**: the Parquet decode's own cost, on the code that is here now, at the batch sizes the rule produces; and the
  verdict on whether the engine's own reader is worth building.
- **Out**: **the reader itself**, unless the measurement says it is worth building. **The house's rule is measure before
  implement**, and ADR-0018 is the precedent: the shape was measured before the rewrite was written.
- **Out**: **the crates.io publish**. The crates stay unpublished while this work lands, so a break that the work
  implies costs nothing.
- **Out**: **the cold path's context** — that is **spec 053**, and this increment does not touch it.

## The instrument

**`crates/columnar-engine/examples/own_decode_probe.rs`** — the shape ADR-0018 was decided on, **in the tree since
2026-10-26** and **never run against the code that is here now**. It measures A, B, C and D on the same file, in the same
process, warm.

**And it grows two things**: the **batch sizes the rule now produces**, and a **JSON report**, so that the figures are
derived from a committed report by `scripts/medians.py` and checked by the guard — the way every other published figure
in this repository is.

## The references

- [ADR-0018](../../docs/adr/0018-the-scan-reads-the-parquet-decode.md) — the decision this increment re-measures: **the
  shape, the numbers, and the refusal**.
- [PERF.md §3ah](../../PERF.md#3ah-the-default-moved) — the promise this increment either keeps or corrects.
- [ADR-0063](../../docs/adr/0063-the-default-moves-to-the-byte-budget.md) — the budget's number, and why it was left
  open.
- [PERF.md §2h](../../PERF.md#2h-the-hot-paths-taken-apart) — the hot paths, taken apart.
- [constitution.md](../../constitution.md), principle **IV — Evidence Over Claims**: a number is a claim about a
  measurement, and a claim without its evidence is not published.
- [constitution.md](../../constitution.md), principle **VII — Explicit Scope, No Silent Deferral**: the reader's own build
  is **named** as conditional on the measurement, rather than left as an omission.

## The amendments

### A1 — The refusal is **re-measured** before the reader is **built**

**Why**: ADR-0018 did not refuse the idea on principle. It **prototyped** the rewrite, measured it, and found it equal.
**The measurement is what decided it, and the measurement is stale**: the `parquet` crate bumped, the default's rule
moved, and the pushdown gained a dictionary path. **A decision made on a measurement is only as good as that
measurement**, and this one was taken on a tree that no longer exists.

**So the shape is measured first, on the code that is here now, at the batch sizes the rule now produces.** The reader is
built **only if the measurement says it is worth building** — and that is the house's rule, and ADR-0018's own precedent.

### A2 — The old probe's **batch sizes** are not the rule's, and the difference is the point

**Why**: ADR-0018's probe read **4,096**, **16,384**, **65,536** and **262,144** rows. **The rule now gives the narrow
shapes a whole row group** — **1,000,000** rows — and the join's probe path says the whole row group is **the minimum of
that curve**. **A decode measured only at small batches cannot say what a whole-row-group batch costs**, and that is the
batch the engine now reads with.

**So the probe reads the rule's own batch sizes**, and the table says which is which.
