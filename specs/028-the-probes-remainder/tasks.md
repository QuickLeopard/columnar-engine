# Spec 028 — The probe's remainder

**Status**: in progress
**Spec**: [spec.md](spec.md)

## The measurement comes first

- [ ] The instrument gains the **candidate with the overflow consult**: the same hand-written lookup, over
      the same rows and the same table, with the map's consult in it. The map's cost becomes a measured
      number.
- [ ] The instrument gains the **cold candidate**: the same hand-written lookup inside the reader's batch
      loop, so that the cold path's remainder is measurable at all.
- [ ] The **before** campaigns are taken on the code this spec replaces, back to back with the **after**
      ones, on the same host: three campaigns each, five iterations each, the load average recorded.
- [ ] The remainder's **attribution** is published — the map's share and the rest — and the stage's own
      probe is derived on both paths: the stage, less the build side's whole and the open.
- [ ] **If the remainder does not shrink to nothing, the rest is named**, and if the probe does not get
      faster, that is the finding and it is published as it is.

## The change

- [ ] `DirectTable` carries whether its overflow map holds anything, and the fact is captured **after**
      the build loop — never before it (R7).
- [ ] The lookup does not call the map at all when the map holds nothing: no `get`, no hash of the key,
      no read of any field of the map (R1).
- [ ] The map is **not allocated** when nothing overflows; its key and value types and its hasher do not
      change (R1, R6).
- [ ] The hashed path is untouched; the direct table's slow path shares the lookup and changes with it
      (R2).

## The answer does not change

- [ ] A unit test **inside `operators/join.rs`** — the tables are private — holds a `DirectTable` over a
      key that carries 300 build rows and a key that carries three against a `HashedTable` over the same
      keys, compared key by key (R2).
- [ ] *a build key more than 255 rows carry still counts every one* — the overflow map's semantics.
- [ ] *a probe key below the table's base matches nothing* — the range rule, and the fact captured after
      the build loop.
- [ ] *a NULL key never matches on either side* — the NULL rule.
- [ ] The join's rows, their order and their count are the same rows; the aggregate's value is the same
      number.
- [ ] The build side's estimate is unchanged, and its tests pass unchanged (R5).

## The publication

- [ ] The comparison campaigns and the join probe's campaigns are **preserved under their own names** —
      `compare-spec026-run*.json`, `join-cost-spec026-run*.json` — and the new sets take the current
      names (R4).
- [ ] `scripts/medians.py` derives the tables from all three sets; `scripts/check-published-figures.py`
      checks every published figure, **the old set's and the new set's**, against the reports, in the
      section that publishes it.
- [ ] The **resident** `join-orders` figures are added to the guard, which today checks the cold one
      only.
- [ ] `PERF.md` §2a, §2b, §2h, §3l and §3o: every figure that moved is published old-and-new with the
      reason, and each table carries the old value it replaces.
- [ ] `README.md`: the status table's row, the prose, and the next question.
- [ ] [ADR-0035](../../docs/adr/0035-the-join-stage-taken-apart.md) is amended, and
      [ADR-0036](../../docs/adr/0036-the-probes-remainder-taken.md) records the decision and the successor
      to its "published, not taken".
- [ ] The raw reports are committed, and `bench-results/README.md` carries the new sets' row.

## The gate

- [ ] `cargo fmt`, clippy on arm64 and x86-64, the whole test suite, `cargo deny`, the wasm32 check, the
      published-figures guard and the page checks all green.
- [ ] CI green, and the spec's status is `done` with the date it finished.
