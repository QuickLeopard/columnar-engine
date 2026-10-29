# Spec 033 — The drift, taken apart — tasks

**Status**: in progress
**Spec**: [spec.md](spec.md)

## The measurement comes first

- [x] The **brackets and the reduction** are read out of the instrument's own code and written down:
      the **order of the blocks** — the query's first, the standalone's after it — what `Timing`
      **keeps** and what it **throws away**, and how the **context** and the **parts** are each
      reduced. The spec's tables are a **hypothesis**; they are confirmed against the code as it
      ships, and corrected if the code says otherwise.
- [x] The **drifts** of every control and of the **engine's own parts** — the **floor**, the
      **plain read**, the **standalone probe**, the **build side's whole**, and the engine's own
      **probe**, **build** and **open** — are computed from the **committed** reports and published,
      **before** anything is measured, **for every family the guard reads**. **This is the first of
      the two passes the derivation makes.**
- [x] The **yardstick's rule** is fixed **before** the new campaign is taken: for each **leaf**, the
      **larger** of the drifts of the sides it stands on, and of the **floor's**. The yardsticks are
      computed from the **committed** reports first, and published with the leaf they judge.
      **The rule is not chosen by looking at any verdict.** **This is the pre-registration** — the
      yardsticks that fix the rule before the campaign are these, and **nothing is compared with the
      new set's yardstick before it exists**. The **costs** — the order's own cost and the
      interleaving's added work — are compared with **the floor's own spread**.
- [x] The **band** — the context's own spread — is published as what it is: **a difference of two
      drifting series**, beside the drift it is compared against.
- [x] The **soundness check** is written down: a yardstick narrower than the machine's own movement
      is **not sound**, and a verdict published when **no leaf could be judged at all** is
      **`inconclusive`**. **Under R2's rule the yardstick is sound by construction**, so the state
      annotates the legacy band-based verdict and catches a yardstick a later change made narrower.
- [x] The instrument measures the **two sides in the same iteration, back to back**, and the **order
      is alternated** between the iterations. The **order's own cost** is measured and published, and
      it is compared with **the floor's own spread** — the one number every cost is compared with,
      because the yardstick is per leaf and a cost is not a leaf.
- [ ] The **added work's cost** — the interleaving's — is measured the way spec 030 measured the
      added clock, published, and compared with **the floor's own spread**. **A route that costs
      more than the floor's own spread is refused, with the reason published.**
- [x] The **build side's file state is measured first, and stays measured first**: the query's build
      side and the standalone's still read the same warm file, and the build side's gap still
      measures what it measures today. **A route that would change the file state's bracket is
      refused, with the reason published.**
- [x] The report carries **every part's per-iteration values** and **the time each block took**, so
      that the ladder's reduction and the parts' correlation are checkable by a reader from the
      committed report, and **the separation between the two sides of one difference** is measurable
      and published. **There is no drift between two blocks to publish, because R4 leaves no blocks
      to drift apart.**
- [x] The **controls** are reported beside every set and are **never summed**. **The floor is not
      made quieter to make the yardstick tighter.**
- [ ] The **drifts, the yardsticks and the states are derived, not carried**: a report is one
      campaign and a drift is a range over three, so `scripts/medians.py` derives them from the
      three committed reports and `scripts/check-published-figures.py` checks the published figures
      against that derivation. **The report carries only what one campaign knows.** **The derivation
      runs twice — once over the committed reports, to fix the rule before the campaign, and once
      over the new set, to publish it.**
- [ ] A **missing measurement is published as missing, not as a zero** — the sets that predate the
      plan output's parts carry no engine parts, and the sets that predate the context carry no
      band; they publish no ladder.
- [ ] The **published phrases** name their level: the **leaf's** state as "the {leaf}, on the {path}
      path, is **{state}**" and the **verdict's** state as "the verdict's state, on the {path} path,
      is **{sound|not sound|inconclusive}**". **One vocabulary per level**, and the guard checks
      each as a phrase of its own.
- [ ] Every part's rows are checked against the engine's rows — the **216,783** the join produced and
      the six million the probe examined — **in order**, before anything is timed, and the aggregate's
      value against the published one.
- [ ] The **ladder is reconciled**: the parts sum to the context, and **what is left unexplained is
      named** and published with its spread.
- [ ] The **sets are renamed before the campaign**: spec 030's sets become
      `join-cost-spec030-run*.json` and `join-cost-spec030-before-run*.json`, and the guard's set
      list is re-pointed at them — the **before** family is **kept**, its glob moves to
      `join-cost-spec030-before-run*.json`, and spec 030's own set is added beside it. **The other
      four families — spec 029's two and spec 028's two — stay as they stand**, because a family
      that disappears would **uncheck a published figure**. **A family whose glob matches nothing
      fails the guard**, so the family is re-pointed, not dropped. **The rename and the campaign
      land in the same commit**, so no glob is ever empty.
- [ ] The **campaign** is taken: three campaigns, five iterations each, in one session, with the
      **floor** in every campaign.
- [ ] Every **leaf** is published with its **value**, its **spread**, its **yardstick** and its
      **state**: **a part of the context**, **at the noise**, or **inconclusive**.
