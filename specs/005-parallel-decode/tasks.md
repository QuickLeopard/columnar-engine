# Spec 005 — Tasks

**Spec**: [`spec.md`](spec.md)
**Status**: **done.** Every requirement below is met and checked.

Legend: `[x]` done · `[ ]` open · `[~]` partially done, and the gap is written down.

---

## T1 — The parallel read

- [x] `ParquetSource::read` takes a thread count, and one thread means what it means today.
      One thread takes the *old* path — a single reader over every row group — so the
      single-threaded behaviour is unchanged by construction, not by a branch that happens to
      work.
- [x] The file's row groups are split across the threads, and each thread builds its own reader
      over its own row groups with `with_row_groups`. The split is into **contiguous blocks**,
      one per thread, which is what makes the merge order the file's order without any sorting.
- [x] The pushdown is applied per thread, on that thread's batches. A batch never spans a row
      group, so the predicates are independent.
- [x] The engine's own limit is the **merge**, and the merge is the work: see T2.

## T2 — The merge

- [x] The partial datasets are merged **in row-group order**, so the result holds the file's rows
      in the file's order. The handles are joined **in block order**, not in completion order:
      that one choice is the whole mechanism, and it costs nothing.
- [x] A plain column concatenates: the values, and the validity words.
- [x] A dictionary column whose partial dictionaries differ is merged by **remapping** one into
      the other — the same operation the engine already performs per batch.
- [x] The merge preserves the row count exactly, and the null rows exactly.

      **And the merge's validity concatenation had a real bug, which the new unit test found.**
      `NullBitmap::concat` copied only the *complete* words of the first mask and dropped its
      last, partial word — which is exactly the word the second mask's bits land in. Every NULL
      in the first part of a merged chunk was silently lost. `crates/columnar-core/src/null.rs`
      now pins the concatenation against a single mask over the same rows, at every combination
      of word alignment.

## T3 — The equivalence

- [x] A test holds the parallel path to the single-threaded path **bit for bit**, over every
      built-in query, on the real generated file
      (`crates/columnar-cli/tests/lazy_scan.rs::every_builtin_query_agrees_at_every_thread_count`,
      at 2, 3, 4, 6, 8 and 14 threads).
- [x] A test holds the parallel path to the **eager** path, row for row, the same way the
      existing differential test holds the lazy path to the eager one.
- [x] `SUM` over the parallel path returns the same bits as over the single-threaded path. The
      order is fixed, so it must.
- [x] **The rows themselves are compared, not just the answer.** A sum does not care which row
      came first, so the aggregate-level tests do *not* catch a mis-ordered merge — verified, by
      reversing the merge order in the source and watching the aggregate tests still pass. The
      test with teeth is
      `crates/columnar-engine/tests/lazy_scan.rs::the_parallel_read_holds_the_same_rows_in_the_same_order`,
      which compares the two datasets row by row with the dictionary resolved.
- [x] The tests use a file with **three row groups**, written by `write_row_groups`. The earlier
      versions of these tests wrote a single batch, so the file had one row group, so the
      parallel read had one worker — and the tests were testing the single-threaded path under
      the parallel path's name.

## T4 — The reporting

- [x] The plan output reports the thread count — and reports the **workers that ran**, not the
      threads that were asked for. The read splits the file's row groups, so a file with six row
      groups has six workers however many threads are requested, and a plan that claimed
      fourteen when six ran would be telling a lie.
- [x] The benchmark report records the thread count in the `machine` block, next to the SIMD
      path and the toolchain — a number without its thread count is a claim, not evidence.
- [x] The memory estimate accounts for the parallel path's peak working set, and the budget is
      still checked before any work is done.
      The parallel read holds **two** copies of the read set at its peak — every thread's part
      at once, and then the merged result — where the single-threaded read holds one. The
      estimate counts two copies when there is more than one worker.
      And the estimate is now a **pre-flight** one: the lazy path's working set is estimated from
      the file's schema and row count, *before* the read, so a plan that does not fit is refused
      having read nothing at all. The old estimate was made over the dataset, which did not exist
      until after the read.

## T5 — The benchmark

- [x] The pinned, one-thread comparison stays the headline. It is the like-for-like claim.
- [x] A **scaling sweep** publishes the engine and both incumbents at 1, 2, 4, 6, 8 and 14
      threads, on the same file, in the same harness. The incumbents already take a thread count;
      the harness has to sweep it.
      `just sweep` → `crates/columnar-bench/src/scaling.rs` → `bench-results/scaling-run*.json`.
- [x] Every published figure carries its thread count.

      **The result.** The engine gets **5.1×** from six workers, against DuckDB's **3.2×**. The
      `q6` cold gap narrows from **2.6× at one thread to 1.6× at six**. The curve is flat past
      six because the file has six row groups, and the report says so.

      **The headline table was re-measured**, because the published lazy `q6` figure was
      **89.1 ms** and the same measurement now gives **84.4 ms** — the incumbents' figures did
      not move, so the difference was the harness's ordering (eager first, lazy second) and not
      the machine. Both numbers are published and the difference is explained; see `PERF.md`
      §2e.

## T6 — The follow-ons, not done

- [ ] **Parallel decode for the eager path.** The same machinery applies, and it is worth doing:
      the eager path reads 231 MiB, and it is the slowest thing in the repository.
- [ ] **The operators stay single-threaded.** The filter, project and aggregate are not
      parallelised. The scan is where the bytes are, and the scan is 90% of the cold path — but
      this is a scope decision, not a measurement, and it should be measured rather than assumed.
      The sweep's shape is the first evidence either way: the engine's curve is steeper than
      DuckDB's precisely because the parallel part is a larger fraction of its work.
- [ ] **Six row groups cap the parallelism at six.** That is enough — DuckDB's observed speedup
      from fourteen cores is 3.2×, and the engine's from six is 5.1× — but a file with more row
      groups would scale further, and changing the dataset to get there is not a change this
      repository makes lightly. The cap is recorded in every sweep report.
- [ ] **The eager path's memory budget is still a post-read estimate.** The pre-flight estimate
      covers the lazy path, where the dataset does not exist before the read. The eager path's
      dataset already does, so its estimate is made over it, and the budget is checked after the
      read — which is the eager path's own semantics, and not changed here.
