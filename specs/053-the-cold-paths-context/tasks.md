# Spec 053 — The cold path's context — tasks

**Status**: done
**Finished**: 2026-11-09
**Started**: 2026-11-09
**Spec**: [spec.md](spec.md)

The order is the house's: **the shape is measured before the verdict is given.** The floors were moved into the iteration
by spec 036; **the context's own two sides were not**, and the context is a difference between them — taken **266.705 ms**
apart on the cold path. **So the sides move into one account, and the verdict is given on a pairing that can carry it.**

## The pairing

- [x] The iteration becomes **two path accounts**, each holding the path's query and the path's standalone work **back to
      back**, with the order alternating between the iterations.
- [x] **And each account carries its own build side's work**, so that every leaf of that path's ladder is a difference of
      two measurements taken inside one account.
- [x] **And the separation is published** — how far apart the two sides of each difference were taken — **old and new**.

## The verdict

- [x] The cold path's context is published against the **file-aware yardstick**, beside the yardstick the old verdict
      used.
- [x] **And the file state's share of the context** is published: what is left of the context once the file's state is
      taken out.
- [x] **And the resident path's verdict is published beside it**, so the two paths are judged the same way.

## The answer

- [x] The verdict is published **either way** — attributed to the file state, or **unattributable at this spread** — and
      the question is **closed** either way.
- [x] **And the old figures are published beside the new ones**, with the reason: **the pairing moved**, and the old
      pairing's defect is named.

## The published figures

- [x] Every figure that moves is published **old-and-new**, with the reason -- **the pairing moved** -- and
      **re-measured, not recomputed**.
- [x] The **guard** checks the new figures **inside the section that publishes them**, and the old ones are checked
      against the **before** reports.

## The publication

- [x] A **new section** in `PERF.md`, the decision in an **ADR**, the status's question rewritten, and the raw reports
      **committed**.
- [x] `bench-results/README.md` gains a row per new set, and `docs/status.html` is rebuilt.
- [x] **And the sentences that rest on the old pairing are corrected where they were published** -- [§3q](../../PERF.md#3q-the-drifts-own-parts),
      [§3r](../../PERF.md#3r-the-file-state-floor) and [STATUS.md](../../STATUS.md)'s question 1 -- and the correction is
      recorded in the ADR.

## The gate

- [x] `cargo fmt`, clippy, the whole test suite, `cargo deny`, the wasm32 check, the published-figures guard, the links
      check, the package check and the dashboard check all green.
- [x] CI green, and the spec's status is `done` with the date it finished.

## What is not done

- **The wide shape's plain string's copy**, which is still not fixed: the conversion copies every string out of the
  Arrow batch, one allocation a row, and the dictionary path borrows instead. **That is the wide shape's gap**, and the
  fix wants its own measurement.
- **The crates.io publish**, which is a separate act and needs a token this repository does not have.