- [ ] The **verdict** is published in one of R2's words — **the build side's**, **the probe's**,
      **the clock's**, **the memory's**, **the host's**, or **unattributed** — **and** in one of R3's
      states, with the yardstick it was judged against and the drifts that made it.
- [ ] A leaf whose own spread is **wider than its yardstick** is published as **inconclusive** and is
      not counted towards the verdict.
- [ ] **There is one verdict per set and path.** The band-based verdict spec 030 published is kept
      beside the new one **as the figure it was**, annotated **not sound** — and it is **not**
      published as a second verdict for the same path.

## The change

- [ ] The change is **only** what the measurement supports: the **instrument** changes and the
      **engine does not**. The query path, the plan, the plan output's shape and the public API are
      untouched.
- [ ] The lookup's shape, the `#[inline]`, the build side's read, the table's shape, the overflow
      map's semantics, both hashers and the hashed path are **untouched**.
- [ ] The **old code is not re-measured** and no A/B is taken: the code that ships is the code spec
      030 shipped. Spec 030's set is the **before**, preserved under its own name.

## The answer does not change

- [ ] The join's rows, their order, their count and the aggregate's value are the **same** — the same
      rows in the same order, with the same count a key, and a NULL key that still never matches.
- [ ] `a_build_key_more_than_255_rows_carry_still_counts_every_one` — the overflow map's semantics.
- [ ] `a_probe_key_below_the_table_s_base_matches_nothing` — the range rule.
- [ ] `a_null_key_never_matches_on_either_side` — the NULL rule.
- [ ] `the_direct_table_agrees_with_the_hash_table` and
      `the_overflow_map_is_allocated_only_when_a_key_overflows`, in `operators/join.rs`, keep holding.
- [ ] The unit test inside `operators/join.rs` is **read again** after any change, not only run.
- [ ] No published **query** figure moves, and the **comparison campaigns are not taken again**, and
      the reason is recorded.
- [ ] The **query's cost** is measured against the **floor's own spread** on both paths, and the
      difference is published.

## The publication

- [ ] Every figure that moved is published **old-and-new**, with the reason, in every place it is
      published: `PERF.md` §3o, the README's Next row, and `STATUS.md`'s last section. **The verdict's
      word and its state are figures too**, and if the word moves it is published with both.
- [ ] **The guard's changes come first** — the yardstick's recompute, the verdict's re-derivation,
      **the states' derivation and their phrases**, and the set list — **and then the documents**,
      written to the phrases the guard checks.
- [ ] The guard's **verdict is re-derived against the yardstick**, not against the band: it derives
      the yardstick from the drifts by R2's rule, judges every leaf against it, and checks that the
      **published** yardstick is the one it derived. The **state** is checked beside it, **as a
      phrase of its own**.
- [ ] The guard derives the ladder for **both** sets whose engine reports the parts — this
      increment's set and spec 030's — so that spec 030's ladder is checked against the yardstick
      and not only quoted.
- [ ] The **yardstick and the drifts** are published inside `PERF.md` §3o **under no new `###`
      heading**, and every figure is added to the guard's parts or differences list **with its
      decimal places**.
- [ ] The **verdict's state** is published as a word of its own beside `unattributed`, and the guard
      checks it.
- [ ] `scripts/medians.py` derives the drifts, the yardsticks and the verdict's state, and both it and
      `scripts/check-published-figures.py` derive the drift the same way. **The states are derived
      the same way too**, or the check is not a check.
- [ ] A join-cost set with **fewer than three campaigns fails the guard**, and a family is never
      skipped silently.
- [ ] **The README's Next row and `STATUS.md`'s probe row are re-derived** — the guard builds
      literal phrases for both, and a phrase left behind fails the guard.
- [ ] **The conclusion is restated, not only the figures**: the documents say today that "the context
      is a real thing" on the resident path, on the strength of a band of **0.418 ms** that R3 says
      was **not sound**. The sentence is restated with the reason, in `PERF.md` §3o, in the README's
      Next row and in `STATUS.md`'s open question.
- [ ] **`STATUS.md`'s join probe row** is updated too — the stage, the floor, the probe, the build
      side's whole, the remainder and the context all move with the set — and the guard's checks on
      that row are satisfied.
- [x] **Spec 030's header records the supersession** of its R4, with the reason, so that the reason
      is recorded in the spec that is amended as well as in this one.
- [ ] `bench-results/README.md` gains a row per new set, and its §3o row lists the new globs.
- [ ] The **resolution** is published: what would make the parts separable, or — if no resolution on
      this host does — the resolution that **would be** needed, bounded below by the machine's drift.
- [ ] The **next question** is written down where this one was — the README's Next row and
      `STATUS.md`'s last section — **either way**.
- [ ] This spec's ADR records the decision, and the raw reports are committed, and
      `bench-results/README.md` carries the new set's row, and **this spec's header gains its `ADR`
      line** and its References its ADR row. **The ADR lands in the same change as the publication
      that names it, because `STATUS.md`'s ADR count is a checked figure and it moves with it.**

## The gate

- [ ] `cargo fmt`, clippy on arm64 and x86-64, the whole test suite, `cargo deny`, the wasm32 check,
      the published-figures guard and the page checks all green.
- [ ] CI green, and the spec's status is `done` with the date it finished.
