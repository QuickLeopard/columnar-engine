# Spec 023 — The scan, taken apart, and the scan that feeds the join

**Status**: done
**Started**: 2026-10-27
**Finished**: 2026-10-27
**ADR**: [ADR-0030](../../docs/adr/0030-the-scan-that-feeds-the-join.md)
**Predecessor**: [022-the-dictionary-counted](../022-the-dictionary-counted/spec.md)

## The problem

The engine's cold `join-orders` is **100.4 ms** against DuckDB's **93.0 ms**. The plan output
says where the time goes: the **scan** is **71.4 ms** of the query and the join stage **21.9 ms**.

Spec 020 took the join apart — the build table and the probe. Spec 021 took the join's probe and
the sort apart, row by row, and closed both. **The scan was never taken apart.** The plan output
says the scan read 6,000,000 rows and materialised two columns: `l_orderkey`, the join's key, and
`l_extendedprice`, the column the revenue is computed from. That is 96 MB of values, copied out of
the reader and into the engine's own column chunks — and **96.4% of those rows do not match the
build side**, so the join throws them away.

Two questions, and they are different:

1. **What is the 71.4 ms made of?** How much is the Parquet decode, which is the floor, and how
   much is the engine's own copy?
2. **Does the engine copy what it should?** The engine copies every row the reader hands it. The
   join decides, a moment later, that 96.4% of those rows are worth nothing.

## The requirements

### R1 — The scan is measured in the shapes it is made of

The instrument measures, on the same file and in the same session:

| shape | what it is |
|---|---|
| `crate_orderkey` | the crate's read of `l_orderkey` alone |
| `crate_price` | the crate's read of `l_extendedprice` alone |
| `crate_both` | the crate's read of both |
| `engine_orderkey` | the engine's own scan, materialising `l_orderkey` |
| `engine_price` | the engine's own scan, materialising `l_extendedprice` |
| `engine_both` | the engine's own scan, materialising both |
| `crate_orders_both` | the crate's read of the build side's two columns |

**The engine's copy is the difference between the engine's shape and the crate's shape over the
same columns.** It is measured, not inferred, and it is published with the raw reports.

### R2 — The scan feeds the join

When a plan has a join, the scan **hands every batch to the join before it copies anything**. The
join's probe reads the batch's key column where it lies — in the reader's own array — and only the
rows the join keeps are copied into the dataset. **The rows the join discards are never copied.**

The probe is **one implementation**, reading its keys through one interface, so that the streamed
scan and the materialising scan cannot answer differently:

- the materialising scan probes a key column inside the dataset — the engine's own encoding;
- the streamed scan probes a key column inside the reader's batch — Arrow's own array.

Both go through the same lookup, against the same build table, with the same NULL rule (a NULL key
never matches) and the same NaN rule (a NaN key never matches).

### R3 — The dataset holds what the join kept, and the answer does not change

The streamed scan's dataset holds the probe side's rows that matched — one row per matching probe
row — and the join's output carries the multiplicity, as it does today: a probe row that matches
two build rows is two rows of the result. The row numbers the join and the sort carry are the
dataset's own row numbers, unchanged.

**The answer must not change.** The streamed scan and the materialising scan must answer the same
query with the same rows and the same values, and a test asserts it, row for row, for a plan with a
join and for a plan without one.

### R4 — The plan output says which path ran

The plan output reports the scan's rows out as the rows the scan **kept**, and the join's stage as
the join's own work. When the scan feeds the join, the probe happens **inside** the scan, so the
probe's cost is the scan's, and the join's stage reports the build table's construction. A reader of
the plan output must be able to tell, from the numbers, which path ran.

### R5 — The figures are published

The scan's decomposition is published with the raw reports, the before and the after side by side,
and the reason for every moved figure. The gate derives every published figure from the committed
reports.

## The instrument

`crates/columnar-engine/examples/scan_cost_probe.rs` — the scan, in the shapes above.

The probe reads the file with the **projection the plan asks for**, at the **batch size the engine
would use for the file** — it asks the engine for that, so its shapes are the engine's shapes by
construction rather than by a copy of the rule. The crate's shapes use the Parquet crate's own
reader over the same projection; the engine's shapes use the engine's own scan.

