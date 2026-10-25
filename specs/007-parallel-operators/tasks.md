# Spec 007 — Tasks

**Spec**: [`spec.md`](spec.md)
**Status**: **done.** Every requirement below is met and checked.

Legend: `[x]` done · `[ ]` open · `[~]` partially done, and the gap is written down.

---

## T1 — The harness defect, found first

- [x] The `--threads` flag reached the **read** and the lazy path, and **nowhere else**: the
      resident path passed `ExecOptions::default()`, whose `threads` is `None`, and `None` means one
      thread. The operators could not have been parallel through it.
- [x] The symptom was **no scaling at all** — 13.5 ms at one thread and 13.8 ms at six — and the
      obvious reading ("the kernel does not scale") was wrong.
- [x] A probe that called the filter directly, with the thread count as an explicit argument, showed
      6.9 ms falling to 2.0 ms: **3.5×**, from the same code on the same data in the same process.
      The probe was deleted afterwards; the diagnosis is in
      [ADR-0013](../../docs/adr/0013-parallel-operators.md).

## T2 — The parallel filter

- [x] The rows are split into **contiguous, word-aligned blocks**, one per worker. The block starts
      are multiples of 64 rows, so each block's match mask is a whole number of words and the masks
      concatenate without a shift.
- [x] The masks are concatenated in **block order**, not completion order — the handles are joined
      in the order they were spawned.
- [x] The block's rows are **sliced**, not copied: the values, the dictionary indices and the
      validity mask are all addressed from the block's start.
- [x] The **validity** mask is the one thing that has to be copied, because a packed bitmap has no
      offset. The copy is a word at a time, and it is the block's, not the chunk's.
- [x] The **serial** path borrows the chunk's validity mask instead of copying it, so the one-thread
      path is not paying for the parallel path's machinery. (The first version copied it in both
      paths; that was a regression for nullable columns and it was fixed before anything was
      measured.)

## T3 — The parallel projection

- [x] The **source rows** are split into contiguous blocks — not the survivors — so the work is even
      by construction whatever the predicate's selectivity is.
- [x] The blocks are at least 4,096 rows: a gather is a copy, and a copy does not need the mask's
      alignment discipline.
- [x] Each block's survivors are gathered independently, and the parts are concatenated in block
      order.
- [x] The validity masks of the parts are concatenated with the mid-word shift that
      `NullBitmap::concat` does — the function spec 005 found the bug in.

## T4 — The aggregate, and the expression evaluation

- [x] **The aggregate is not parallelised, and never will be.** `SUM` is not associative, so a
      parallel aggregate's answer would depend on the block boundaries, and the block boundaries
      depend on the thread count. This is a permanent property, not a deferral.
- [x] **The expression evaluation is not parallelised.** It is 0.2 ms of a 15 ms resident query, and
      parallelising it would mean slicing the columns into blocks — a copy per column.

## T5 — The one-thread path

- [x] One thread runs the **serial** code, not a parallel path with one worker. There is no second
      implementation to keep in agreement.
- [x] The one-thread figures are bit-for-bit the figures the previous increments published.

## T6 — The plan output

- [x] The plan reports the workers the operators **ran**: `operator workers: N filter, M projection`.
- [x] The two are **separate numbers**, because they are separate — the filter's blocks are 64-row
      aligned and the projection's are at least 4,096 rows, so on a 20,000-row dataset with 64
      threads the filter runs 63 workers and the projection runs 5.
- [x] The count is the workers that ran, not the threads that were asked for. The read's count was
      already this honest; the operators' is now too.

## T7 — The tests

- [x] The parallel filter is **bit-identical** to the serial one: the same rows, in the same order,
      at every thread count, over plain integers, plain floats, dictionary-encoded integers,
      dictionary-encoded strings and NULLs in every column.
- [x] The parallel gather holds the same values, in the same order, with the same validity bits.
- [x] The answer does not depend on the thread count, including for a floating-point sum.
- [x] A dataset with fewer rows than a filter block has one block, whatever the thread count.
- [x] The tests are **mutation-verified**: reversing the filter's merge order fails 5 of them,
      reversing the gather's merge order fails 3, and making the filter's blocks one row too long —
      so they are no longer word-aligned — fails 4.

## T8 — The measurement

- [x] The thread-count sweep gains a **resident** mode: the query against data that is already in
      memory, with both incumbents measured the same way at the same thread count.
- [x] The cold sweep is kept, and the two are published side by side, because the operators are
      **58–69%** of the resident query and **3.8%** of the cold one.
- [x] The report records the engine's **operator** stages as well as the total and the scan, so a
      reader can see which part of the number moved.
- [x] The headline table was re-measured under the new code, because the plan output embedded in
      the committed reports changed.

## T9 — The follow-ons, not done

- [ ] **The eager path's decode is still serial.** It is 1,277.8 ms — the largest number in the
      repository, and 144 times the cost of the operators this increment parallelised. The
      engine's load is currently *behind* DuckDB's (1277.8 ms against 1218.0 ms), and six workers
      would put it roughly five times ahead. This is the next increment.
- [ ] **The operators do not overlap with the read.** The read's workers and the operators' workers
      are the same budget, used in sequence. A pipeline that overlaps them needs a morsel driver.
- [ ] **There is no task pool.** The blocks are equal-sized and the work is even, so work-stealing
      would buy nothing measurable. If a measurement shows the blocks are uneven, the parts are
      sorted by their block index before the merge — they already know their own position.
- [ ] **The filter's block size is a constant, 64 rows.** It is small enough that the masks
      concatenate cleanly and large enough that the per-block overhead does not show. A measurement
      would have to move it.
