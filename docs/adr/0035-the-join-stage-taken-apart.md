# ADR-0035 — The join's stage, taken apart

**Status**: accepted
**Date**: 2026-10-28
**Spec**: [spec 026](../../specs/026-the-join-stage-taken-apart/spec.md)
**Predecessor**: [ADR-0034](0034-the-joins-rules.md)
**Successor**: [ADR-0036](0036-the-probes-remainder-taken.md) — the remainder this ADR published as a finding, **not taken**, is taken

## Context

The engine's cold `join-orders` is **96.7 ms** against DuckDB's **95.0 ms** — level, **1.7 ms** apart,
inside both spreads — while the peer's is **168.5 ms** and Polars' **222.8 ms**. The plan output says
where the engine's own time is, and it is the join's stage: **24.2 ms** of the cold query, a quarter
of it, and all of it the engine's own work.

**The stage had never been taken apart.** Spec 021 measured the join's *probe* — **9.0 ms** against a
**1.7 ms** floor — two increments ago, on a different code version and a different host state. The
build side's read was measured separately, at **9.99 ms**, as part of the *scan's* decomposition. No
measurement had ever put the stage and its parts in the same session, and none had ever asked what the
stage's *other* parts were.

**And the two paths did not agree.** The same parts are charged to the join's stage on both paths — the
build side's open, the table's construction, and the probe over six million rows — and they differ by
**2.9 ms**. Worse, they reported **different row counts for the same probe**: `216783 → 216783` on the
cold path and `6000000 → 216783` on the resident one. The two stages described different things.

**The review found that the measurement could not be made as it stood.** Five of the join's rules had
**two owners each**; the build side's memory estimate was **not an upper bound**, and the budget is
checked *before a row is read*; the join's helpers were left behind by spec 027's context; and two
documents said the opposite of the code. A stage that reports two different things for the same work
is not a stage that can be measured, and a budget checked against an estimate that is not an upper
bound is not a budget.

The instrument that took it apart — `crates/columnar-engine/examples/join_cost_probe.rs` — measured the
stage and its parts in the same session, on both paths, against the engine's own plan output. The
figures are the **medians of three campaigns**, run back to back on the same host, with the old code in
its own worktree; the raw reports are `bench-results/join-cost-run{1,2,3}.json` and
`bench-results/join-cost-before-run{1,2,3}.json`.

## The decision

### 1. The stage reports the rows the probe consumed

The reader reports how many rows the filter **examined**, and the join's stage reports that. The two
paths then say the same thing — `6000000 → 216783` — and describe the same work. The **times** did not
move: the change is in what the stage says, not in what it does.

### 2. Every rule of the join has one owner, and the other caller asks

The **read decision** lives in the **reader** — `ParquetSource::decide_read` — and every read asks it:
the probe side's scan with the plan's materialised set, and the build side with its own key as its
materialised set. It is in the reader and not in the scan because the build side is an *operator* and
the build side's *estimate* is a leaf: both are **below** the scan, and nothing below may depend on
anything above it. The pushdown's list of the types it evaluates exactly moved with it, into the
pushdown's own module; the probe's list of the types it can read is published by the key module and
asked for by the scan. The two lists are deliberately different questions. The type check is made once,
from one source. The NULL rule is stated once, in the interface both key sources implement, and the
interface says how long the column is, so that "the row is in range" has one answer — held by a
**differential test** that compares the two key sources, value for value, over NULLs, strings,
dictionaries, the plain-integer fast path and a row past the end.

### 3. The estimate is an upper bound, and it asks the reader

The build side's estimate is charged the way the read actually happens: the file's rows, the read set
the **reader** decides to read, the copies that read holds, the keys the join collects, and the
filter's own mask and selection. It asks the reader the same question the join asks, so the two cannot
drift apart — and it is **tested**, the way the other estimates are.

### 4. The join's helpers take the pipeline's context

`ordered_stage` and `join_outcome` took **seven** parameters each — a copy of the context's fields —
and now take the context and the two things that are not the context's. The table build is the **build
side's own method**: it runs before the scan, and the context is what the scan makes, so there is no
context to hand it yet.

### 5. The documents say what the code does, and the dead surface goes

`explain.rs` said the scan's elapsed carries the probe's cost; it does not. `memory.rs` said neither
the overflow case nor the overflow map is charged; the table is kept and the map **is** charged. Both
were corrected. `KeyColumn::from_decoded` and the whole of `KeySlice` — the dictionary variants that
were built and never read, and the integers that answered for a decoded column — went, and the sort's
fast path reads the column's own values through `plain_numbers` instead, covering exactly what it
covered before. The **join's** fast path is untouched.

## Consequences

