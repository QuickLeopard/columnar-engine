# Spec 046 — The join-orders query, on both sides, taken apart — tasks

**Status**: implemented
**Started**: 2026-11-06
**Spec**: [spec.md](spec.md)

The order is the house's: **measure, then publish.** Nothing in the engine changes until the measurement
has said what to change.

## The measurement comes first

- [x] The **hypothesis** is written down before the instrument is built: `DuckDB` does not join faster
      than this engine, it joins **fewer rows**, because it pushes the build side's keys into the probe
      side's scan as a filter and this engine has no such filter.
- [x] The instrument is built **against the hypothesis**, and the hypothesis is **not** built into it:
      the counterfactual is a **setting**, not a conclusion.
- [x] Both sides' **answers** are checked against the published value — the same value the published
      campaigns recorded — **before** anything is timed.
- [x] The two sides are measured **back to back in one iteration**, and the **order is alternated**
      between the iterations. The iteration count is **even**, and the reason is the one spec 026 gave.
- [x] The **four arms** are measured in every iteration: the engine, `DuckDB` profiled, `DuckDB`
      unprofiled, and `DuckDB` with the join filter pushdown **disabled**.
- [x] Every part is read **by name**, and each part says what it is. The parts that have no counterpart
      on the other side say so on their own row.
- [x] The **row counts** are published on both sides: how many rows reach each side's join, and how many
      the join produced.
- [x] The **bytes** `DuckDB` read are published from its own profile, and the engine's bytes are
      **not** claimed.
- [x] The **unprofiled** arm is measured, so the profiler's price is published rather than assumed, and
      the whole the parts sum to is not mistaken for the whole the comparison harness measures.
- [x] Every part's **own spread** is published beside its value, and the **host's load average** travels
      with the report.
- [x] The **three campaigns** are taken, the raw reports are **committed**, and the published table is
      derived from them by `scripts/medians.py`.

## The change

- [x] **No change to the engine's query path.** The increment measures; what it finds decides what is
      built, and the decision belongs to the increment that follows.
- [x] The instrument lives in `columnar-bench`, which is where the engine and the incumbents are
      measured by one harness, and it adds **no dependency**: `DuckDB` is driven as the subprocess the
      comparison harness already drives.

## The answer does not change

- [x] The engine's own answer is the published one, and `DuckDB`'s is the published one.
- [x] The join's rows, their order and their count are untouched — nothing in the engine changed.

## The publication

- [x] The parts are published in `PERF.md` under a **new** section, with **every figure that moved**
      published **old-and-new** and the reason.
- [x] The engine's and `DuckDB`'s **published totals** are **not** re-derived, and the reason is
      recorded: they are the comparison harness's figures, taken by a different campaign.
- [x] The **hypothesis** is published as it came out — confirmed or **refused** — and a refused
      hypothesis is published with the numbers that refuse it.
- [x] The **next question** is written where this one was: `STATUS.md`'s last section, and the
      `README`'s Next row if it carries one.
- [x] `scripts/medians.py` derives the new table, and `scripts/check-published-figures.py` checks the
      published figures against the committed reports. A set with **fewer than three campaigns fails**,
      and a family is never skipped silently.
- [x] `bench-results/README.md` gains a row per new set.
- [x] This spec's ADR records the decision, and the raw reports are committed.

## The gate

- [x] `cargo fmt`, clippy on arm64 and x86-64, the whole test suite, `cargo deny`, the wasm32 check, the
      published-figures guard, the links check, the package check and the dashboard check all green.
- [ ] CI green, and the spec's status is `done` with the date it finished.
