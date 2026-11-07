# Spec 049 — The batch size's price

**Status**: done
**Started**: 2026-11-07
**Finished**: 2026-11-07
**Predecessor**: [spec 048 — the probe, twice](../048-the-probe-twice/spec.md)

**CI green on `6d10bee`** -- run 42348260263, sixteen of sixteen jobs.

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

- [x] The instrument installs a **counting global allocator** and reports the **peak held bytes** of a read — the
      peak, because a peak is what a budget has to survive, and a budget that is checked against an estimate has
      never seen the real thing.
- [x] The peak is measured at **every batch size** of the sweep — **62,500**, **125,000** (the engine's own),
      **250,000** and **1,000,000** — and at **three widths**: the join query's **two** columns, `q6`'s **two**
      predicate columns with nothing materialised, and the **eager** path's **sixteen**.
- [x] Each point is measured **three times** and the median is published, so a point is not a single reading.

### R2 — Both shapes are priced, in **time** as well as in memory

- [x] The `filter-range` query — the one the limit was chosen for — is measured at the **same** batch sizes as the
      join query, **in the same session**, **back to back**, with the **order alternated**.
- [x] **So the join's 4.926 ms is priced against what the other shape pays**, and the difference is the **paired**
      one, not the difference of two medians from two campaigns.

### R3 — The estimate is **measured against**, and is not changed

- [x] The engine's own estimate is published **beside the measurement**, at every point, so the estimate's error
      is published **as the estimate's error** and not as a difference of two numbers.
- [x] **And the estimate is not changed in this increment.** Changing it is **spec 050's** work, and the reason
      is in the amendments.

### R4 — The default's **rule** is decided, and the default does not move

- [x] If the measurement says a **byte budget** is the right rule, the rule is published, with the numbers that
      said it — and the width the rule is for is named.
- [x] If the measurement says the **row count** is the right rule, **that is published too**, with the numbers
      that said it.
- [x] **And the default does not move in this increment.** Moving it moves **every** published figure the
      reader's batch size touches, and re-measuring those is **spec 050's** work. **A rule that is decided and
      a default that moves are two different acts, and this increment does the first.**

### R5 — The answer is published either way

- [x] A **new section** in `PERF.md`, the decision in an **ADR**, the status's next question rewritten, and the raw
      reports **committed**.
- [x] The **guard** checks the published figures **inside the section that publishes them**, as it does for
      spec 048's.

## What the measurement found

**The price scales with the width, and the rule is a byte budget.**

A **counting global allocator** measured the **peak** a read holds at three widths, and the **time** was measured
beside it in one session, back to back, with the order alternated.

- **The bytes a batch holds scale with the query's width**: between the **62,500**-row batch and the **1,000,000**-row
  one, the peak grows by **13.39 bytes a row** on the two predicate columns, by **16.03** on the two materialised
  ones, and by **173.88** on the sixteen — so a whole row group is **12.8 MiB** on a narrow query and **165.8 MiB**
  on a wide one. **And the knob is named in rows, so it cannot see any of that.**
- **The engine's memory estimate does not model the batch**: it says **1,500,016** bytes where the read holds
  **13,719,490**, and **1,549,500,016** where the read holds **1,157,291,375** — and the **default budget refuses
  the wide shape at every batch size**, including the one that would have fitted.
- **And the batch's optimum is width-dependent**: the join's part is **−3.717 ms** at a whole row group, faster in
  **24 of 24**; the filter-range's scan does not move; and the wide read is **+14.455 ms**, faster in **1 of 24**.

**So the rule is a byte budget, and the budget is 16 MiB** — bounded by the measurement from both sides: above the
narrow shapes' whole row group (**12.8 MiB** and **15.3 MiB**) and at or below the wide shape's own batch
(**20.7 MiB**). **And it gives every shape at least what it has today**: the narrow shapes get the whole row group,
and the wide shape gets **62,500** rows.

**And the default does not move in this increment.** Moving it moves **every** published figure the reader's batch
size touches, and that is **spec 050's** work. See
[ADR-0061](../../docs/adr/0061-the-batch-sizes-price.md) and `PERF.md` §3af.

## The scope

- **In**: the reader's batch size, the memory a read holds, the engine's memory estimate, and the default's rule.
- **Out**: **any change to the engine**. The **estimate** does not change and the **default** does not move — both
  are **spec 050's** work, and the amendments say why.
- **Out**: **the reader's own decode**. Spec 048's finding — that the reader's context is the join's other work —
  and ADR-0018's — that owning the decode was measured three times and refused — are two different questions, and
  **this one does not answer either.** Owning the decode is **spec 051**, and this increment does not touch it.
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

### A3 — The engine does not change in this increment

**Why**: two changes are named by the measurement, and **both of them move published figures**, and the house's rule
is that a moved figure is published old-and-new with the reason and **re-measured**, not recomputed.

- **The estimate.** It does not model the reader's batch, so the estimate is **9× low** on a narrow query at a large
  batch and **35% high** on a wide one — and the budget cannot see the knob's price. **Fixing it moves the estimate's
  own published figures**, in three places in `PERF.md` and in a sentence about a query the budget **refuses**: the
  24-million-row query that is rejected today would fit, and the sentence that says so would be wrong.
- **The default.** It becomes a byte budget, and **every** published figure the reader's batch size touches moves
  with it: the headline table, the thread sweeps, the ladder, the join's parts.

**So this increment measures the price and decides the rule, and spec 050 builds both.** That is the house's order —
the measurement decides what is built, and the building is its own increment — and it is also the safer one: a rule
that is decided on one day's campaigns and a default that moves on another day's are two different claims, and the
second one needs its own campaigns.

**And the estimate's gap is published here, beside the measurement.** A gap that is published is a finding; a gap
that is left in a blind spot is not.