**The stage is the build side's read and the probe, and the build side's read is more than half of
it.** The build side's whole is **11.831 ms** of the cold stage's **20.735 ms** — **57%** — and the
read itself is **11.542 ms**. It is Arrow's decode of the build side's columns out of `orders.parquet`,
and it has been measured three times and **refused every time**. This ADR does not touch it.

**The probe is not at the floor, and the code named a candidate for why.** The engine's probe over its
own column costs **10.051 ms**; the same lookup, written out by hand over the same six million rows and
the same table, costs **6.959 ms**. The difference is **3.095 ms**, and the engine's per-row lookup
named a candidate for it: `DirectTable::count` consulted the **overflow map** once a probe row, and the
map is **empty** for this data, because every key's count fits in its byte. **This was published as a
finding, not taken**: the table's shape was chosen by measurement in spec 021, and a different table is
a different spec with its own measurement to earn its place.

> **The candidate has been measured, and it was not the reason.** [ADR-0036](0036-the-probes-remainder-taken.md)
> took the remainder apart: the same hand-written lookup **with** the map's consult in it costs
> **0.020 ms** more than the one without it, and each set's own three campaigns are judged against **its
> own** candidate spread — **−0.221 ms … 0.406 ms** against **0.709 ms** in the old set, **−0.278 ms …
> 0.071 ms** against **0.168 ms** in the new one — so the share is **at or below the noise** in both. The
> remainder is the lookup's **shape**, not the map — and the shape is **named, not measured**: the method
> over `&self`, the byte's `Option` chain, the `u32` width, and the map's **`Option` discriminant**, which
> is on the fast path of the code that ships; the **saturating add** and the map's consult are **off** it.
> The map's consult is **gone** — the table's map is an `Option` now, so when nothing overflows it is not
> allocated at all — and the change bought **nothing measurable**: the probe's own measurement is
> **0.950 ms** faster, but the floor, which the change cannot touch, moved **1.514 ms** slower in the same
> two sessions, and the **resident** join stage moved **23.6 ms** against **22.2 ms** in the comparison
> campaigns and **23.2 ms** against **21.7 ms** in the same-moment A/B, while the **cold** stage and the
> **scan's** stage — which moved **0.3 ms** the other way — did not move with it. **That movement is not
> this ADR's business, and it is not explained away here.** It is written down as
> [spec 029](../../specs/029-the-lookup-shape-and-the-stage-that-moved/spec.md).

**The two paths differ by the probe, and the remainder is named.** The stages differ by **1.519 ms**,
and the build side's read is the same call on both paths, so the difference is the probe. The probe,
measured on its own, differs by **0.170 ms**, and **the two measurements do not agree**: the parts are
measured one at a time, each against a file and a table the measurement before it has pushed out of the
caches, where the stage runs its parts in one pipeline. The parts are therefore **bounds**, not a
decomposition that adds up, and the stage — the engine's own account — is the authority. The **1.35 ms**
between the two is the **remainder**, and it is named as one.

**The read is counting, not waiting for memory.** The probe key column's plain read is **0.509 ms** over
48 MB and **0.039 ms** over 512K values: the same per byte.

**No published timing moved.** The two campaigns — the new code and the code it replaced — were run back
to back on the same host, and the stage is **20.735 ms** cold against **20.582 ms**, the resident
**19.216 ms** against **19.283 ms**, and the probe **10.051 ms** against **10.270 ms**: every difference
is inside the noise. The **comparison campaigns were not re-run**, and the reason is recorded in the
spec: the fixes do not change the work the query does, and the probe's own before-and-after campaigns
are the measurement that says so. The **one** published figure that moved is the plan output's cold-path
join row count — `216783` back to `6000000` — and it is published old-and-new, with the reason.

**What this did not do.** The scan's decode is untouched. The build side's reader is untouched. The
table's shape and the probe's hash are untouched. The eager path's load is untouched. The plan is
untouched.

## References

| principle | how this ADR serves it |
|---|---|
| I. One Predicate, Six Implementations, One Answer | The two key sources are compared by a **differential test**, value for value, over NULLs, strings, dictionaries, the plain-integer fast path and a row past the end — the NULL rule and the range rule are one answer on both paths. |
| III. NULL Is an Absent Bit, and It Never Matches | The NULL rule is stated once, in the interface both key sources implement, and the column's length is part of that interface, so "the row is in range" has one answer. |
| IV. Evidence Over Claims | Every figure here is the median of three campaigns of five iterations, committed as raw reports and re-derived by `python3 scripts/medians.py`; the guard checks every published figure against the reports, in the section that publishes it. |
| V. The Comparison Is Reproducible by a Stranger | The instrument is committed in the tree, and the campaigns are three runs of one command. |
| VII. Explicit Scope, No Silent Deferral | The overflow map's **3.095 ms** is published as a finding and not taken; the table's shape, the probe's hash, the reader and the plan are named as out of scope, with the reason. |
