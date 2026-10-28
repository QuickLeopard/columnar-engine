# Spec 028 — The probe's remainder

**Status**: in progress
**Started**: 2026-10-28
**Predecessor**: [026-the-join-stage-taken-apart](../026-the-join-stage-taken-apart/spec.md)

## The problem

Spec 026 took the join's stage apart, and the third of its three findings is this spec's question.

The stage is the **build side's read** — **11.83 ms** of the cold stage's **20.74 ms**, more than half of
it — and the **probe**, **10.05 ms** over six million rows. The build side's read is Arrow's decode of
`orders.parquet`: measured three times, and **refused every time**.

**The probe is not at the floor.** The engine's probe over its own column costs **10.051 ms**; the same
lookup, written out by hand over the same six million rows and the same table, costs **6.959 ms**. The
difference is **3.095 ms** — the probe's remainder.

**And the code names the one thing the engine's per-row lookup does that the candidate's does not.** The
candidate reads the table's byte and stops:

```rust
let count = counts.get(offset).copied().unwrap_or(0);
```

The engine's `DirectTable::count` reads the same byte **and consults the overflow map**:

```rust
let byte = self.counts.get(index).copied().map_or(0, u32::from);
let extra = self.overflow.get(&key).copied().unwrap_or(0);
byte.saturating_add(extra)
```

The map exists for one case: a key with more than **255** build rows does not fit in its byte, so the byte
holds 255 and the map holds the rest. The map is consulted on **every** probe row — six million times over
the probe side — whether it holds anything or not, and for this data it holds **nothing**: every key's
count fits in its byte.

**The prize is the join's only remaining gap.** The engine's cold `join-orders` is **96.7 ms** against
DuckDB's **95.0 ms** — level, **1.7 ms** apart, inside both spreads — while the engine is **1.7×** ahead of
the peer and **2.3×** ahead of `Polars`. The remainder is **3.095 ms** of the engine's own work: taking it
out puts the cold join at **93.6 ms**, ahead of DuckDB on the median, and takes the resident join's stage
down by the same.

**And the remainder has not been taken apart.** Spec 026 measured the probe and the candidate — the whole
lookup — and named the difference between them. It did not say **how much of the 3.095 ms** is the map's
empty check and how much is the rest of the engine's per-row work. A spec that removes the check and a
spec that removes nothing would both be called "3.10 ms", and only one of them would be true.

## The requirements

### R1 — The table knows whether its overflow map holds anything

**The map is written to only when a byte fills, and the table must carry that fact into the lookup.**

The direct-addressed table holds one byte a count over the key's range, and a count that does not fit in
a byte overflows into a map. The construction already knows whether anything overflowed — the map is
touched only when a byte fills — and that fact must be carried into the table, so that the per-row lookup
does not consult a map that holds nothing.

The lookup, when the map is empty, must not **hash** the key, must not **probe** a bucket, and must not
read the map's own state. The map's **shape** is not changed: it is still a map from the key to the rows
its byte could not hold, and a key with more than 255 build rows still counts exactly.

### R2 — The answer does not change

**The same rows, in the same order, with the same count a key.**

The direct table's answer must be the hash table's answer, bit for bit, for keys that overflow and keys
that do not. A test holds the two to that, with a key carrying more than 255 build rows and one that does
not. The **general path** — strings, dictionaries, and any column with a NULL in it — is untouched.

### R3 — The remainder is measured, part by part

**The published figure is the one the measurement gives, not the one the difference suggests.**

On the same host, back to back against the code it replaces, the instrument measures:

- the probe over the engine's column, **before** and **after**;
- the **candidate**: the same lookup, written out by hand, over the same rows and the same table;
- the **floor**: the table's own load, one a probe row.

and publishes **what is left of the remainder** after the change. If the remainder does not shrink to
nothing, the figure says so and the rest is **named**, not rounded away.

### R4 — The figures are published, and every figure that moved is published old-and-new

**A figure that moved is published with the figure it replaced and the reason it moved.**

The join's stage, the probe and the remainder are published in
[`PERF.md`](../../PERF.md) §3o, old code and new code side by side, derived from the committed reports by
`python3 scripts/medians.py` and checked by `scripts/check-published-figures.py` **in the section that
publishes them**.

**The comparison campaigns are re-measured**, for the code and for the code it replaces, back to back on
the same host. The cold `join-orders` is a published query time, and this spec moves it: the campaign
that says so is the one that is taken. Every figure that moved is published old-and-new with the reason,
and every figure that did not move is left where it is.

## The scope

**What this spec does not do, and why.**

- **The table's shape is not changed.** One byte a count over the key's range was chosen by measurement in
  spec 021 and beat every candidate in that campaign. This spec removes a lookup, not a table.
- **The overflow map's semantics are not changed.** A key with more than 255 build rows still counts
  exactly, and the map still holds what the byte cannot.
- **The probe's hash is not changed.** `FxHash` was chosen by measurement in spec 021 and is already the
  reason the probe runs at about a tenth of the default hasher's cost.
- **The general path is not changed.** Strings, dictionaries and columns with NULLs take the general path
  and are untouched.
- **The scan's stage is not changed.** It is **73.6 ms** of the cold query, and it is Arrow's decode —
  measured three times and refused every time.
- **The build side's read is not changed, and the pruning idea is dead.** It is **11.83 ms** of the stage,
  and it is Arrow's decode of `orders.parquet`. A row group whose statistics exclude the date predicate
  could be skipped, and the engine's estimate already uses those statistics — but **`orders.parquet` has
  two row groups, and both of them carry `o_orderdate` from 1992-01-02 to 1998-12-01**. Neither can be
  excluded, so the build side reads every row of the file and there is nothing to skip. That is the file's
  own statistics saying so, not a guess.
- **The plan is not changed.** No new plan, no new operator, no new join type.

## The instrument

`crates/columnar-engine/examples/join_cost_probe.rs`, committed in the tree by spec 026, with the engine's
probe measured **after** the change as well as before, and the campaigns taken **before** and **after**,
back to back, on the same host. The engine's **216,783** rows are checked against the probe's own, in
order, before anything is timed, and the aggregate's value is checked against the published one.

## The measurement

To be filled from the instrument's campaigns: the stage, the probe, the remainder and the comparison
campaigns, with every figure that moved published old-and-new.

## The references

| | |
|---|---|
| [ADR-0021](../../docs/adr/0021-the-materialised-column-and-a-rust-peer.md) | the peer the engine is compared against, and how the two are held to the same conditions |
| [ADR-0028](../../docs/adr/0028-the-hot-paths-taken-apart.md) | the join's probe taken apart the first time, and the table's shape chosen by measurement |
| [ADR-0030](../../docs/adr/0030-the-scan-that-feeds-the-join.md) | the scan that feeds the join, and the rule that the rows are read where they lie |
| [ADR-0035](../../docs/adr/0035-the-join-stage-taken-apart.md) | the join's stage, taken apart, and the remainder this spec is about |
| [spec 021](../021-the-hot-paths-taken-apart/spec.md) | the join's probe measured against its floor, and the table's shape chosen by measurement |
| [spec 026](../026-the-join-stage-taken-apart/spec.md) | the join's stage taken apart, and the **3.095 ms** remainder this spec measures |
| [`PERF.md`](../../PERF.md) §2a, §2b, §2h, §3n, §3o | the published tables this spec's figures are re-derived into |
