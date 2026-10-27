# Spec 022 — The dictionary, counted — tasks

**Status**: done
**Spec**: [spec.md](spec.md)

## The instrument

- [x] `crates/columnar-engine/examples/dictionary_cost_probe.rs`, a new diagnostic that measures the
      crate's read and the engine's comparison on the same file, in the same process, with the
      engine's own batch size.
- [x] Four shapes: `crate_expanded`, `crate_kept`, `engine_expanded`, `engine_kept`, plus the
      engine's own read (`engine_read`) — the shape the published figure comes from.
- [x] The **alternative** is measured, not argued about: the same columns read with the dictionary
      kept, which is the only way the engine could stop comparing every row.
- [x] **The two paths are checked against each other before either is timed.** The probe reports the
      surviving rows of both, and says so when they disagree.
- [x] The crate's dictionary is checked for **reuse**: the probe reports whether the same dictionary
      array reaches more than one batch, because that decides whether the engine could compare the
      distinct values once a row group instead of once a batch.
- [x] One column at a time, in both modes, and one predicate at a time — so the attribution is per
      column and per predicate rather than a single number.
- [x] Three campaigns on each of the three files (six, twelve and twenty-four row groups), the
      reports committed as `bench-results/dictionary-cost-{6,12,24}-run{1,2,3}.json`.

## The separation

- [x] The crate's read and the engine's comparison separated, per row group.
- [x] The alternative measured: the crate's dictionary reader is **18.3 times slower** on this
      file, and **34.9 times** and **26.5 times** slower for the two numeric columns.
- [x] The crate's dictionary is rebuilt **every batch**, so the engine cannot compare the distinct
      values once a row group: the measurement says the crate will not hand it one to keep.

## The implementation

- [x] `crates/columnar-engine/src/dictionary.rs`, one module for the dictionary path:
      `distinct_matches`, `map_mask`, `mask_to_bools` and `compare_str`.
- [x] The **pushdown** uses it: the bounds are resolved once, before the loop over the values, so a
      comparison per distinct value does not match on an `enum` per distinct value.
- [x] The **filter** uses it: the map and the string comparison were written twice, once in each
      place, and they are now one implementation — so the two paths cannot drift apart.
- [x] The map stays **branchless**: a word at a time, because a date column's dictionary indices run
      in long stretches and a conditional bit set is a mispredicted branch every time a run ends.
- [x] A NULL never matches: a dictionary entry with no value is not a match, in either path.

## The tests

- [x] The shared module's own tests: the distinct values, the map, an index outside the dictionary,
      the bit mask, and the byte-wise string comparison.
- [x] The whole suite green: **341 tests**.

## The publication

- [x] `PERF.md`: the separation, the alternative's eighteen-fold cost, and the attribution.
- [x] `README.md`: the next question, and the status table.
- [x] `scripts/check-published-figures.py`: the new figures checked against the committed reports.
- [x] `bench-results/README.md`: the new report set documented.

## The gate

- [x] `cargo fmt`, clippy on arm64 and x86-64, the whole test suite, `cargo deny`, the wasm32
      check, the published-figures guard and the page checks all green.
