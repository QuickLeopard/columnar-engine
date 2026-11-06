# ADR-0056: The rows are handed out as they lie

**Status**: accepted
**Date**: 2026-11-06
**Reviewers**: TBD

## Context

Spec 043 gave a store's table its own door — `FileSetSource::execute_rows` — and measured what came out of
it. The measurement said three things, and one of them was the next increment's:

- **The set costs nothing that can be claimed.** The rows out of one file and out of six files are the same
  rows, and the difference between the arms is inside the noise.
- **The identity is exact.** The same 6,000,000 rows, and the two files the two round trips write are the
  same file, byte for byte.
- **The gather is 2,292.710 ms**, and it is most of what the eager path spends handing the rows out. On a
  plan that filters nothing **the copy buys nothing**, because the rows are the rows that were read.

That third thing was a **copy of the whole table**: six million rows, sixteen columns, and every dictionary
value materialised into its own string. The rows a plan hands out were **gathered** — the plan's projection,
copied out of what was read — and the copy was made whether the plan wanted it or not.

The copy cost more than the read. The eager path's read was 516.196 ms and its gather was 2,823.164 ms;
the lazy path's rows were 3,338.468 ms, which is its scan and the same copy. **And the copy cost the shape
as well**: the gather materialised the dictionary away, so the rows came out as plain columns where the
file's own schema says a dictionary — and the Parquet writer stores the Arrow schema in the footer, so the
round trip's metadata said the file's dictionary columns were plain strings. **That is why spec 043's round
trip was the published file's data and not its metadata.**

The engine had everything it needed to stop: the reader's chunks are the engine's own vocabulary, and the
store's conversion already takes a dictionary column and a plain one.

## Decision

1. **The rows are the rows that were read, when they are.** The rows path makes one test, and it is about
   the rows and not about where they came from:
   - **No row was removed.** The filter's selection holds the survivors in the order they were read, and
     the selection's rows are distinct, so a selection as long as the dataset *is* every row.
   - **Nothing reordered them.** A join's output and a sort's output are new rows, in a new order.

   When both hold, the plan's projection is handed out **as it was read**: the engine's own chunks, with
   their own encodings, in their own order.

2. **The lazy path moves them and the eager path copies them.** The lazy path read the columns, so the
   engine owns them and gives them away — **no copy at all**. The eager path's caller owns the dataset, so
   the rows are a copy of it — **but a copy of the columns, not a gather of them**, so the encodings
   survive it.

3. **The gather stays for every plan that needs it.** A filter that removed rows in the engine, a join and
   a sort all make rows that are not the rows that were read, and the survivors are scattered through the
   batch and have to be pulled out. **The gather is the right thing to do there, and it is still done.**

4. **The shape depends on the plan, and it is stated.** The rows' encoding says which shape the caller
   got: `Utf8Dict` is a dictionary column and `Utf8` is a plain one. The engine's documentation, the
   store's README and `docs/embedding.md` say so.

5. **The store does not change at all.** Its conversion already mapped the three dictionary forms and the
   three plain ones (ADR-0054), and its writer, its grid, its commit and its schema rule are untouched.

6. **The surface does not move.** The rows are the same type they were — a `Dataset` of column chunks — so
   **no new type appears and no break is taken**, and the crates can be published with this increment in
   them.

7. **The measurement says both halves.** The cost is published before and after, the shape is published as
   the counts of the columns that carry a dictionary and the columns that do not, and the round trip is
   published as the bytes and the first differing byte.

## Consequences

**What becomes easier.** **The round trip is the file, byte for byte.** The rows keep the dictionary, the
Arrow schema in the footer says what the file says, and the file that comes out of the store is the file
the rows came from: 242,859,461 bytes and no differing byte. Spec 043's one difference — the metadata — is
gone, and the difference that is left is the store's own nullability rule, which is the store's rule.

**What becomes faster.** The rows out are **six times faster**: 549.167 ms against 3,338.468 ms. The copy
that is left in the eager path is **eighteen times smaller**: 154.858 ms against 2,823.164 ms. **The lazy
path pays no copy at all.**

**What becomes harder.** The rows' shape now depends on the plan, and a caller that assumed one shape gets
the other. The store takes both, so the embedder does not have to care; a caller that reads the encoding
directly has to.

**What we gave up.** Nothing that was promised. The gather is not removed — it is where it belongs — and
the estimate does not move: what the budget is charged is what it was, and what the plan actually uses went
down.

## Measurement

**The cost, the shape and the bytes** are in [PERF.md §3ab](../../PERF.md) and
[PERF.md §3aa](../../PERF.md).

- **The gather is gone.** The one file's rows are **549.167 ms** against **3,338.468 ms**, and the eager
  path's copy is **154.858 ms** against **2,823.164 ms**. The lazy path is now the cheaper of the two, by
  157.467 ms, and that difference is the copy.
- **The shape is the plan's.** On the published file's schema the rows come out with **7 dictionary
  columns** and **9 plain ones**.
- **The round trip is the file.** Under the file's own schema the round trip writes **242,859,461** bytes
  and **no differing byte**; the Arrow schema is the file's Arrow schema and the Parquet schema is the
  file's Parquet schema. **The store's own rule still differs**, at byte **20,566,725**, because a column
  that holds no NULL is written REQUIRED.
- **The estimate does not move**: **1,549,500,016 bytes** for one file and for the set.

## The correction

**§3aa published the gather wrong, and the figure is corrected in the place that published it.**
[PERF.md §3aa](../../PERF.md) published the gather as **2,292.710 ms** — the difference between the eager
rows and the eager read — and published it as **81%** of what the eager path spends handing the rows out.

**But the eager rows measurement did not include the read.** The probe measures the decode and the rows
**separately**: `eager_read` is `Dataset::read_parquet` and `eager_rows` is the `execute_rows` call that
follows it. So the difference between them was never the gather — **the gather was the whole of the eager
rows, 2,823.164 ms**, and the eager path's total was **3,353.618 ms**.

**The claim the wrong figure supported was right** — the gather was most of what the eager path spent, and
on a plan that filters nothing the copy bought nothing. **The figures were wrong**: the gather was
**2,823.164 ms** and **84.2%** of the eager path's cost, and not 2,292.710 ms and 81%.

**The correction is in §3aa, where the wrong figure was published**, and the figure the correction quotes
is checked by the guard against the **before** reports — `bench-results/set-rows-spec044-before-run1.json`
and its two siblings — so the correction quotes the reports and not a memory of them.

## Confidence

**high** — the round trip is byte-for-byte exact against the published file, the shape is measured and
stated, the cost is measured before and after in the same campaign with the arms' order alternating, and the
one correction is named with the figure it replaces. The one thing that could change the decision is a
caller that needs **one** shape and gets the other: the store takes both, and the encoding says which one
the caller got.
