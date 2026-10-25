# Spec 008 — Tasks

**Spec**: [`spec.md`](spec.md)
**Status**: **done.** Every requirement below is met and checked.

Legend: `[x]` done · `[ ]` open · `[~]` partially done, and the gap is written down.

---

## T1 — The measurement, before anything was written

- [x] The decode was split in two: Arrow's own decode with no engine conversion (**376.1 ms**)
      and the engine's Arrow→chunk conversion on top of it (**823 ms**).
- [x] **The bottleneck was never Arrow.** The conversion was 69% of the cost, and its dominant
      cost is per batch, per column — every batch rebuilds each dictionary column's table of
      merged dictionary slots.
- [x] The finding decided the shape of the work: the conversion's cost parallelises, so the
      parallel decode was worth doing — but it also scales with the **number of batches**, which
      is where the first defect was.

## T2 — The batch size

- [x] **The eager path used the Parquet reader's default of 1,024-row batches.** The lazy path
      had already moved to 8,192 with a measurement behind it; the eager path, which reads
      **sixteen** columns, was never touched.
- [x] The controlled comparison — same binary, same process shape, one change at a time: the
      one-thread eager read went from **1145.6 ms to 587.0 ms**.
- [x] The constant was then **measured** rather than guessed, over the real file and all sixteen
      columns, at six values. The optimum is broad and 32,768 is its middle.
- [x] `EAGER_BATCH_ROWS` is the eager path's own constant, and the table that chose it is in its
      documentation.

## T3 — The parallel eager decode

- [x] The file's **row groups** are split into contiguous blocks, one per worker, and the parts
      are merged afterwards **in block order**.
- [x] The workers are capped by the file's row groups, exactly as the lazy read's are.
- [x] One thread is the serial path, and the serial path is the path that was always there.

## T4 — The merge

- [x] The merge is **one pass**, not a fold. The old fold copied the accumulated result once per
      part, so the work grew with the **square** of the number of parts — and for the validity
      masks it was an allocation storm rather than a copy: a 750 KB mask allocated afresh on
      every step.
- [x] `NullBitmap::concat_all` does it in one pass: one allocation, one copy of each mask, the
      same mid-word shift. **The six-thread read went from 366.7 ms to 276.4 ms.**
- [x] The merge **moves** the parts' values rather than copying them, so the peak memory of a
      parallel eager decode is about **one** copy of the data rather than two.
- [x] The dictionary remap is a table lookup per **distinct value**, not a string hash lookup per
      row.

## T5 — The NULL rows' indices

- [x] **A NULL row's dictionary index is undefined** — the engine's convention is slot zero, and
      the validity mask is what says the row is NULL.
- [x] The merge got this wrong at first: it repointed the placeholder index like a live reference,
      so a NULL row ended up pointing at a real dictionary slot. **No answer changed** — the
      validity mask still said the row was NULL — but the chunk's bytes were no longer the bytes
      the serial read produced.
- [x] The merge now consults the part's validity mask, and a NULL row's index is set to zero
      rather than remapped.

## T6 — The tests

- [x] The parallel eager read is **bit-identical** to the serial one: the same rows, in the same
      order, with the same values, the same validity bits and the same dictionary.
- [x] The fixture's row groups hold **different** dictionary values. That is the point of the
      fixture: with identical dictionaries the merge's remap is the identity, and a merge that
      forgot to repoint the indices would still pass. The first version of the fixture was wrong
      in exactly that way, and it was fixed rather than the mutation being excused.
- [x] The merged dictionary holds the values in **first-appearance order**, and the merged rows
      hold the values they held before the merge.
- [x] The answer does not depend on the thread count.
- [x] `NullBitmap::concat_all` is pinned against the pairwise `concat` at every alignment, and
      against one mask over the same rows.
- [x] The tests are **mutation-verified**: reversing the merge order fails 2 of them, repointing
      the NULL rows' indices fails 2, not repointing the indices at all fails 4, and dropping the
      validity concat's shift fails 3 engine tests and 2 core tests.

## T7 — The measurement

- [x] The load is measured at every thread count, and **the incumbents are measured beside it**:
      the engine and both incumbents load the same file at the same thread counts, in the same
      harness.
- [x] The sweep gains a **load** mode, and the report says which mode it is.
- [x] The headline table was re-measured, because the committed reports carry the load figure.

## T8 — The follow-ons, not done

- [ ] **The morsel driver** — overlapping the read with the operators — is not here. The read's
      workers and the operators' workers are the same budget, used in sequence.
- [ ] **The merge is still serial.** It has to be, and at six parts it is about a tenth of the
      decode.
- [ ] **The eager path still decodes every column.** That is what the eager path is; the lazy
      path is the answer to it, and the comparison publishes both.
- [ ] **The x86-64 measurement** stays blocked: there is no x86-64 machine.
