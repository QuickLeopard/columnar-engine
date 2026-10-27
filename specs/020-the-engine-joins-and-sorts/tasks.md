# Spec 020 — The full engine increment: hash join and sort — tasks

**Published**: <https://oib98vz3b8zwj.space.minimax.io>

## R1 — The dataset can be joined

- [x] `columnar-testdata` generates `orders`: TPC-H's `orders` at SF-1, 1,500,000 rows, nine
      columns, `o_orderkey` distinct over the same key space as `l_orderkey`.
- [x] The generator stays pure: the `orders` table uses its own PRNG stream, seeded
      deterministically from the same seed, so not one `lineitem` row changes — and there is a test
      that says so, by generating both and comparing the `lineitem` batches byte for byte.
- [x] The manifest records the second table: its row count, its size, its columns, the key space,
      and the join's expected match rate. **Measured** on the real SF-1 dataset: 1,500,000 distinct
      keys, 1,501,215 of 6,000,000 `lineitem` rows matched — 25.02%, against the 25% the design
      predicts.
- [x] `just dataset` writes both files.

## R2 — The engine joins

- [x] An inner equi-join, executed as a hash join: the build side is the right table, its
      predicates are evaluated before the table is built, and the probe side stays on the lazy
      path with its predicates pushed down.
- [x] A NULL key never matches, on either side. A NaN key is not a key at all, and `-0.0` is
      folded onto `0.0`.
- [x] A differential test against a brute-force nested-loop reference, row for row — with a build
      filter, a NULL key on each side, an empty build side, an empty probe side, a duplicate key in
      the build side, and a probe row with no partner.

## R3 — The engine sorts

- [x] `ORDER BY <column> [DESC] [LIMIT n]` as a stage, with a bounded heap when there is a limit.
- [x] A plan with a sort and an aggregate aggregates over the **sorted** rows.
- [x] A differential test against a reference sort, mutation-checked: disabling the sort fails two
      tests, and breaking the heap's "keep the best" rule fails one.

## R4 — The plan says what it does

- [x] The optimisation order gains the join and the sort, and is printed in the plan output.
- [x] The plan output names the join's keys, the build side's file, and the sort's key and limit.

## R5 — The comparison publishes the two new queries

- [x] `join-orders` and `sort-topk` are measured against `DuckDB`, `Polars` and the `DataFusion`
      peer, with the same SQL and the same two files.
- [x] The answers are cross-checked with the harness's relative tolerance. Polars and the peer
      compute the same answer as the engine: 11,472,226,962.89 against 11,472,226,962.89.
- [x] The incumbents register the second table: `DuckDB` reads both files by path, `Polars` and
      the peer register both tables by name. **The peer's binary is rebuilt before a campaign** —
      a stale one does not know the new flags and fails every query.
- [x] The two rows are published, as the median of three full campaigns, with the spread, derived
      by `scripts/medians.py`. The join's **resident** figure is **not** published: the engine's
      resident path reads the build side inside the clock and the incumbents do not, so the two
      numbers would not be the same measurement. It is named in the prose instead.
- [x] Where a published figure moves, both numbers are published and the reason is named.

## R6 — The evidence is checkable

- [x] The differential tests are in the tree and run in CI.
- [x] `scripts/check-published-figures.py` checks the new figures against the committed reports.

## The gate

- [x] fmt, clippy (arm64 and x86-64), the wasm32 cross-compile, the full test suite (310 tests),
      `cargo deny`.
- [x] `scripts/check-published-figures.py` and `scripts/check-page.py` are green.
- [ ] CI is green on the pushed commits.

## Two defects the increment found

- [x] **The join's probe hashed with `SipHash`** — built to resist collision attacks, which a
      query engine's hash table has no use for, and expensive: the probe is the hot loop. The join
      stage went from **219.9 ms to 117.7 ms** and the whole lazy query from **318.9 ms to
      199.5 ms**, with the same answer.
- [x] **`PYTHON_BIN` was read but never set**, so on a machine with several Pythons the harness
      asked the wrong one and the comparison quietly lost Polars — a row missing, with no
      explanation. The driver now finds an interpreter that has Polars.
- [x] **The `DataFusion` peer was a separate cargo workspace, so nothing linted it and no CI job
      tested it** — and the published comparison quotes it. It has the repository's lint table now,
      and a CI job of its own.
