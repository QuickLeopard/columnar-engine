# Spec 028 — The probe's remainder

**Status**: in progress
**Spec**: [spec.md](spec.md)

## The measurement comes first

- [x] The instrument gains the **candidate with the overflow consult**: the same hand-written lookup, over
      the same rows and the same table, with the map's consult in it. The map's cost becomes a measured
      number.
- [x] The instrument gains the **cold candidate**: the same hand-written lookup inside the reader's batch
      loop, so that the cold path's remainder is measurable at all.
- [x] The **before** campaigns are taken on the code this spec replaces, back to back with the **after**
      ones, on the same host: three campaigns each, five iterations each, the load average recorded.
- [x] The remainder's **attribution** is published — the map's share and the rest — and the stage's own
      probe is derived on both paths: the stage, less the build side's whole and the open.
- [x] **If the remainder does not shrink to nothing, the rest is named**, and if the probe does not get
      faster, that is the finding and it is published as it is.
      — **The remainder did not shrink to nothing, and the rest is named: the lookup's shape. The
      map's share is 0.020 ms of a 3.337 ms remainder — at or below the noise. The probe's own
      measurement got 0.950 ms faster and the query did not, because the parts are bounds and the
      stage is the authority; the resident stage moved 1.4 ms the other way, reproducibly, and the
      parts cannot account for it. It is published as it is.**

## The change

- [x] `DirectTable` carries whether its overflow map holds anything, and the fact is captured **after**
      the build loop — never before it (R7).
- [x] The lookup does not call the map at all when the map holds nothing: no `get`, no hash of the key,
      no read of any field of the map (R1).
- [x] The map is **not allocated** when nothing overflows; its key and value types and its hasher do not
      change (R1, R6).
- [x] The hashed path is untouched; the direct table's slow path shares the lookup and changes with it
      (R2).

## The answer does not change

- [x] A unit test **inside `operators/join.rs`** — the tables are private — holds a `DirectTable` over a
      key that carries 300 build rows and a key that carries three against a `HashedTable` over the same
      keys, compared key by key (R2).
- [x] *a build key more than 255 rows carry still counts every one* — the overflow map's semantics, and
      the fact captured after the build loop.
- [x] *a probe key below the table's base matches nothing* — the range rule.
- [x] *a NULL key never matches on either side* — the NULL rule.
- [x] The join's rows, their order and their count are the same rows; the aggregate's value is the same
      number.
- [x] The build side's estimate is unchanged, and its tests pass unchanged (R5).

## The publication

- [x] The comparison campaigns and the join probe's campaigns are **preserved under their own names** —
      `compare-spec026-run*.json`, `join-cost-spec026-run*.json` — and the new sets take the current
      names (R4).
- [x] `scripts/medians.py` derives the tables from all three sets; `scripts/check-published-figures.py`
      checks every published figure, **the old set's and the new set's**, against the reports, in the
      section that publishes it.
- [x] The published comparison campaigns' **resident** join stage — **21.3 ms** — is added to the guard,
      which today checks the cold path's stages and §3o's own resident stage.
- [x] The resident **query** figure is **not** added to the guard: the engine's resident path reads the
      build side inside its clock and the incumbents do not, so the two are not the same measurement —
      spec 020's rule, and the figure stays in the prose.
- [x] `PERF.md` §2a, §2b, §2h, §3l and §3o: every figure that moved is published old-and-new with the
      reason, and each table carries the old value it replaces.
- [x] `README.md`: the status table's row, the prose, and the next question.
- [x] [ADR-0035](../../docs/adr/0035-the-join-stage-taken-apart.md) is amended, and
      [ADR-0036](../../docs/adr/0036-the-probes-remainder-taken.md) records the decision and the successor
      to its "published, not taken".
- [x] The raw reports are committed, and `bench-results/README.md` carries the new sets' row.

## The gate

- [ ] `cargo fmt`, clippy on arm64 and x86-64, the whole test suite, `cargo deny`, the wasm32 check, the
      published-figures guard and the page checks all green.
- [ ] CI green, and the spec's status is `done` with the date it finished.
