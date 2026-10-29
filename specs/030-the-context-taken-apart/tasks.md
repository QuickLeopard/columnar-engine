# Spec 030 — The context, taken apart — tasks

**Status**: done
**Spec**: [spec.md](spec.md)

## The measurement comes first

- [x] The **brackets** are read out of the engine's own code and written down: for the stage on both
      paths, for the standalone probe on both paths, and for the standalone build side — what each
      clock **brackets** and what it **does not include**. The spec's table is a **hypothesis**; it is
      confirmed against the code as it ships, and corrected if the code says otherwise.
- [x] Every difference between the two brackets is its own **named part**, and a part that compares
      two different intervals says so on its own row. **The cold stage's clock is a sum of two
      disjoint intervals**, not a span, and the standalone cold probe carries the **row-number offset
      arithmetic** that the engine's own probe clock does not.
- [x] **R3's route** is chosen on the evidence — the plan output's parts, or the marginal query — and
      the choice is recorded with the reason.
- [x] The **marginal route** is attempted only on the same file and the same build side, and only if
      the build side's in-query cost is **shown** to be the same in both queries. **A marginal cost is
      not a total**: the fixed cost the two queries share is **measured, not assumed**.
- [x] The **plan-output route**'s added clock's own cost is measured and published, and the route is
      **refused** if it is outside R3's threshold — **the floor's own spread across the campaigns of
      that set** — with the reason published.
- [x] The instrument gains the **parts**: the build side's gap, the build side's file state, the
      probe's gap, the probe's file state, the probe's row-number offset, and the clock's bracket —
      one named part at a time. Each part differs from the last by **exactly one** thing.
- [x] The **controls** — the **floor** and the **scan's stage** — are reported beside every set and
      are **never summed**. **The context itself is not a part.**
- [x] Every part's rows are checked against the engine's rows — the **216,783** the join produced and
      the six million the probe examined — **in order**, before anything is timed, and the aggregate's
      value against the published one.
- [x] Every part's **own spread** is published beside its value.
- [x] The **ladder is reconciled**: the parts sum to the context, and **what is left unexplained is
      named** and published with its spread.
- [x] The **interleaved A/B** is taken: three campaigns each, old and new alternating, in one session,
      with the **floor** in every campaign. The instrument is **byte-identical** in both sets; only the
      engine's own file changes.
- [x] The report **names the code** each campaign measured. The commit alone does not distinguish the
      two codes — spec 029's two sets carry the same commit.
- [x] The **verdict** is published in one of R4's six words — **the build side's**, **the probe's**,
      **the clock's**, **the memory's**, **the host's**, or **unattributed** — with the context's own
      spread in each set as the band, and each set judged against its own band.
- [x] A part whose own spread is **wider than the band** is published as **inconclusive** and is not
      counted towards the verdict; a part inside its own spread is published as **at the noise**.
- [x] **"Unattributed" means no part accounted for more of the context than the band**, and the
      unexplained remainder is published separately and is **not** the verdict.

## The change

- [x] The change is **only** what the measurement supports, and only if the measurement supports
      anything: the engine's **query path** does not change.
- [x] A change that makes the query slower by more than **the floor's own spread** is not a change,
      and the figure says so.
- [x] The lookup's shape, the `#[inline]`, the overflow map's semantics, both hashers and the hashed
      path are **untouched**.

## The answer does not change

- [x] The join's rows, their order, their count and the aggregate's value are the **same** — the same
      rows in the same order, with the same count a key, and a NULL key that still never matches.
- [x] `a_build_key_more_than_255_rows_carry_still_counts_every_one` — the overflow map's semantics.
- [x] `a_probe_key_below_the_table_s_base_matches_nothing` — the range rule.
- [x] `a_null_key_never_matches_on_either_side` — the NULL rule.
- [x] `the_direct_table_agrees_with_the_hash_table` and
      `the_overflow_map_is_allocated_only_when_a_key_overflows`, in `operators/join.rs`, keep holding.
- [x] The unit test inside `operators/join.rs` is **read again** after any change, not only run.

## The publication

- [x] Every figure that moved is published **old-and-new**, with the reason, in every place it is
      published: `PERF.md` §2h and §3o, the README's Next row, and `STATUS.md`'s last section.
- [x] The **sets are preserved under their own names** — `join-cost-spec029-run*.json`,
      `join-cost-spec029-before-run*.json` — and the new sets take the current names.
- [x] The **ladder is published inside `PERF.md` §3o under no new `###` heading**, and every part's
      figure is added to the guard's parts or differences list **with its decimal places**.
- [x] The **context's spread** is published, and both `scripts/medians.py` and
      `scripts/check-published-figures.py` derive it across the campaigns the same way.
- [x] A join-cost set with **fewer than three campaigns fails the guard**, and a family is never
      skipped silently.
- [x] The **README's Next row's context figures** are added to the guard, which checks three phrases
      in that row today and not these.
- [x] `bench-results/README.md` gains a row per new set, and its §3o row lists the new globs.
- [x] The **comparison campaigns are not taken again**, and the reason is recorded.
- [x] The **next question** is written down where this one was — the README's Next row and
      `STATUS.md`'s last section — **either way**: a context attributed to the clock's bracket is an
      answer, and an **unattributed** context is published with the numbers that show it.
- [x] This spec's ADR records the decision, and the raw reports are committed, and
      `bench-results/README.md` carries the new sets' row.

## The gate

- [x] `cargo fmt`, clippy on arm64 and x86-64, the whole test suite, `cargo deny`, the wasm32 check,
      the published-figures guard and the page checks all green.
- [x] CI green, and the spec's status is `done` with the date it finished.
