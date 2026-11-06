# Spec 044 — The rows as they lie

**Status**: done
**Started**: 2026-11-06
**Finished**: 2026-11-06
**Predecessor**: [spec 043 — the set's rows](../043-the-sets-rows/spec.md)

**CI green on `3b2ac50`** -- run 42224024673, fifteen of fifteen jobs.

## The problem

**The rows the engine hands out are a copy of the rows it read, and on a plan that filters nothing the copy
buys nothing.**

### What the gather is

The rows a plan hands out are **gathered**: the plan's projection, copied out of what was read, with the
dictionary materialised away. A Parquet string column is stored as a dictionary — the distinct strings once,
a small index per row — and the gather turns every index back into its own string. On the published file
that is six million rows, sixteen columns, and every dictionary value made into its own string.

**Spec 043 measured it: the gather was the whole of the eager rows — `2,823.164 ms` — and it was 84.2% of
what the eager path spends handing the rows out at all** (against an eager read of `516.196 ms`). **And it is
a copy the plan did not ask for**: a plan that filters nothing hands out exactly the rows that were read,
and the copy buys nothing.

**The figure spec 043 published for the gather — `2,292.710 ms`, and 81% — was wrong, and the reason is
in the amendment below.** The probe measures the decode and the rows separately, so the difference between
the two was never the gather. The claim was right; the figure was not.

### What the copy costs besides the time

**The memory.** The gathered rows sit beside the rows that were read, so the peak holds two copies of the
table. **The shape.** The gather materialises the dictionary away, so the rows come out as plain columns
where the file's own schema says a dictionary — and **the Parquet writer stores the Arrow schema in the
footer**, so the file the store writes from the rows records plain strings where the file says dictionaries.
That is why spec 043's round trip is the published file's **data** and not its **metadata**: the first
differing byte is inside the metadata, and the two files' data regions are byte for byte the same.

### The rule that decides it

**The rows are the rows that were read** when two things hold:

1. **No row was removed.** The filter's selection holds the surviving rows in the order they were read, so
   a selection as long as the dataset **is** every row — whether the plan has no filter at all, or a filter
   that kept everything.
2. **Nothing reordered them.** A join's output and a sort's output are not the rows that were read, in any
   order; they are new rows, and they are gathered as they are today.

**And then the rows do not need to be copied at all.** The engine read the columns; it can hand the same
columns out.

## The requirements

### R1 — The rows are the rows that were read, when they are

**When the selection is every row and nothing reordered them, the rows path hands out the columns as they
lie.** The engine's own chunks, with their own encodings, in their own order — **not a gathered copy of
them**.

**One rule, both paths.** The eager path and the lazy path make the same test, because the test is about
the rows and not about where they came from.

**And the rule is stated, not discovered.** A caller that receives the rows can see which shape they are in
— the encoding says so — and the engine's documentation says the shape depends on the plan.

### R2 — The rows keep their encoding

**The dictionary survives.** The rows come out as the engine's own vocabulary — a column chunk with an
encoding — and the encoding is the one the reader produced: `Utf8Dict` where the column is a dictionary,
`Utf8` where it is not.

**The store already handles both.** Its conversion maps the three dictionary forms and the three plain ones
([ADR-0054](../../docs/adr/0054-the-engines-rows-reach-a-stores-table.md)), and it is measured doing it
byte for byte.

### R3 — The round trip is byte-exact

**A table's rows, read by the engine and written by the store, are the file they came from** — byte for
byte, no differing byte. Spec 043 measured the round trip's **data** as exact and its **metadata** as
different, and named the reason: the gather materialised the dictionary away. **With the rows as they lie
the reason is gone**, and the file comes out the file.

**And the store's own rule still differs, and that is the store's rule.** A column that holds no NULL is
written REQUIRED ([ADR-0054](../../docs/adr/0054-the-engines-rows-reach-a-stores-table.md)), so the store's
own `append_rows` writes a file that is not the file — **by the store's rule and not by the engine's**. The
measurement says both, and the section says which is which.

### R4 — The cost is measured, and the published figures that move are published old-and-new

**The rows out, before and after**, in the same campaign, with the arms' order alternating so the host's
movement is in both sides of every pair.

**Two published sections move** — §3aa's round trip and §3z's plan's rows — and **both are published
old-and-new with the reason**, because a figure that moved without its old value is a figure nobody can
check.

**And the estimate is published beside it.** The budget is charged from the estimate, so what the estimate
says about the rows path is published with what the rows path actually used.

### R5 — The scope is named, so it is a decision and not an oversight

- **The gather does not go away.** It is the **right** thing to do when a filter removed rows or a join
  reordered them: the survivors are scattered through the batch and have to be pulled out. **The gather
  stays for every plan that needs it.**
- **The rows' shape depends on the plan**, and that is stated in the engine's documentation and in the
  store's README. It is a contract, not an accident.
- **The eager path may copy.** The caller owns the dataset there, so the rows are cloned rather than
  moved — **but they are not materialised**, which is where the time went.
- **The estimate is not changed.** What the budget is charged is what it was; **what the plan actually uses
  goes down**, and both are published.
- **No SQL. No transactions with rollback. No MVCC. No concurrent writers. No row-level updates. No
  compaction. One writer.** And **no public break**: the rows are the same type they were.

## The plan's shape

1. **The test** — the selection's length against the dataset's rows, and whether a join or a sort produced
   the rows — is made **once**, in the rows stage, and it decides between two paths.
2. **The rows as they lie** — the dataset's own chunks, projected. The lazy path **moves** them (the
   engine owns what it read); the eager path **clones** them (the caller does).
