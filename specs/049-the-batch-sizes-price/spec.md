# Spec 049 — The batch size's price

**Status**: planned
**Started**: 2026-11-07
**Predecessor**: [spec 048 — the probe, twice](../048-the-probe-twice/spec.md)

## The problem

**Spec 048 made the reader's batch size a knob, and measured what it buys. It did not measure what it costs.**

The knob buys **4.926 ms**: a batch of a row group — **1,000,000** rows — is cheaper than the engine's own
**125,000**, faster in **24 of 24** paired samples, and the curve is monotone
([§3ae](../../PERF.md#3ae-the-probe-measured-twice)). **And the knob's price is unmeasured and unmodelled**, on
two counts.

**The first is that the bytes a batch holds scale with the query's width, and the knob is named in rows.** The
reader holds a batch's columns at once: the join query reads **two**, and the eager path reads **sixteen**. The same
**1,000,000**-row batch is a few megabytes on the first and several times that on the second — **and a knob named in
rows cannot see the width.** The engine's limit, `MAX_READER_BATCH_ROWS`, is a row count too: it was chosen for the
`filter-range` shape, where the reader reads two columns
([§3e](../../PERF.md#3e-the-scans-decomposition-and-what-the-compaction-was-worth)).

**The second is that the engine's own memory estimate does not model the batch at all.** `estimate_lazy_read_for`
charges the **file's** rows for the columns the plan materialises, and charges **nothing** for the columns the reader
fetches for the predicates — they are decoded per batch and dropped — and **nothing** for the batch's own transient
bytes. The `q6` query's estimate is **1.4 MiB**, while the reader is decoding a batch's worth of predicate columns on
top of that, and **at a million-row batch on a wide file it would be holding tens of megabytes the budget cannot
see.**

**And the optimum is shape-dependent.** §3e chose the limit for the `filter-range` shape — best on one shape and
within **1%** of the best on the other — and spec 048 found the **join's** probe is cheapest at a row group. **So the
default cannot be right for both, and the memory it holds is width-dependent.** A row count is the wrong shape for a
rule that has to serve both.

**So the question is not "is the batch size good". It is "what does it cost, and what is the rule".**

## The requirements

### R1 — The price is **measured**, not estimated

- [ ] The instrument installs a **counting global allocator** and reports the **peak held bytes** of a read — the
      peak, because a peak is what a budget has to survive, and a budget that is checked against an estimate has
      never seen the real thing.
- [ ] The peak is measured at **every batch size** of the sweep — **62,500**, **125,000** (the engine's own),
      **250,000** and **1,000,000** — and at **three widths**: the join query's **two** columns, `q6`'s **two**
      predicate columns with nothing materialised, and the **eager** path's **sixteen**.
- [ ] Each point is measured **three times** and the median is published, so a point is not a single reading.

### R2 — Both shapes are priced, in **time** as well as in memory

- [ ] The `filter-range` query — the one the limit was chosen for — is measured at the **same** batch sizes as the
      join query, **in the same session**, **back to back**, with the **order alternated**.
- [ ] **So the join's 4.926 ms is priced against what the other shape pays**, and the difference is the **paired**
      one, not the difference of two medians from two campaigns.

### R3 — The estimate **learns the batch**

- [ ] The engine's memory estimate models the **batch's** transient bytes: the columns the reader fetches for the
      predicates, and the batch's own rows, are charged at the **batch's** size and not at the file's.
- [ ] The estimate's figure is published **old-and-new**, with the reason — and the **budget** can then refuse a plan
      whose batch does not fit, which today it cannot see.
- [ ] **The public surface does not break.** The estimate's functions are `pub`, so a new parameter would be a major
      break; a new function is an addition, and the crate is still unpublished.

### R4 — The default's rule is **decided**, either way

- [ ] If the measurement says a **byte budget** is the right rule, the default becomes one: the largest divisor of
      the row group's row count that holds no more than **B** bytes — so a wide query gets a smaller batch and a
      narrow one gets a bigger, and the memory is bounded whatever the width.
- [ ] If the measurement says the **row count** is the right rule, **that is published too**, with the numbers that
      said it.
- [ ] **Every figure that moves is published old-and-new, with the reason** — and if the default moves, the figures
      that move are **re-measured**, not recomputed.

### R5 — The answer is published either way

- [ ] A **new section** in `PERF.md`, the decision in an **ADR**, the status's next question rewritten, and the raw
      reports **committed**.
- [ ] The **guard** checks the published figures **inside the section that publishes them**, as it does for
      spec 048's.

## The scope

- **In**: the reader's batch size, the memory a read holds, the engine's memory estimate, and the default's rule.
- **Out**: **the reader's own decode**. Spec 048's finding — that the reader's context is the join's other work —
  and ADR-0018's — that owning the decode was measured three times and refused — are two different questions, and
  **this one does not answer either.** Owning the decode is **spec 050**, and this increment does not touch it.
- **Out**: any change to the plan's shape, and any change to the public surface.
- **Out**: the crates.io publish. The crates stay unpublished while this work lands, so a break that the work implies
  costs nothing.

## The instrument

**`crates/columnar-engine/examples/batch_memory_probe.rs`** — the increment's own probe: a counting global
allocator, the read at each batch size and width, and the engine's own estimate beside the measurement.

**And `crates/columnar-bench/src/join_parts.rs`** — spec 046's probe, for the query's own parts at each batch size,
because the question is not only "what does the batch hold" but "what does the query pay for holding it".

## The references

- [PERF.md §3ae](../../PERF.md#3ae-the-probe-measured-twice) — the knob, the sweep, and the **4.926 ms** this
  increment prices.
- [PERF.md §3e](../../PERF.md#3e-the-scans-decomposition-and-what-the-compaction-was-worth) — the limit, and the
  shape it was chosen for.
- [PERF.md §3ac](../../PERF.md#3ac-the-join-orders-query-on-both-sides-taken-apart) — the join part, which moves if
  the default moves.
- [ADR-0060](../../docs/adr/0060-the-probe-measured-twice.md) — the knob's decision.
- [ADR-0023](../../docs/adr/0023-the-per-row-group-cost.md) — the partial batch, and the divisor rule.
- [ADR-0018](../../docs/adr/0018-the-scan-reads-the-parquet-decode.md) — owning the decode, measured three times and
  refused; **spec 050's** question, not this one's.
- [constitution.md](../../constitution.md), principle **IV — Evidence Over Claims**: a number is a claim about a
  measurement, and a claim without its evidence is not published.
- [constitution.md](../../constitution.md), principle **VII — Explicit Scope, No Silent Deferral**: the decode is
  **named** as the next increment rather than left as an omission.

## The amendments

### A1 — The price is measured with a counting allocator, and not with the engine's own estimate

**Why**: the engine already has a memory estimate, and it is a **pre-flight** bound — a plan is refused when its
estimate does not fit, and it says so itself: *"It is meant to fail early and loudly, not to be accurate to the
byte."* **An estimate cannot be the measurement of its own error.** The increment's own question is what the estimate
gets wrong, so the measurement has to come from outside the estimate — a counting allocator sees every allocation the
process makes, and it sees the **peak**, which is the number the budget has to survive.

**And the allocator is in the example, not in the engine.** Every crate here carries `#![deny(unsafe_code)]`, and a
`GlobalAlloc` implementation needs `unsafe`. **An example is its own crate**, so the counting allocator is the
diagnostic's and never the engine's — and the engine's own memory behaviour is untouched by it.

### A2 — The two measurements are **separate processes' worth of work**, not one

**Why**: a counting allocator counts **every** allocation, and that is not free. **So the memory is measured in the
allocator's instrument, and the time is measured in spec 046's probe, which has no allocator in it.** The two are
never in the same measurement, because an allocator that counts every allocation would make the time it sits next to
untrustworthy — and a time measured next to an allocator is not the engine's time.
