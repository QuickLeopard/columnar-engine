# Spec 049 — The batch size's price — tasks

**Status**: planned
**Started**: 2026-11-07
**Spec**: [spec.md](spec.md)

The order is the house's: **the measurement decides what is built.** The knob is already there — spec 048 built it —
and this increment prices it. The default moves only if the measurement says so, and the answer is published either
way.

## The instrument, first

- [ ] The **counting global allocator**: the current held bytes and the **peak**, in the increment's own example.
- [ ] The read at **every batch size** of the sweep — **62,500**, **125,000**, **250,000**, **1,000,000** — and at
      **three widths**: the join query's two columns, `q6`'s two predicate columns, the eager path's sixteen.
- [ ] The **engine's own estimate** printed beside the measurement, at the same point, so the two can be read
      against each other.
- [ ] Every point measured **three times**, the median published.
- [ ] The allocator lives in the **example**, never in the engine: `#![deny(unsafe_code)]` stands in every crate.

## The measurement

- [ ] The **peak held bytes** at each batch size and width, and the **estimate's** figure beside it.
- [ ] The **gap** between them — the estimate's error, published as the estimate's error.
- [ ] The **query's own parts** at each batch size, in spec 046's probe, so the join's win is priced against the
      other shape.
- [ ] The **`filter-range`** query at the same batch sizes, **back to back** with the join query, **order
      alternated**, so the difference is the **paired** one.
- [ ] The **three campaigns**, the raw reports **committed**, and the published table derived from them by
      `scripts/medians.py`.

## The engine

- [ ] The **estimate learns the batch**: the columns the reader fetches for the predicates, and the batch's own
      rows, are charged at the **batch's** size.
- [ ] The **budget** can then refuse a plan whose batch does not fit.
- [ ] The **public surface does not break**: a new function is an addition, a new parameter would not be.
- [ ] A **test** that the estimate charges the batch, and that the two charges agree with what the allocator
      measured.

## The decision

- [ ] The default's **rule** is decided by the measurement: a **byte budget**, or the row count — and **either way**
      it is published with the numbers that decided it.
- [ ] If the default moves, **every figure that moves is published old-and-new**, with the reason, and re-measured
      rather than recomputed.
- [ ] The estimate's own figure is published **old-and-new**, with the reason.

## The publication

- [ ] A **new section** in `PERF.md`, with the tables, and the guard checking the figures **inside** it.
- [ ] The **ADR** records the decision, and the correction of the two specs' wrong constitution citation.
- [ ] `STATUS.md`'s next question is rewritten, and `docs/status.html` is rebuilt.
- [ ] `bench-results/README.md` gains a row per new set.

## The gate

- [ ] `cargo fmt`, clippy, the whole test suite, `cargo deny`, the wasm32 check, the published-figures guard, the
      links check, the package check and the dashboard check all green.
- [ ] CI green, and the spec's status is `done` with the date it finished.