3. **The rows as they were** — the gather, exactly as it is today, for every plan that needs it.
4. **The store changes not at all.** Its conversion already takes both shapes ✓, and its writer, grid,
   commit and schema rule are untouched.

## The measurement

One probe re-run, and one more beside it: **the set's rows** (spec 043's probe) and **the engine's rows**
(spec 042's probe), because both publish figures this increment moves. The medians with the spreads, the
raw reports committed under their own names, and the tables derived from the reports by
`scripts/medians.py` and checked by the guard **inside the section that publishes them**.

**The four questions**, the same four the last increment asked:

| | |
|---|---|
| **the cost** | the rows out, before and after, and the gather's share of it |
| **the shape** | the rows' encoding — dictionary when nothing was filtered, plain when it was |
| **the round trip** | the written file against the published file: the bytes, and **the first differing byte** |
| **the estimate** | what the budget is charged, against what the rows path used |

## The amendment

**The increment is done, and four things moved under it.** They are recorded here because the spec is where
the reasoning was written down, and a reader has to be able to see what changed and why.

1. **The shape depends on the plan *and* on the path, and the spec said the plan alone.** R1 says the test
   is about the rows and not about where they came from, and that is what the code does — **the same test,
   made once, in the rows stage**. But the two paths can answer it differently, because **the read is
   different**: the lazy path pushes the plan's predicates into the reader, so a predicate the reader took has
   **already removed its rows**, and the rows that come out are the rows that were read. The eager path
   reads everything and filters in the engine, so the survivors are scattered through the dataset and they
   are gathered. **So the rule is one rule, and the answer depends on the read** — and the documentation
   says so, in the engine's README, in the store's README and in `docs/embedding.md`.

2. **§3z's plan's rows did not move, and the reason is published.** The Q6 plan's predicates remove rows
   in the engine, so **the gather is still the right thing to do there** — the survivors are scattered
   through the batch — and the bytes are the same **982,089**. The section is published old-and-new with
   that reason, and the timings moved inside the noise.

3. **The probe's "the file's own schema" arm changed, and it changed for the reason the increment is
   about.** The arm used to write the rows under a schema with the dictionary materialised away, because
   the rows were gathered. **The rows keep the dictionary now, so the arm writes them under the file's own
   schema** — and the probe publishes the shape as well: the rows' dictionary columns and the rows' plain
   columns, counted.

4. **§3aa published the gather wrong, and the correction is in the place that published it.** The published
   figure was **2,292.710 ms** — the difference between the eager rows and the eager read — and it was
   published as **81%** of what the eager path spends handing the rows out. **But the eager rows
   measurement did not include the read**: the probe measures the decode and the rows **separately**, so
   the gather was the whole of the eager rows, **2,823.164 ms**, and the eager path's total was
   **3,353.618 ms** — so the gather was **84.2%** of it, and not 81%. **The claim the wrong figure supported
   was right** — the gather was most of the cost, and on a plan that filters nothing the copy bought
   nothing — **and the figures were wrong**. They are corrected in [PERF.md §3aa](../../PERF.md) and in
   [STATUS.md](../../STATUS.md), **in the places that published them**, and
   [ADR-0056](../../docs/adr/0056-the-rows-are-handed-out-as-they-lie.md) records the correction. **The
   figure the correction quotes is checked by the guard against the before reports** — the reports the
   section replaces are kept under their own names — so the correction quotes the reports and not a memory
   of them.

## The references

- [constitution.md](../../constitution.md) IV — evidence over claims: the cost, the shape and the bytes are
  measured, and the figures that moved are published old-and-new.
- [constitution.md](../../constitution.md) VII — explicit scope, no silent deferral: the scope table names
  what the gather still does and what the estimate does not change.
- [constitution.md](../../constitution.md) — *Development Workflow & Quality Gates*: Specify → Plan → Tasks →
  Implement; this spec is the specification, and the code follows it.
- [ADR-0044](../../docs/adr/0044-the-stores-half.md) — the store's half: the grid, the commit, the identity
  claim, and the rule this increment does not touch.
- [ADR-0054](../../docs/adr/0054-the-engines-rows-reach-a-stores-table.md) — the engine's rows reach a
  store's table: the conversion that takes both shapes, and the schema rule that still differs.
- [ADR-0055](../../docs/adr/0055-the-sets-rows-are-handed-out.md) — the set's rows: the round trip, the
  gather's cost, and the reason the round trip was the file's data and not its metadata.
- [ADR-0056](../../docs/adr/0056-the-rows-are-handed-out-as-they-lie.md) — the rows as they lie: the rule,
  the move against the copy, and the correction of the gather's published figure.
- [spec 042](../042-the-set-on-both-sides/spec.md) — the set on both sides: the write path and the
  conversion.
- [spec 043](../043-the-sets-rows/spec.md) — the set's rows: the round trip, measured, and the correction.
