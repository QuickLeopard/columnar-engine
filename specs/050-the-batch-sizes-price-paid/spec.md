# Spec 050 — The batch size's price, paid

**Status**: done
**Started**: 2026-11-07
**Finished**: 2026-11-07
**Predecessor**: [spec 049 — the batch size's price](../049-the-batch-sizes-price/spec.md)

**CI green on `c27262c`** -- run 42358361131, sixteen of sixteen jobs.

## The problem

**Spec 049 priced the reader's batch size and decided the rule's shape: a byte budget.** A **counting global
allocator** measured the **peak** a read holds at three widths, and the peak grows by **13.39 bytes a row** on two
predicate columns, by **16.03** on two materialised ones and by **173.88** on the file's sixteen
([§3af](../../PERF.md#3af-the-batch-sizes-price)). **And the rule is not built, because the rule needs to know what a
row costs before the read, and the engine has no single honest statement of that.**

**What the engine charges today is not what a row costs.** The estimate charges a `utf8` column **24 bytes** a row —
a `String` header — **and a dictionary-encoded column's real cost is its index's four**, which the estimate's own
document says: *"the dictionary is shared by the whole column, and the per-row cost is the index."* **Both paths do
it**: the eager path's estimate *has* the chunk's encoding in hand and charges the header anyway, and the lazy
path's estimate cannot see the encoding at all.

**And the file's own footer is not the statement either.** A column chunk's `total_uncompressed_size` over its
`num_values` is the **file's** bytes a row, and it is **the file's pages, not the working set**: on the published
file the sixteen columns come to **70.7 bytes** a row that way, against the **173.88** the allocator measured. The
file stores `l_discount` in **0.50** bytes a row and the engine holds it in **eight**. **So the footer cannot be the
statement, and the schema cannot be the statement, and the estimate is not the statement.**

**And the default's move moves every published figure the reader's batch size touches.** The headline table, the
thread sweeps, the ladder, the join's parts. **A rule that is decided and a default that moves are two different
acts**, and this increment does the first.

## The requirements

### R1 — What a row costs is **measured against**, not asserted

- [x] The width statement is **checked against the allocator's measurement** at all three widths, and the check is
      published: the statement, the measured bytes a row, and the difference.
- [x] **And the file's own footer is published beside it**, so that the reader can see that the footer is **2.46×
      low** on the wide shape and why the footer's number is the file's and not the working set's.

### R2 — **One** honest statement of what a row costs

- [x] A single statement gives the bytes a row of a column costs: **8** for an `int64` or a `float64`, **4** for a
      dictionary-encoded string's **index**, and **24** for a plain string's **header**, plus the bytes the string's
      own heap holds.
- [x] **The estimate and the batch's rule share it.** The estimate's per-column charge and the rule's per-row cost
      are the same number, read from the same place.
- [x] **The estimate's document and the estimate's code agree**, and the dictionary column's cost is its index.

### R3 — The estimate **charges the batch**, so the budget can see the knob's price

- [x] The estimate charges the **batch's** transient bytes — the columns the reader fetches, at the **batch's** size
      — beside the **file's** rows for the columns the plan materialises.
- [x] **And the budget can then refuse a plan whose batch does not fit**, which today it cannot see.
- [x] **The public surface does not break**: the estimate's functions are `pub`, so a new parameter would be a major
      break; a new function is an addition, and the crate is still unpublished. **The semver baseline does not move**,
      because the trait's new method has a **default body** and `cargo semver-checks` says **202 pass** and **58 skip**
      against the baseline. **The correction is recorded in ADR-0062.**

### R4 — The estimate's figures are published **old-and-new**

- [x] Every published figure the estimate touches is published **old-and-new**, with the reason.
- [x] **And the sentence about the query the budget refuses is corrected where it was published**, if the corrected
      estimate no longer refuses it.

### R5 — The answer is published either way

- [x] A **new section** in `PERF.md`, the decision in an **ADR**, the status's next question rewritten, and the raw
      reports **committed**.
- [x] The **guard** checks the published figures **inside the section that publishes them**.

## The scope

- **In**: what a row costs, the memory estimate, and the reader's batch size's rule.
- **Out**: **the default does not move in this increment.** Moving it moves **every** published figure the reader's
  batch size touches, and re-measuring those is **spec 051's** work.
- **Out**: **the reader's own decode** — that is **spec 052**, and this increment does not touch it.
- **Out**: the crates.io publish. The crates stay unpublished while this work lands, so a break that the work implies
  costs nothing.

## The instrument

**`crates/columnar-engine/examples/batch_width_probe.rs`** — the increment's own probe: the **width statement** beside
the allocator's **measured** bytes a row, at the three widths of spec 049, and the file's own footer beside both.

**And `crates/columnar-engine/examples/batch_memory_probe.rs`** — spec 049's probe, for the estimate's figure beside
the measurement, with the estimate changed.

## The references

- [PERF.md §3af](../../PERF.md#3af-the-batch-sizes-price) — the price, the rule, and the budget's number.
- [PERF.md §3ae](../../PERF.md#3ae-the-probe-measured-twice) — the knob, and the win the batch buys.
- [ADR-0061](../../docs/adr/0061-the-batch-sizes-price.md) — the decision this increment builds.
- [ADR-0018](../../docs/adr/0018-the-scan-reads-the-parquet-decode.md) — owning the decode, measured three times and
  refused; **spec 052's** question, not this one's.
- [constitution.md](../../constitution.md), principle **IV — Evidence Over Claims**: a number is a claim about a
  measurement, and a claim without its evidence is not published.
- [constitution.md](../../constitution.md), principle **VII — Explicit Scope, No Silent Deferral**: the default's move
  and the decode are **named** as the next increments rather than left as omissions.

## The amendments

### A1 — The statement is **measured against**, and the footer is published beside it

**Why**: there are three candidate answers to "what does a row cost" — the schema's types, the file's footer, and the
allocator's measurement — and **the measurement is the only one that is about the working set**. The schema's types
cannot see a dictionary column; the footer is the **file's** pages and is **2.46× low** on the wide shape.

**So the statement is written down, checked against the measurement, and published with the measurement beside it.**
A statement that is not measured against is a guess with a formula.

### A2 — The default does not move here, and the reason is the same as spec 049's

**Why**: the default's move re-measures the headline table, the thread sweeps, the ladder and the join's parts, and
publishes each one **old-and-new**. **That is one increment's work**, and doing it in the same increment as the
statement would put two campaigns' worth of moving figures into one place — and the house's rule is that a moved
figure is re-measured, not recomputed. **So the rule is built and measured here, and the default moves in spec 051.**
