# Spec 024 — The time the plan output does not name — tasks

**Status**: in progress
**Spec**: [spec.md](spec.md)

## The instrument

- [x] `crates/columnar-engine/examples/query_cost_probe.rs`, a new diagnostic that measures the
      engine's query in the shapes it is made of: the filter's selection expanded into row
      numbers, the plan's resolution, the memory estimate, the build side's file opened, the
      plan output's own stage table, and the whole query.
- [x] The probe asks the engine for the plan it would run and runs the query through the
      engine's own entry point, so its shapes are the engine's shapes.
- [x] The query's answer is checked against the engine's own **before** anything is timed: a probe
      that measures the wrong thing is not a measurement.
- [x] Three campaigns on the same host, the reports committed as
      `bench-results/query-cost-run{1,2,3}.json` -- and the same probe against the code this
      spec replaced, committed as `bench-results/query-cost-before-run{1,2,3}.json`, so the
      same file measures both halves.

## The separation

- [x] The selection's expansion is measured on its own: the row numbers of six million rows, built
      where the engine builds them.
- [x] The whole query and the stage table are measured side by side, on the same query, in the
      same session, and the difference is **measured, not inferred**.
- [x] The difference is attributed: the expansion, the plan's resolution, the memory estimate, the
      build side's file, or the remainder — whichever it is, and the remainder is named if it is
      not one of the four.
- [x] The six queries that account for themselves are the control, and they are measured too --
      the probe measures **every** published query, on **both** paths, and the six are in its
      report beside the two that had the hole.

## The change

- [x] The plan output accounts for the whole query: the stages it reports add up to the query's
      time, on both paths, for every published query.
- [x] The selection is not expanded into an index vector the join and the sort do not need — or it
      is named, and the time it takes is published. Either way the time is inside a stage or it
      does not happen. **It does not happen**: the selection that holds every row is the range,
      and the 24 MB vector is gone rather than renamed.
- [x] The plan output says what it did: the rows it names and the time it spends.

## The answer does not change

- [x] The join and the sort are handed the same rows, in the same order, with the same values.
- [x] The aggregate's answer is the same number, and the published queries' answers are unchanged.
- [x] A test asserts that the plan output accounts for the query, so the attribution cannot rot
      silently again.

## The fallbacks, named

- [x] A plan whose selection is not "every row" — a query with a predicate — is measured and
      published separately: its selection is a bitmap with holes, and the expansion is a different
      shape.
- [x] The join's build side's file open is the join's own cost, not the sort's, and the two are
      reported apart.
- [x] The cold path's streamed join holds the rows it kept, not the file's rows, so its selection
      is small; the resident path's is not, and the two are reported apart. The probe reports
      both paths for every query, and the difference is in the table.

## The publication

- [x] `PERF.md` §3m: the unnamed time, what it is, the change, and the before-and-after.
- [x] `README.md`: the status table, and the next question.
- [x] `bench-results/README.md`: the new reports, and the recipe that writes them.
- [x] `scripts/medians.py` derives the new tables; `scripts/check-published-figures.py` checks
      every published figure against the committed reports.
- [x] The comparison campaigns re-measured, for the code and for the code it replaced, back to back
      on the same host.

## The gate

- [x] `cargo fmt`, clippy on arm64 and x86-64, the whole test suite, `cargo deny`, the wasm32
      check, the published-figures guard and the page checks all green.