The probe reports, per shape, the median of five iterations and the spread, the host's load, and
the commit it ran at.

## The measurement

The instrument's medians, over three campaigns on the same host, on `data/lineitem.parquet`
(six million rows, six row groups), at a host load of 7.0–8.9:

| shape | the figure | the engine's copy |
|---|---|---|
| `crate_orderkey` — the crate's read of `l_orderkey` | 33.25 ms | |
| `crate_price` — the crate's read of `l_extendedprice` | 33.04 ms | |
| **`crate_both` — the crate's read of both** | **66.54 ms** | |
| `engine_orderkey` — the engine's own scan of `l_orderkey` | 36.19 ms | 2.94 ms |
| `engine_price` — the engine's own scan of `l_extendedprice` | 36.03 ms | 2.99 ms |
| **`engine_both` — the engine's own scan of both** | **73.03 ms** | **6.49 ms** |
| `crate_orders_both` — the crate's read of the build side | 9.99 ms | |

**The crate's decode is 66.54 ms and the engine's own work over it is 6.49 ms.** That 6.49 ms is
the copy: 96 MB of values, out of the reader's array and into the engine's own column chunks. It is
**8.9% of the scan**, and it is the whole of the engine's share of the 71.4 ms the plan output
reports.

**And 96.4% of what it copies is thrown away.** The join's output is 216,783 rows out of 6,001,215
probe rows: the build side holds 216,345 keys, and 96.4% of the probe side's rows match none of
them.

So the copy is 6.49 ms of work for 96.4% waste, and the engine's copy of the rows that survive is
3.5 MB rather than 96 MB.

## The gate

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`, and the same for `--target
  x86_64-apple-darwin`
- `cargo test --workspace --all-targets` — including the test that holds the streamed scan and the
  materialising scan to the same answer, row for row
- `cargo deny check`
- `just check-wasm`
- `python3 scripts/check-published-figures.py` — every published figure derived from the committed
  reports
- `python3 scripts/medians.py` — the tables derived from the raw reports, not typed

## Scope

**In scope:**

- The **probe side** of a join: the scan that feeds the join.
- The join's **probe** over the reader's batches, and the build table being built before the scan
  when the scan feeds the join.
- The plan output's stage table, which reports the scan's rows out as the rows the scan kept.

**Out of scope, with the reasons:**

- **The build side is not streamed.** It is 1.5 million rows against the probe side's 6 million,
  it is read once, and the hash table of its keys is what makes the probe O(1). Streaming it would
  not touch the 6.49 ms.
- **A plan whose probe side has a predicate of its own takes the materialising scan.** The
  predicate would have to be evaluated over every row of the file rather than over the rows that
  survived the scan, and that trade is not measured. The fallback is named here, not hidden: the
  streaming path is taken only when every predicate was pushed into the reader, which is exactly
  when the filter stage has no work to do.
- **A join key whose Arrow type the probe cannot read takes the materialising scan.** The probe
  reads integers, doubles and strings; a key of another type is declined, and the decline is
  recorded.
- **The aggregate's gather over the joined rows is unchanged.** The project stage still gathers the
  aggregate's column over the join's output. That gather is over 216,783 rows, not 6,000,000, and
  it is not what this spec is about.
- **The probe side's rows are still materialised.** The dataset holds one row per matching probe
  row; it is not a stream of them. What is saved is the copy of the rows the join discards.

## References

| principle | how this spec serves it |
|---|---|
| I. One Predicate, Six Implementations, One Answer | The probe is one implementation over one key interface, and the streamed scan and the materialising scan are held to the same answer by a test. |
| II. The Answer Is the Same | The streamed scan answers the same query with the same rows and the same values; the differential tests and the published queries are unchanged. |
| III. NULL Is an Absent Bit, and It Never Matches | The probe's key source carries the reader's validity, so a NULL key never matches, in either path. |
| IV. Evidence Over Claims | The 6.49 ms is measured by a committed, re-runnable probe, and every published figure is derived from the committed reports. |
| VI. The Dataset Is Named Honestly | The scan's rows out is the rows the scan kept, and the plan output says which path ran. |
| VII. Explicit Scope, No Silent Deferral | The four exclusions above, each with its reason; the fallbacks are named, not hidden. |
