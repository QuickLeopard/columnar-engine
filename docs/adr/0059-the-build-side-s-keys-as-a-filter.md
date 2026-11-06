# ADR-0059: The build side's keys as a filter

**Status**: accepted
**Date**: 2026-11-06
**Reviewers**: TBD

## Context

[ADR-0058](../../docs/adr/0058-the-join-orders-query-on-both-sides-taken-apart) took the `join-orders`
query apart on both sides and named the mechanism: `DuckDB` joins **216,783** rows and the engine joins
**6,000,000**, because `DuckDB` pushes the build side's keys into the probe side's scan as a filter —
its own **perfect-hash filter** over the build side's key set — and the engine has no such filter.

**And it named the prize.** `DuckDB`'s filter costs its scan **5.827 ms** over six million rows and saves
its join **51.422 ms**. **The engine's prize, if its filter costs what `DuckDB`'s costs, is the whole of
its own join — 14.625 ms — less what the filter would cost it.** [§3ac](../../PERF.md#3ac-the-join-orders-query-on-both-sides-taken-apart)
left the question open on purpose: *what does a filter over the build side's keys cost, and what does it
save*.

**And the shape was not settled.** Spec 021 built "a bitmap in front of the table" and measured it at
three sizes — 8 KiB, 32 KiB and 256 KiB — and it was never better than the table alone. **Those sizes are
saturated**: the house said so at the time, that with 216,345 keys a filter needs about six million bits
before it rejects anything at all. **The filter that rejects was never measured** — a bitmap the size of
the key's own range, one bit a key, **six million bits**, three quarters of a megabyte, an eighth of the
table's size, with **no hash**: the direct-addressed table consults nothing, so a filter over it has
nothing to hash.

**And there was a second shape, and it is the one the prize is about.** A filter in front of the table
skips the table's byte for the rows it refuses. A filter that **compacts** the rows — the filter's test
over every row, and the **survivors alone** probed — keeps the rejected rows out of the probe's own work
as well. **The prize is only collectable by the second shape**, and a ladder that measured only the shape
the question named would have refused that shape and left the question open.

## Decision

1. **The filter is measured, and the measurement decides what is built.** No engine work changes in this
   increment: not the lookup, not the memory estimate, not the plan.

2. **The candidates are the engine's own probe, line for line**, and the engine's own probe is measured
   **in the same iteration, over the same column**, as the control. **A rung that is not the engine's
   probe would price a shape the engine does not have.** The control is measured at **7.622 ms** against
   the candidate's **7.967 ms**.

3. **The probe side's column is materialised afresh in every iteration, and it is not copied.** The
   engine's own query materialises the probe column and then probes it, so the column it probes is one
   the reader has **just written**. A column held warm across the iterations would be a column the engine
   does not pay for — and **that is the whole of the filter's case**: the filter only wins if the probe
   column's stream evicts the table and leaves the filter.

4. **Both shapes are measured**, and the difference is the **paired** one: the two sides measured **back
   to back in the same iteration**, with the **order alternated**. **The six orders are chosen so that
   every pair of candidates is back to back in four of the six iterations, two in each direction** — a
   rotation of three would have put one pair back to back four times and another twice.

5. **The rows are checked before anything is timed.** The candidates' rows are compared with the engine's
   own probe's rows, **row for row, in order**, in every iteration. A filter may let a key through that
   is not in the build side — the table decides that one — but it may **never** refuse a key that is in
   it, and the comparison is what says so.

6. **The filter is not built.** The filter in front of the table is **+2.318 ms** — a loss, in ten of
   twelve paired samples — and the filter compacting the rows is **+0.547 ms**, a **wash**, with four of
   its twelve paired samples negative. **A wash is not a win**, and the engine keeps no filter: the
   lookup stays as it is, and the memory estimate does not move.

## Consequences

**What became knowable.** **The prize is real and it is unreachable: +6.981 ms.** The probe over the
survivors alone costs **0.906 ms** against the probe as it is at **7.967 ms**. **And the reason is that
the engine's lookup is already the cheapest membership test there is.**

**A filter's test costs more than the lookup it would replace.** The filter's own load over the same six
million rows is **5.222 ms**; the table's own load — one random access into the table's own bytes — is
**4.399 ms**. **Both are one random access, and the bitmap's test carries a shift and a mask on top of
the access the byte does not.** The direct-addressed table's byte **is** the membership test: the key's
own range addresses it and it consults nothing. **A filter can only be consulted in addition to it, and
what it is consulted in addition to is cheaper than it is.**

**And the same test is worth two orders of magnitude more in `DuckDB`.** `DuckDB`'s filter costs its scan
**5.827 ms** over six million rows — the same order as this filter's **5.222 ms** — and it saves its join
**51.422 ms**. **The difference is not the test: it is what the test replaces.** `DuckDB`'s filter replaces
a **hash join**, which costs **+44.190 ms** more than the engine's lookup over the same six million rows.
**`DuckDB`'s filter is the price of `DuckDB`'s join, and the engine does not have that join to pay for.**
**The engine's advantage over `DuckDB` was already taken; copying `DuckDB`'s filter would give it back.**

**The instrument stays, and the engine is untouched.** The ladder is the house's, and it is the evidence
for the refusal and for the next increment's question. The engine's diff in this increment is **empty**.

**The next question is named, and it is not about the filter.** The engine's join part is **14.625 ms**;
its probe over six million rows is **7.967 ms** and its table's construction is **0.672 ms**. **A filter
cannot make the lookup cheaper, because the lookup is the membership test.** So the next increment has to
find the join's **other** work — the part the probe does not account for — and ask what it is.

**And the open question in `STATUS.md` is rewritten**, because the old one ("what would answer it: the
parts, not the total") has been answered and the new one is narrower.
