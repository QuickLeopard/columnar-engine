# Spec 036 — The file-state floor — tasks

**Status**: in progress
**Spec**: [spec.md](spec.md)

## Phase 1 — the instrument

- [x] The **build side's read** moves into the iteration, beside the parts: the engine's own reader,
      the engine's own predicates, the engine's own columns, timed on its own. It is the **file-state
      floor**.
- [x] The **machine's own floor** moves into the iteration too, over the **same fresh column** the
      engine's own probe has just used, so it reads nothing and perturbs nothing.
- [x] The report carries both series **per iteration**, and the figures that move are published old
      and new with the reason.

## Phase 2 — the pairing

- [x] The **paired** figure — the file's own read less the machine's own movement, iteration by
      iteration — is published for every iteration.
- [x] The **attribution** is answered: the excursions are the file's state or the machine's, with the
      per-iteration figures that show it.
- [x] The **order's own cost** and the **added clock cost** are published for the new series: what it
      costs to measure the file's state is a figure, not a caveat.

## Phase 3 — the yardstick

- [x] The yardstick is **file-aware**: the sum of the drifts of the measurements a leaf stands on, the
      machine's floor's drift, and the file-state floor's drift — the larger of them.
- [x] The leaves' **verdicts** are re-answered against it, on the new set, and the verdicts under the
      old yardstick are published beside them.

## Phase 4 — the campaigns

- [x] Three campaigns of the new instrument, on this machine, the raw reports committed under their
      own names.
- [x] The standing set is **not** re-taken and does **not** slide; the new set is published beside it.

## Phase 5 — the publication

- [x] `PERF.md` publishes the new set's ladder, the file-aware yardstick, the attribution and the
      correction, and the guard checks every figure **inside the section that publishes it**.
- [x] `scripts/medians.py` prints the new set beside the others.
- [x] The **correction** is published where the wrong claim is: the old pairing and the new one, with
      the reason.

## Phase 6 — the ADR and the questions

- [x] The **ADR** records the decision and the correction.
- [x] The **next question** is written down where this one was: the README's Next row and
      `STATUS.md`'s last section — **either way**, as an answer that separates and as one that does
      not.

## Phase 7 — the closure

- [ ] The spec's status is `done` with the date it finished, and CI is green on the publish commit.

## The gate

- [ ] `cargo fmt`, clippy on arm64 and x86-64, the whole test suite, `cargo deny`, the wasm32 check,
      the published-figures guard and the page checks all green.
- [ ] CI green, and the spec's status is `done` with the date it finished.
