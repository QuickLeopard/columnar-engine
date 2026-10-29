# Spec 029 — The lookup's shape, and the stage that moved — tasks

**Status**: done
**Spec**: [spec.md](spec.md)

## The measurement comes first

- [x] The instrument gains the **ladder**: the **chain**, the **width**, the **state over `&self`**, the
      **map's option**, and the **engine's own lookup** over the instrument's own key source. Each rung is
      the plain candidate plus **exactly one** thing.
- [x] Every rung's rows are checked against the engine's rows, **in order**, before anything is timed, and
      the aggregate's value against the published one.
- [x] The **remainder is reconciled against the ladder** — each increment, each increment's spread, and what
      is left of the remainder **unexplained** named. The ladder's sum is the remainder and what is left
      unexplained is **0.000 ms**.
- [x] The **interleaved A/B** is taken with the **instrument**: three campaigns each, old and new
      alternating, in one session, with the **floor** in each campaign. The instrument is **byte-identical**
      in both sets; only the engine's own file changes.
- [x] The **verdict** on the resident stage's movement is published in one of three words — **the host's**,
      **the code's**, or **unattributed** — with the floor, the cold stage and the scan's stage as the
      controls, and the **context** named as what the residual's gap to the standalone probe is. **The
      verdict is the host's.**
- [x] If the remainder does not shrink to nothing, the rest is named; if the probe does not get faster,
      that is the finding and it is published as it is. **The remainder is 0.109 ms** and the shape is
      free, so the rest is named and published.

## The change

- [x] The direct table's **fast path** is changed **only** in what the measurement supports, and only if the
      measured saving is **larger than the spread of the remainder** in the set that measures it. **The
      remainder's spread is 1.083 ms and every part of the shape is inside it, so the lookup does not
      change.**
- [x] The change must not make the probe **slower**. The lookup is marked `#[inline]`, which changes
      nothing the query does and makes the instrument's measurement the query's cost.
- [x] The overflow map's **semantics** do not change; the map is still an `Option`, and still **not
      allocated** when nothing overflows.
- [x] The **hashed** path is untouched; the direct table's **slow path** shares the lookup and changes with
      it, and its **semantics** do not.

## The answer does not change

- [x] The unit test **inside `operators/join.rs`** keeps holding the direct table's answer against the hash
      table's, key by key, over a key that carries 300 build rows and a key that carries three — and it is
      **read again** after the change, not only run.
- [x] *a build key more than 255 rows carry still counts every one* — the overflow map's semantics.
- [x] *a probe key below the table's base matches nothing* — the range rule.
- [x] *a NULL key never matches on either side* — the NULL rule.
- [x] The join's rows, their order and their count are the same rows; the aggregate's value is the same
      number.
- [x] The build side's estimate is unchanged, and its tests pass unchanged.

## The publication

- [x] Every figure that moved is published **old-and-new**, with the reason, in every place it is
      published: `PERF.md` §2a, §2b, §2h, §3l, §3o, and the README's status table, prose and next question.
- [x] The **four stale statements** are corrected where they are published — the shape list, the A/B's
      figures, the scan's stage, and the yardstick's set — and each correction says what it was.
- [x] The **sets are preserved under their own names** — `join-cost-spec028-run*.json`,
      `join-cost-spec028-before-run*.json` — and the new sets take the current names.
- [x] `scripts/medians.py` derives the ladder's rungs and the reconciliation; `scripts/check-published-figures.py`
      checks every published figure, **the old set's and the new set's**, against the reports, in the
      section that publishes it.
- [x] The **resident** query figure is still **not** in the guard — spec 020's rule — and the reason is
      recorded.
- [x] The **prize** is named as the stage's own probe, and the standalone probe's remainder is published as a
      **bound**.
- [x] [ADR-0036](../../docs/adr/0036-the-probes-remainder-taken.md) is amended, and this spec's ADR records
      the decision.
- [x] The raw reports are committed, and `bench-results/README.md` carries the new sets' row.

## The gate

- [x] `cargo fmt`, clippy on arm64 and x86-64, the whole test suite, `cargo deny`, the wasm32 check, the
      published-figures guard and the page checks all green.
- [x] CI green, and the spec's status is `done` with the date it finished.
