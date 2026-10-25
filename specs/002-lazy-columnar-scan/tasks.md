# Spec 002 — Tasks

**Spec**: [`spec.md`](spec.md)
**Status**: implemented. Every requirement below is done, and the gate is green.

Legend: `[x]` done · `[ ]` open · `[~]` partially done, and the gap is written down.

---

## T1 — The schema abstraction

- [x] `ColumnSchema` trait in `columnar-engine/src/plan.rs`: column count, name, type.
- [x] `Dataset` implements it, over its chunks.
- [x] `ParquetSource` implements it, over the file's Arrow schema.
- [x] `QueryPlan::resolve_against(&dyn ColumnSchema)`, with `resolve` kept as the eager wrapper.
- [x] The resolution is type-checked: an `int64` column against a `float64` threshold is a
      `TypeMismatch` on both paths.

## T2 — The lazy source

- [x] `ParquetSource::open(path)` reads the footer and nothing else.
- [x] `ParquetSource::read(columns, pushdown)`:
  - [x] a `ProjectionMask` over the file's root columns, so the other columns' bytes are never
        fetched;
  - [x] a `RowFilter` carrying the pushed predicates;
  - [x] a reader batch size of 8,192 rows, measured (the reader's default of 1,024 costs 5,860
        batches for a six-million-row file).
- [x] `Dataset` carries its row count explicitly, so a scan that produces rows and no columns --
      a `count(*)` over a fully pushed-down filter -- is representable.
- [x] `decode_batches` is shared with the eager read.

## T3 — The pushdown

- [x] `columnar-engine/src/pushdown.rs`: an `ArrowPredicate` that reproduces the engine's filter
      semantics exactly.
  - [x] NULL never matches, for every operator, including `!=`.
  - [x] NaN values and NaN thresholds never match, for every operator, including `!=`.
  - [x] `-0.0` and `+0.0` compare equal, as they do in the engine and not as totalOrder does.
  - [x] A dictionary column is compared once per distinct value and mapped back through the
        indices.
  - [x] Strings compare byte-wise.
- [x] The mask is built a word at a time, not through Arrow's `BooleanBuilder`. The builder's
      per-element capacity check cost more than decoding Q6's two columns.
- [x] Two predicates on the same column are pushed as **one** predicate. The reader decodes a
      column once per projection mask that mentions it, and Q6 has two predicates on
      `l_discount` and two on `l_shipdate`.

## T4 — Deciding what to read and what to push down

- [x] The read set is the plan's materialised columns.
- [x] A predicate whose column is already being read is **declined**: pushing it would make the
      reader decode that column twice.
- [x] A predicate the pushdown cannot evaluate exactly is declined.
- [x] A declined predicate's column joins the read set, because the engine's filter stage needs it.
- [x] The filter stage runs with the declined predicates only. The conjunction is separable:
      `(A ∧ B)` applied by the reader and then `B` applied by the engine is `A ∧ B`.

## T5 — The plan output

- [x] `columns read:` — the columns whose bytes were fetched, including the ones the pushed
      predicates read.
- [x] `pushed down:` — the predicates that were pushed, described.
- [x] The scan stage reports the file's rows in and the surviving rows out.

## T6 — The differential test

- [x] `crates/columnar-engine/tests/lazy_scan.rs` — 15 tests:
  - [x] every operator, over NULLs, NaNs, a NaN threshold and a dictionary column;
  - [x] a conjunction of predicates;
  - [x] the NULL rule, the NaN rule and the NaN-threshold rule, explicitly;
  - [x] row counts 0, 1, 2, 63, 64, 65, 127, 128, 129, 191, 192, 193 -- the boundaries of the
        64-row block the kernel works in;
  - [x] the projection: only the columns the plan needs are read;
  - [x] the pushdown: what was pushed, and what was not;
  - [x] an unknown column and a type mismatch are errors, on both paths.
- [x] `crates/columnar-cli/tests/lazy_scan.rs` — the same, over the **real** generated file with
      the **real** Q6 plan, for every built-in query.

## T7 — The benchmark

- [x] `compare` measures the engine twice: eager and lazy, the same way it measures the
      incumbents.
- [x] The two paths must agree, or the comparison refuses to report.
- [x] The report shows the lazy path in the **cold** table, where it belongs: it reads the file
      during the query, so it has no resident form.
- [x] `query --lazy` runs one query the lazy way and prints the plan output.

## T8 — The measurement, and what it found

- [x] The kernel A/B is unchanged; this increment does not touch the kernel.
- [x] The cold `q6` number falls from the eager path's **1181 ms** to the lazy path's **121 ms** —
      a **9.7×** reduction — and the gap against DuckDB's cold 32 ms from **36.9× to 3.8×**. See
      `PERF.md`; the figures are the median of three full runs, and the raw reports are committed.
- [x] The finding: the lazy path is now bound by the **Parquet reader**, not by the engine's
      operators. Three defects were found and fixed on the way:
  - [x] the reader decoded each predicate column twice, once per predicate;
  - [x] the Arrow-to-chunk conversion copied the dictionary's distinct values per batch -- 1.8
        million string allocations for a six-million-row file;
  - [x] the pushdown's mask was built through Arrow's `BooleanBuilder`, whose per-element
        capacity check cost more than decoding the columns.
- [x] **The dictionary-copy fix was worth more to the eager path than to the lazy one.** The
      lazy path never materialises the dictionary column, so it never paid the copy; the eager
      path pays it 5,860 times over, about 14.6 million string allocations. Putting the copy
      back and re-measuring puts the eager path's cold `q6` at **1890 ms against 1181 ms** — a
      1.60× difference, with the lazy path unmoved as the control. The perturbed run is committed
      as `bench-results/experiment-dictionary-copy.json`.
- [x] The remaining gap to DuckDB is measured and attributed, not described.

## T9 — Follow-ons, not done

- [ ] **Parallel decode. This is the gap.** `DuckDB` gets **3.3×** from fourteen cores on the
      identical query and file — 32.0 ms to 10.0 ms — and the engine has one thread, because the
      comparison pins both incumbents to one thread to keep it like-for-like. The reader-speed
      term is closed (the engine's full-file decode is at parity with DuckDB's), so this is
      almost exactly what stands between the engine and DuckDB on the cold path.
- [ ] **The lazy path materialises columns nobody reads.** The pushed predicate columns are
      built into engine chunks even when the query is `count(*)`, which needs no column at all.
      The plan output says `materialised columns: (none)` — and the reader still built two.
- [ ] **Page-level pruning.** *Not the gap, and not to be published as one.* DuckDB prunes
      nothing on this file either: its plan says `Row Groups Scanned: 6 / 6`, and the file has no
      page index for either system to select against. It would still be a further win on a file
      that *has* one, and it needs either a `RowSelection` computed from a page index with the
      engine's own rules, or the row-format predicate -- which computes a different function, see
      [ADR-0010](../../docs/adr/0010-pushdown-equivalence.md).
- [ ] **The file has no page index.** The generator writes `EnabledStatistics::Chunk`, so there
      are no per-page statistics to prune with. Writing `EnabledStatistics::Page` would change
      the dataset, and the dataset is meant to model a real TPC-H file.
- [ ] **The pushdown is scalar.** The engine's SIMD comparison kernel works on the engine's own
      chunk format. Using it here would mean converting the column first, which costs more than
      the scalar comparison saves. A pushdown that reads the Arrow array's raw slice directly
      would be worth having.
- [ ] **Parallel decode, again, for the eager path.** Sixteen columns, no scheduler needed, and
      the reader is single-threaded like the rest of the engine in this increment.
