# Spec 003 — Tasks

**Spec**: [`spec.md`](spec.md)
**Status**: implemented. Every requirement below is done, and the gate is green.

Legend: `[x]` done · `[ ]` open · `[~]` partially done, and the gap is written down.

---

## T1 — Attribute the scan

- [x] The pushdown reports what it cost, and over how many rows, through a shared, lock-free
      counter: `PushdownMetrics` and `PushdownCounter` in `columnar-engine/src/pushdown.rs`.
- [x] The plan output prints it: `pushdown cost: 42.9 ms over 8178861 rows (5.25 ps/row)`.
- [x] The measurement is taken **before** the optimisation, which is the only way it can say
      anything. See spec 003 §1 for the attribution.

## T2 — Run the engine's kernel in the pushdown

- [x] `evaluate_int64` and `evaluate_float64` call `columnar_kernels::compare_i64_into` and
      `compare_f64_into`, over `array.values()` — Arrow's raw value slice, with no copy.
- [x] The conjunction over one column is evaluated one comparison at a time and the masks are
      ANDed, a word at a time. The kernel does one comparison per call.
- [x] The pushdown's own `compare_i64` and `compare_f64` are **deleted**. The module no longer
      contains a float comparison of its own.
- [x] The flavor is the one the filter would use: `options.flavor.unwrap_or_else(best)`, threaded
      through `ParquetSource::read` into `PushdownPlan::to_row_filter`.
- [x] The threshold's type is checked once per predicate rather than once per row, in the string
      paths as well as the numeric ones.

## T3 — The validity bitmap

- [x] Arrow's null buffer and the engine's `NullBitmap` are both LSB-first and bit-packed, with a
      set bit meaning "this row holds a value", so the common case is a straight copy.
- [x] Arrow's buffer is a whole number of *bytes*, not of words — a small array's null buffer is
      one byte long — so the words are assembled from the bytes in little-endian order, which is
      how Arrow packs them.
- [x] A buffer with a **bit** offset is shifted down into place, one bit at a time. The Parquet
      reader never produces one; the path exists so the answer is right if something else does.
- [x] A NULL row never matches, for every operator, including `!=`: the kernel's contract
      provides it, and the pushdown passes the validity to it.

## T4 — The dictionary map

- [x] Branchless, and accumulated a word at a time, exactly as the engine's own dictionary path
      is. The engine's path was rewritten for that reason and cost 41 ms before it was fixed; the
      pushdown pays the same price for the same mistake if it is written the obvious way.
- [x] A NULL row is NULL regardless of what its dictionary entry says: the validity mask is
      applied to the answer, not to the lookup.

## T5 — The equivalence

- [x] The differential test is **not** weakened. `crates/columnar-engine/tests/lazy_scan.rs`
      still holds the eager and lazy paths to the same answer, row for row, over every operator,
      every NULL and NaN edge, a dictionary column and a plain UTF-8 column, at row counts 0, 1,
      2, 63, 64, 65, 127, 128, 129, 191, 192 and 193.
- [x] `crates/columnar-cli/tests/lazy_scan.rs` does the same over the real generated file with
      the real Q6 plan.
- [x] The test caught a real defect while this was being built: Arrow's `Buffer::typed_data` and
      the `BooleanBuffer`'s byte length do not agree about word alignment, and the first attempt
      at the validity conversion panicked on a small array's one-byte null buffer. The
      conversion is now byte-exact.

## T6 — The measurement

- [x] The cold `q6` query and every other lazy query are faster, and the improvement is
      attributed in `PERF.md`.
- [x] The pushdown's share of the scan is published, before and after.

## T7 — Follow-ons, not done

- [ ] **The string path is still scalar.** The dictionary column is compared once per distinct
      value and mapped, but a *plain* UTF-8 column is compared row by row. The kernel is numeric;
      a string kernel is a different project.
- [ ] **The dictionary gather is not vectorised.** `mask[row] = distinct[key[row]]` is a gather.
      AVX2 has no gather instruction, and AVX-512's `vpgatherdd` is not in the kernel's target
      set. The map is made branchless instead, which is what the engine's own path does.
- [ ] **The pushdown still costs what the decode costs.** After this spec the two are close to
      even. The reader's decode is now the larger half, and it is parallelisable — see spec 005.
