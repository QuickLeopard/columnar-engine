# ADR-0055: The set's rows are handed out

**Status**: accepted
**Date**: 2026-11-06
**Reviewers**: TBD

## Context

Spec 040 gave the engine a source over a **set** of files, and spec 042 gave the join's **build side** a
set and gave the store a way to take the **engine's rows**. What was left over was the link between them:
**the set's own rows could not be handed out.**

The engine had three doors for a plan — `execute` over a `Dataset`, `execute_on` over a `ParquetSource`,
and `FileSetSource::execute` over a set — and the third one **folded the rows into an aggregate**. The two
that hand rows out (`execute_rows`, `execute_rows_on`) took a dataset and a file. **So a store's table, which
is a set of files, could not be read by the engine and written back**: an embedder could copy the files one
by one, and then the engine never saw the rows and the file that came out was never the engine's.

The gap was **one method wide**. The source read a set, the pipeline handed a plan's rows out, the store
took the engine's rows, and `Table::paths` gave the list of paths — everything was built except the door
between the set and the rows.

And there were two questions nobody had asked, because the door was closed:

1. **Are the set's rows the one file's rows?** The set's rows are read from **six files** and the one
   file's rows from **one**, and the two have to be the same rows in the same order.
2. **Does the round trip reproduce the file?** The store's claim is *"fed the same rows, it writes the same
   file, byte for byte"*, and it had only ever been measured through Arrow and through the engine's
   **eager** rows. A round trip through a **plan** had never been measured.

## Decision

1. **The set's rows are handed out.** `FileSetSource::execute_rows(&self, plan, options)` is the set's own
   counterpart of `execute_rows_on`, and **both go through one function** —
   `facade::execute_rows_with`, which is `facade::execute_source` with `Output::Rows`. The set's rows are
   therefore the file's rows by construction: the same plan resolution, the same scan decision, the same
   stages, the same rules.

2. **The refusal is one rule.** A plan with an aggregation is refused on the set's path with the **same
   error and the same wording** as the eager and one-file paths: the aggregate folds the rows into groups
   and there is nothing left to write. Three doors, one message.

3. **The surface does not move.** `execute_rows` takes a `&QueryPlan` and an `&ExecOptions` and gives back a
   `Dataset` — all three already on the surface — so **no new type appears and no break is taken**. The one
   break the project ever took (`JoinSpec.build_path` → `build_paths`, ADR-0053) was taken while the crates
   were unpublished; this increment takes **none**, so it can be made after the publish as well as before it.

4. **The store does not change at all.** `append_rows` takes the engine's rows, `Table::paths` gives the
   list of paths, and the round trip is the embedder's three lines. The store's library still does not
   depend on the engine, and the engine's read path still cannot move because of anything in the store.

5. **The round trip is measured, and the difference is named.** The measurement says three things, and all
   three are published:
   - **The set's rows and the one file's rows write the same file, byte for byte.** The identity is exact.
   - **The round trip reproduces the published file's data exactly.** The probe compares the two files'
     **data regions** — everything in front of the metadata — and they are **byte for byte the same**, and
     the first differing byte is the published file's **metadata's first byte**.
   - **The metadata differs because the gather materialised the dictionary away.** The Parquet writer
     stores the Arrow schema in the footer, and the Arrow schema says the dictionary columns are
     dictionaries. The rows the engine hands out are **plain columns** where the file's schema says a
     dictionary. **The Parquet schema is the same and the Arrow schema is not**, and both are measured.

6. **The copy the rows path makes is named, and it is the next increment.** The rows the engine hands out
   are **gathered** — the plan's projection, copied out of what was read — and the gather is
   **2,292.710 ms**: 81% of what the eager path spends handing the rows out. On a plan that filters nothing
   **the copy buys nothing**, because
   the rows are the rows that were read. **This increment measures it and names it; it does not fix it**,
   because the fix changes what a plan's rows are — the dictionary survives — and that moves §3z's published
   figures and is a decision of its own.

## Consequences

**What becomes easier.** A store's table can be read by the engine and written back: the paths go to the
set, the set's rows come out, and the store writes them. The composition the project was built for is
closed end to end, and the glue is three lines.

**What becomes harder.** Nothing. The set's rows are the file's rows, the set costs nothing the one file
does not cost, and the estimate does not move.

**What we gave up.** **The round trip is not byte-exact**, and the reason is in the rows themselves: the
engine's vocabulary carries **which rows are NULL** and not **whether a column may be** (ADR-0054's rule)
and not **whether a column is dictionary-encoded** (this ADR's finding). Both differences are the schema,
and both are published rather than hidden. **A caller who needs the file exactly** writes it through the
Arrow path, where the schema comes with the rows.

## Measurement

**The cost, the identity and the three files** are in [PERF.md §3aa](../../PERF.md).

- **The set costs nothing that can be claimed**: the rows out are **+6.425 ms** against the one file's and
  the write **+34.659 ms**, and the difference between the arms in every iteration — measured back to back,
  with the order alternating — straddles zero with a spread wider than either, on a host whose load average
  was 18.03, 12.81 and 15.72. The estimate is **1,549,500,016 bytes** for both.
- **The identity is exact**: the same **6,000,000** rows, and **the two files the two round trips write are
  the same file, byte for byte**.
- **The round trip is the published file's data, and not its metadata**: the store's rule makes the first
  difference **20,566,725** — the same byte §3z published — and the data differs, because the definition
  levels are in the data pages. The file's own nullability moves the first difference to **242,857,739**,
  which is **inside the metadata** (the published file's metadata starts at **242,845,766**), and **the two
  files' data regions are byte for byte the same**.
- **The gather is 2,292.710 ms** — 81% of what the eager path spends handing the rows out — and it is a
  copy of the whole table.

## The correction

**The probe's first run reported the wrong first differing byte, and the figure was wrong while the claim
was right.** The probe's file comparison returned the **chunk boundary** when the two files had different
lengths — and they do, because the round trip's file is the shorter one — and the boundary it returned was
the offset of the published file's **last chunk**, **242,810,880**. **The comparison now finds the first
byte that differs**, and the probe **compares the two files' data regions** — everything in front of the
metadata — which is the question a first differing byte cannot answer on its own. **The corrected figure is
242,857,739**, inside the metadata, and the data regions are identical.

**The claim the wrong figure supported was right**: the difference is the metadata and not the data. **The
figure was wrong**, and it is corrected here, in [PERF.md §3aa](../../PERF.md) and in
[STATUS.md](../../STATUS.md) — **in the places that published it** — and the probe that produced it is
fixed.

## Confidence

**high** — the identity is exact and byte-for-byte, the set's cost is inside the noise on a busy host, and
the two differences are named with their first differing byte. The one thing that could change the decision
is a caller who needs the file byte for byte **through the engine's plan rows**: that caller writes through
the Arrow path, and the reason the bytes differ is published.
