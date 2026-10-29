# Spec 030 — The context, taken apart — tasks

**Status**: in progress
**Spec**: [spec.md](spec.md)

## The measurement comes first

- [ ] The **brackets** are read out of the engine's own code and written down: for the stage on both
      paths, for the standalone probe on both paths, and for the standalone build side — what each
      clock **starts before**, what it **stops after**, and what it **does not include**. The spec's
      table is a **hypothesis**; it is confirmed against the code as it ships, and corrected if the
      code says otherwise.
- [ ] Every difference between the two brackets is a **named rung**, and a rung that compares two
      different intervals says so on its own row.
- [ ] **R3's route** is chosen on the evidence — the plan output's parts, or the marginal query — and
      the choice is recorded with the reason. If neither can be taken **without changing the query's
      cost**, that is published with the numbers that show it, and the rest is named.
- [ ] The instrument gains the **ladder**: the build side's gap, the probe's gap, the clock's bracket,
      the memory's state, and the host's floor. Each rung differs from the one before it by
      **exactly one** thing.
- [ ] Every rung's rows are checked against the engine's rows — the **216,783** the join produced and
      the six million the probe examined — **in order**, before anything is timed, and the aggregate's
      value against the published one.
- [ ] The **ladder is reconciled**: the rungs sum to the context, and **what is left unexplained is
      named** and published with its spread.
- [ ] The **interleaved A/B** is taken: three campaigns each, old and new alternating, in one session,
      with the **floor** in every campaign. The instrument is **byte-identical** in both sets; only the
      engine's own file changes.
- [ ] The **verdict** is published in one of R4's five words — **the build side's**, **the probe's**,
      **the clock's**, **the host's**, or **unattributed** — with the context's own spread in each set
      as the yardstick, and each set judged against its own band.
- [ ] If the context is attributed to more than one mechanism, the **largest** is the verdict and the
      others are published beside it.

## The change

- [ ] The change is **only** what the measurement supports, and only if the measurement supports
      anything: the engine's **query path** does not change.
- [ ] If the plan output gains the join stage's parts, the clock it adds is **measured and
      published**, and the change is to what the engine **says**, never to what it **does**.
- [ ] A change that makes the query **slower** is not a change, and the figure says so.
- [ ] The lookup's shape, the `#[inline]`, the overflow map's semantics, both hashers and the hashed
      path are **untouched**.

## The answer does not change

- [ ] The join's rows, their order, their count and the aggregate's value are the **same** — the same
      rows in the same order, with the same count a key, and a NULL key that still never matches.
- [ ] *a build key more than 255 rows carry still counts every one* — the overflow map's semantics.
- [ ] *a probe key below the table's base matches nothing* — the range rule.
- [ ] *a NULL key never matches on either side* — the NULL rule.
- [ ] The unit test **inside `operators/join.rs`** keeps holding the direct table's answer against the
      hash table's, key by key, and it is **read again** after any change, not only run.

## The publication

- [ ] Every figure that moved is published **old-and-new**, with the reason, in every place it is
      published: `PERF.md` §2h and §3o, the README's Next row, and `STATUS.md`'s last section.
- [ ] The **sets are preserved under their own names** — `join-cost-spec029-run*.json`,
      `join-cost-spec029-before-run*.json` — and the new sets take the current names.
- [ ] `scripts/medians.py` derives the ladder's rungs and the reconciliation;
      `scripts/check-published-figures.py` checks every published figure, **the old set's and the new
      set's**, against the reports, in the section that publishes it.
- [ ] The **context's** own spread is published for both sets — the yardstick R4 names — and the
      **comparison campaigns are not taken again**, and the reason is recorded.
- [ ] The **next question** is written down where this one was — the README's Next row and
      `STATUS.md`'s last section — **either way**: a context attributed to the clock's bracket is an
      answer, and an **unattributed** context is published with the numbers that show it.
- [ ] This spec's ADR records the decision, and the raw reports are committed, and
      `bench-results/README.md` carries the new sets' row.

## The gate

- [ ] `cargo fmt`, clippy on arm64 and x86-64, the whole test suite, `cargo deny`, the wasm32 check,
      the published-figures guard and the page checks all green.
- [ ] CI green, and the spec's status is `done` with the date it finished.
