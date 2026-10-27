# Spec 021 — The hot paths, taken apart — tasks

## R1 — The join's probe resolves its key once, and looks it up in a numeric table

- [x] The probe's key type is resolved **once**, before the loop: the plan already requires the two
      sides to have the same type, so the type is a property of the plan.
- [x] An **integer** join key is looked up in a table of the key's own bits, not in a `HashMap`
      over a 32-byte enum. `JoinKey` no longer derives `Hash`: the hash is written by hand, over
      the payload alone, so the discriminant is not hashed a second time for every row.
- [x] When the build side's keys lie in a **dense integer range that fits the budget**, the table
      is addressed by the key itself: no hash, no probe chain, one load. The table's width comes
      from the **file's own row-group statistics**, so the estimate and the builder are made from
      the same number. When the range is wide, or the key is not an integer, or a key falls
      outside the range the statistics claim, the table is an open-addressed hash table.
- [x] The build table reserves its capacity, so it does not rehash its way to its final size.
- [x] The probe's cost is **measured**, not estimated: `crates/columnar-engine/examples/
      operator_probe.rs` decomposes it and the numbers are published.

  Measured over six million probe rows against a table of 216,345 keys, the two code versions
  measured back to back on the same host:

  | | before | after | |
  |---|---|---|---|
  | reading the key column | 9.3 ms | 9.4 ms | |
  | reading the key column's own integers | 1.7 ms | 1.7 ms | the floor |
  | **the engine's probe** | **102.7 ms** | **9.0 ms** | **11.4×** |
  | an open-addressed table over the key's bits | 66.2 ms | 70.0 ms | |
  | a table addressed by the key itself, 4 bytes a count | 44.0 ms | 42.9 ms | |
  | **a table addressed by the key itself, 1 byte a count** | **16.4 ms** | **15.5 ms** | |

## R2 — A key's NULL check is hoisted out of the hot loop

- [x] A column that holds **no NULL at all** does not have its validity bitmap read, once per
      row, in the join's probe or in the sort. `KeyColumn` carries the answer, and the filter
      operator's question is now asked of the key reader too.
- [x] The check is still made where it is needed: a column **with** NULLs keeps the check, and a
      NULL key still never matches and still sorts last.
- [x] The join's probe has a fast path that reads the key column's own integers — no enum, no
      validity bitmap — and a general path for a column that holds NULLs or is dictionary-encoded.

  Measured: reading the key column went from **9.3 ms** to **1.7 ms**.

## R3 — The sort reads each key once, and compares without an enum

- [x] The sort **without** a limit reads each key **once**. It read it `2n·log n` times: the
      comparator called the key accessor twice per comparison. The rows are decorated with their
      keys before the sort begins.
- [x] The direction of the sort is resolved before the loop.
- [x] A key's comparison does not match on an enum per row, and does not carry an `Option` per
      row — **on the fast path**, a plain number column with no NULLs, where the key is the
      column's own value and the comparison is a number's comparison. The general path (a column
      with NULLs, or a dictionary-encoded one) still carries the enum, and the measurement says
      that is the right trade: the sort's own work is now **a tenth of a millisecond** of its
      **3.7 ms**, and the
      key read is the rest.

  Measured: the engine's bounded sort went from **13.3 ms** to **3.7 ms**, against a plain-read
  floor of **3.6 ms**.

## R4 — The row positions the join and the sort carry are the dataset's row numbers

- [x] The identity `positions` vector — `0..n`, 24 MB on a six-million-row query — is gone.
- [x] The probe and the sort emit **dataset row numbers**, so the aggregate gathers from them
      directly and the sort's key lookup loses one dependent load per row.
- [x] The translation between the two numberings is gone with them.

## R5 — The estimates are honest, and they are tested

- [x] The join's build side is charged by the rows that can survive the build filter — bounded
      from the **filter column's own row-group statistics**, so a row group whose range cannot
      satisfy the filter is not counted — and by the table the join will build, decided from the
      same numbers the join decides it from.
- [x] `ParquetSource::column_range` reports a column's range over the file's row groups.
- [x] A **global** aggregate is charged by its **groups**, not by the rows, on the eager path as
      well as the lazy one. The eager path was charging 91.6 MiB of phantom on every plan.
- [x] The row-position vectors are counted, because they are memory the engine holds.
- [x] Every estimate has a **test**, and every test asserts a **property** rather than a number:
      `crates/columnar-engine/tests/memory_estimates.rs`.

## R6 — The plan output says what happened

- [x] The project stage in the ordered path reports the rows it **gathered**, not the rows it
      was handed. The plan output said `project 6000000 6000000` for a projection that gathered
      216,783 rows; it now says `216783 216783`.

## The instrument

- [x] `crates/columnar-engine/examples/operator_probe.rs` takes the join's probe and the sort
      apart, and checks every candidate against the engine's own output, row for row, before it
      is timed. A candidate that is wrong is not a candidate.
- [x] The check is **real**, and it earned its place: it caught the sort's new fast path
      reversing its bounded heap's ordering — the query answered **500025.13** instead of
      **1049999.60** — and it caught the probe's own direct-addressed table answering for a key
      below its base, which put **2 extra rows** in a 216,783-row answer. Both were fixed before
      anything was published.
- [x] A **Bloom filter** in front of the table was built and measured, and it does not help. The
      reason is published: with 216,345 keys the filter needs about six million bits before it
      rejects anything, and a filter that large is no cheaper to consult than the table it
      guards. **A negative result, published.**

## The measurement

- [x] Three campaigns on a host quiet enough to measure on (load 5.53–7.17), the reports
      committed, and the published tables re-derived by `scripts/medians.py`.
- [x] **The old code was re-measured on the same host**, back to back, in its own three
      campaigns, so the code's share of every moved figure is separated from the machine's:
      `bench-results/compare-before-run{1,2,3}.json`.
- [x] The set before that is committed too — `bench-results/compare-spec020-run{1,2,3}.json` — so
      every published figure, old and new, is derived from a committed report by a script, and
      the guard checks all four sets.
- [x] **The plan output is the attribution, read a stage at a time.** The stage breakdown is in
      the committed reports too: on this host the join stage went from **107.4 ms** to
      **21.9 ms** and the sort stage from **14.5 ms** to **5.3 ms**, while the two scan stages —
      Arrow's decode, which this change did not touch — moved *up* with the busier host.
- [x] The moved figures published old-and-new, with the reason. The control: in the cold lazy
      path the six queries this increment did not touch moved by **0% to +14.0%** between the two
      host-matched sets; the join moved by **−45.0%** cold and **−73.4%** resident, and the sort
      by **−5.9%** cold and **−44.2%** resident.

## The gate

- [x] `cargo fmt`, clippy on arm64 and x86-64, the whole test suite (336 tests), `cargo deny`,
      the wasm32 check, the published-figures guard and the page checks all green.
- [x] CI is green on the pushed commits.
