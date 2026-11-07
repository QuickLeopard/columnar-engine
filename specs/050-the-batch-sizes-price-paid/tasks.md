# Spec 050 — The batch size's price, paid — tasks

**Status**: planned
**Started**: 2026-11-07
**Spec**: [spec.md](spec.md)

The order is the house's: **the statement is measured against before it is written down.** The knob is there and the
price is measured; this increment makes the engine's own numbers honest.

## The statement, first

- [ ] The **width statement**: **8** bytes for an `int64` or a `float64`, **4** for a dictionary-encoded string's
      index, **24** for a plain string's header, plus the string's own heap bytes.
- [ ] The **footer's own numbers** published beside it — the file's `total_uncompressed_size` over its `num_values` —
      and the **2.5×** difference on the wide shape, so the reader can see why the footer cannot be the statement.
- [ ] The statement **checked against the allocator's measurement** at all three widths, and the difference published.
- [ ] **How the engine knows a column is dictionary-encoded** is settled and published: the footer lists
      `RLE_DICTIONARY` among a chunk's encodings even for a **plain** column, so the signal has to be the chunk's own
      dictionary, and not the list.

## The engine

- [ ] **One** place says what a row costs, and **the estimate and the batch's rule read it**.
- [ ] The estimate's per-column charge is the statement's, so **the estimate's document and the estimate's code
      agree**.
- [ ] The estimate **charges the batch's** transient bytes beside the file's rows, and **the budget can then refuse a
      plan whose batch does not fit**.
- [ ] **The public surface does not break**: a new function is an addition, a new parameter would not be. The
      semver baseline moves, and the move is recorded.

## The published figures

- [ ] Every figure the estimate touches is published **old-and-new**, with the reason.
- [ ] **The sentence about the query the budget refuses** is corrected where it was published, if the corrected
      estimate no longer refuses it.
- [ ] The **guard** checks the new figures **inside the section that publishes them**, and the old ones are checked
      against the **before** reports.

## The publication

- [ ] A **new section** in `PERF.md`, the decision in an **ADR**, the status's next question rewritten, and the raw
      reports **committed**.
- [ ] `bench-results/README.md` gains a row per new set, and `docs/status.html` is rebuilt.

## The gate

- [ ] `cargo fmt`, clippy, the whole test suite, `cargo deny`, the wasm32 check, the published-figures guard, the
      links check, the package check and the dashboard check all green.
- [ ] CI green, and the spec's status is `done` with the date it finished.
