# Spec 005 — Tasks

**Spec**: [`spec.md`](spec.md)
**Status**: not started. Every requirement below is open.

Legend: `[x]` done · `[ ]` open · `[~]` partially done, and the gap is written down.

---

## T1 — The parallel read

- [ ] `ParquetSource::read` takes a thread count, and one thread means what it means today.
- [ ] The file's row groups are split across the threads, and each thread builds its own reader
      over its own row groups with `with_row_groups`.
- [ ] The pushdown is applied per thread, on that thread's batches. A batch never spans a row
      group, so the predicates are independent.
- [ ] The engine's own limit is the **merge**, and the merge is the work: see T2.

## T2 — The merge

- [ ] The partial datasets are merged **in row-group order**, so the result holds the file's rows
      in the file's order.
- [ ] A plain column concatenates: the values, and the validity words.
- [ ] A dictionary column whose partial dictionaries differ is merged by **remapping** one into
      the other — the same operation the engine already performs per batch.
- [ ] The merge preserves the row count exactly, and the null rows exactly.

## T3 — The equivalence

- [ ] A test holds the parallel path to the single-threaded path **bit for bit**, over every
      built-in query, on the real generated file.
- [ ] A test holds the parallel path to the **eager** path, row for row, the same way the
      existing differential test holds the lazy path to the eager one.
- [ ] `SUM` over the parallel path returns the same bits as over the single-threaded path. The
      order is fixed, so it must.

## T4 — The reporting

- [ ] The plan output reports the thread count.
- [ ] The benchmark report records the thread count in the `machine` block, next to the SIMD
      path and the toolchain — a number without its thread count is a claim, not evidence.
- [ ] The memory estimate accounts for the parallel path's peak working set, and the budget is
      still checked before any work is done.

## T5 — The benchmark

- [ ] The pinned, one-thread comparison stays the headline. It is the like-for-like claim.
- [ ] A **scaling sweep** publishes the engine and both incumbents at 1, 2, 4, 8 and 14 threads,
      on the same file, in the same harness. The incumbents already take a thread count; the
      harness has to sweep it.
- [ ] Every published figure carries its thread count.

## T6 — The follow-ons, not done

- [ ] **Parallel decode for the eager path.** The same machinery applies, and it is worth doing:
      the eager path reads 231 MiB, and it is the slowest thing in the repository.
- [ ] **The operators stay single-threaded.** The filter, project and aggregate are not
      parallelised. The scan is where the bytes are, and the scan is 90% of the cold path — but
      this is a scope decision, not a measurement, and it should be measured rather than assumed.
- [ ] **Six row groups cap the parallelism at six.** That is enough — DuckDB's observed speedup
      from fourteen cores is 3.3× — but a file with more row groups would scale further, and
      changing the dataset to get there is not a change this repository makes lightly.
