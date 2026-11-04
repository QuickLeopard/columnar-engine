# Spec 035 — The drift's own parts — tasks

**Status**: in progress
**Spec**: [spec.md](spec.md)

## Phase 1 — the ladder

- [x] The **movement machinery**: every measurement's movement between every pair of campaigns, the
      reconciliation, and the **paired** share — the part's own series less the floor's own series,
      iteration by iteration. It reads the committed reports and writes nothing.
- [x] The machinery is **derived**: the guard computes the figures from `bench-results/`, and
      nothing in the publication is typed by hand.

## Phase 2 — the leaves

- [x] The ladder is applied to the **context's leaves**, on both paths, with the floor's share beside
      each one.
- [x] The **engine's own variance** is the named remainder of an exact reconciliation, and the
      reconciliation closes to **0.000 ms**.

## Phase 3 — the file's state

- [x] The **reads' movement** is named as the file's state: the build side's read and the probe's
      batches, with the cold path's own figure — the excursion and the baseline it broke away from.
- [x] And the **yardstick's validity** is answered per path: the floor is sound on the resident path
      and it is not on the cold path, with the numbers, and with the **resolution that would be
      needed**.

## Phase 4 — the reproducibility

- [x] For both paths: the cost's **drift**, the drift as a **share of the cost**, and the machine's
      share of it.
- [x] The figure is published **whether it is large or small**.

## Phase 5 — the publication

- [x] `PERF.md` publishes the ladder, and the guard checks every figure **inside the section that
      publishes it**.
- [x] `scripts/medians.py` prints the decomposition beside the others.
- [x] **Both** sets are published: the set the file publishes now, and the set this increment measured
      beside it.

## Phase 6 — the ADR and the questions

- [x] The **ADR** records the decision.
- [x] The **next question** is written down where this one was: the README's Next row and
      `STATUS.md`'s last section — **either way**, as an answer that separates and as one that does
      not.

## Phase 7 — the closure

- [ ] The spec's status is `done` with the date it finished, and CI is green on the publish commit.

## The gate

- [x] `cargo fmt`, clippy on arm64 and x86-64, the whole test suite, `cargo deny`, the wasm32 check,
      the published-figures guard and the page checks all green.
- [ ] CI green, and the spec's status is `done` with the date it finished.
