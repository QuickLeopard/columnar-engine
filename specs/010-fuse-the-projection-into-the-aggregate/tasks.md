# Spec 010 — Tasks

**Spec**: [010-fuse-the-projection-into-the-aggregate](spec.md)
**ADR**: [ADR-0016](../../docs/adr/0016-the-aggregate-reads-in-place.md)
**Status**: done

## T1 — The measurement, before anything was written

- [x] `filter-range`'s stages were read off the plan output: filter 1.4 ms, **project
      7.8 ms**, aggregate 2.2 ms. The projection was more than half the query.
- [x] The projection was materialising a column **nobody would ever see**: a plan with
      an aggregation ignores the projection, because the aggregate result is the
      output.
- [x] The alternatives were **prototyped and measured** before the design was fixed.
      On the real file, with the real selection, four ways of getting the same answer:
      **gather then fold 9.06 ms**, **fold at the selection's rows 1.83 ms**, fold by
      testing the selection per row 17.68 ms, fold a word at a time 1.99 ms, and the
      whole column 3.73 ms.
- [x] All four folds produced **bit-identical** sums.
- [x] The spread decided the shape: the fold must **follow the selection** and must
      **not copy**.

## T2 — The aggregate reads the source in place

- [x] A global aggregate takes the **source column** and the **selection**, and reads
      the survivors' values out of the source where they are.
- [x] The projection **materialises nothing** for such a column. The plan output still
      names it, because the query read it.
- [x] The source column's **encoding** is read directly: a dictionary-encoded column
      is read through its dictionary, exactly as the gather read it.
- [x] A run-length-encoded column cannot be read a row at a time, so it is **gathered**
      as it always was.

## T3 — The answers did not change

- [x] The survivors are visited in **ascending row order**, so a float sum is bit for
      bit the sum the gathered column produced.
- [x] A NULL row is skipped, and a row beyond the validity mask's length is a NULL
      row — exactly as the gather treated it.
- [x] An integer sum **saturates**; a float min/max compares with `total_cmp`.

## T4 — What is fused, and what is not

- [x] **Global** aggregations only. A grouped aggregation needs its keys aligned with
      its values, and the hash table wants both.
- [x] A column read by a **plain** aggregate and by nothing else. A column an
      *expression* reads is still gathered, because the expression is evaluated over
      the survivors.
- [x] **One** global implementation: the grouped path converts its gathered columns
      into the same value sources and calls the same fold.

## T5 — The fold is specialised, not dispatched

- [x] The first implementation dispatched on the column's kind and the accumulator's
      kind **inside** the loop, and cost **6.2 ms**.
- [x] Resolving both **before** the loop took the same fold to **2.0 ms**.

## T6 — The tests

- [x] The fused sum is compared against the **gathered** sum — the same sum computed
      through an expression, which gathers — **bit for bit**.
- [x] The float column's data is **order-sensitive on purpose**: it repeats
      `0.1, 0.2, 0.3`, and the test asserts that the ascending and the descending sums
      differ, so a fold in the wrong order cannot pass.
- [x] The NULL rows hold a value **no correct sum would hold**, so a fold that counted
      one is caught.
- [x] **New**: a fused aggregate materialises **nothing** — the decode counter says so.
- [x] **New**: an expression's columns are still gathered, and a column read both
      plainly and through an expression is gathered.
- [x] **New**: a grouped aggregate still gathers.
- [x] **New**: a dictionary column is folded through its dictionary.
- [x] **New**: a count over a column skips the NULL rows; a min and a max are folded in
      place too; an integer sum saturates; the filtered column can be the aggregated
      column.
- [x] **The mutations were run**: the wrong order, the NULL rows not skipped, a count
      counting every survivor, a dictionary read backwards, an expression's column
      fused, a grouped aggregate fused, an integer sum wrapping, and a min and a max
      swapped — each injected, each caught.

## T7 — The publication

- [x] The headline table was **re-measured**: three full campaigns, medians and
      spreads, the raw reports committed.
- [x] The **sweeps were not re-measured**: they measure `q6`, whose aggregate is
      `count(*)` over no column, and the fusion does not reach it.
- [x] ADR-0016, this spec, `PERF.md`, `README.md` and the constitution were updated.
- [x] The old figures are published next to the new ones.
