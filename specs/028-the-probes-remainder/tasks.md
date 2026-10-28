# Spec 028 — The probe's remainder

**Status**: in progress
**Spec**: [spec.md](spec.md)

## The change

- [ ] `DirectTable` carries whether its overflow map holds anything, and the per-row lookup does not
      consult a map that holds nothing — no hash, no bucket probe, no read of the map's own state.
- [ ] The map's shape and its semantics are unchanged: a key with more than 255 build rows still counts
      exactly.
- [ ] The general path — strings, dictionaries, columns with NULLs — is untouched.

## The answer does not change

- [ ] The direct table's answer is the hash table's answer, bit for bit, for a key that overflows and a
      key that does not: a test holds the two to that.
- [ ] The join's rows, their order and their count are the same rows.
- [ ] A NULL key never matches.
- [ ] The aggregate's value is the same number.

## The measurement

- [ ] The instrument measures the engine's probe **before** and **after**, the candidate, and the floor,
      on the same host, back to back.
- [ ] The **remainder** is published: what is left of the **3.095 ms** after the change, and the rest
      **named** if it does not shrink to nothing.
- [ ] The stage, cold and resident, is published old code and new code.
- [ ] The comparison campaigns are re-measured, for the code and for the code it replaces, back to back on
      the same host, because the cold `join-orders` is a published query time and this spec moves it.

## The publication

- [ ] `PERF.md`: the figures are published, and every figure that moved is published old-and-new with the
      reason it moved.
- [ ] `README.md`: the status table carries the spec, and the next question is written down.
- [ ] `scripts/medians.py` derives the new tables; `scripts/check-published-figures.py` checks every
      published figure against the committed reports, **in the section that publishes it**.
- [ ] The raw reports are committed.

## The gate

- [ ] `cargo fmt`, clippy on arm64 and x86-64, the whole test suite, `cargo deny`, the wasm32 check, the
      published-figures guard and the page checks all green.
- [ ] CI green, and the spec's status is `done` with the date it finished.
