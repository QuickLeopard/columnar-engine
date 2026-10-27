# ADR-0029 — The dictionary is the crate's, and the engine's share is one implementation

**Status**: accepted
**Date**: 2026-10-27
**Spec**: [022-the-dictionary-counted](../../specs/022-the-dictionary-counted/spec.md)
**Supersedes**: nothing. **Amends**: nothing.

## Context

Spec 016 removed the partial batch, which was the whole of the engine's per-row-group cost at the
time, and it left a residual on the real Q6's **three**-predicate shape: about **0.14 ms a row
group**, against a **two**-predicate shape that is flat.

The two shapes differ by exactly one thing: the third predicate's column, `l_quantity`, which the
Parquet writer fell back to dictionary-encoding — **4,901 values a row group** — although the column
was written plain. So two things could be paying that residual, and they have opposite answers:

| | the cause | what follows |
|---|---|---|
| (a) | the **crate** rebuilding that dictionary, per row group | the crate's cost, and not fixable here |
| (b) | the **engine** comparing the dictionary's distinct values, per row group | the engine's cost, and fixable |

Spec 016 wrote the question down rather than guessing at it. This ADR is the answer, and the
instrument that produced it.

## The instrument

`crates/columnar-engine/examples/dictionary_cost_probe.rs` measures four shapes on the same file, in
the same process, with the engine's own batch size — and it checks that the two paths answer the
same question before it times either of them, the rule ADR-0028 recorded for the operator probe:

| shape | what it is |
|---|---|
| `crate_expanded` | the crate's read of the three predicate columns, dictionaries expanded |
| `crate_kept` | the crate's read of the same columns, dictionaries **kept** |
| `engine_expanded` | the crate's read, and the engine's comparison over the values |
| `engine_kept` | the crate's read, and the engine's comparison over the **distinct values** |

and the engine's own read, which is the shape the published figure comes from.

The batch size is the one the engine derives for the file — **125,000** rows — and the two paths
keep the same **110,915** rows, which the probe checks before it times either of them.

## Decision

### 1. The alternative is closed: the dictionary cannot be kept

The engine would stop comparing every row if the crate handed it the dictionary and the indices.
Asked for that — the same three columns, read with the dictionaries kept — the crate is not a little
slower. It is **18.3× slower**, and for the two numeric columns it is **34.9×** and **26.5×**
slower:

| column | expanded | kept | |
|---|---|---|---|
| `l_quantity` | 5.09 ms | **177.47 ms** | **34.9× slower** |
| `l_discount` | 3.75 ms | **99.35 ms** | **26.5× slower** |
| `l_shipdate` | 6.38 ms | 6.32 ms | the file's schema already says dictionary |
| all three | **15.46 ms** | **282.25 ms** | **18.3× slower** |

The reason is in the file: `l_quantity`'s Arrow schema says `Float64`, so a reader that is asked
for a dictionary has to **build** one from the values, and building a dictionary is more expensive
than the expansion it replaces. The one column whose file schema *is* a dictionary — `l_shipdate`,
which the generator wrote as one — is the one column the crate keeps cheaply.

**And the reader will not keep one either.** The crate hands back a **new** dictionary array for
every batch: **0 of 23** consecutive batches share the same array, on all three files and in both
versions of the code. The probe checks this on `l_shipdate`, by pointer identity rather than by
comparing the values. So "compare the distinct values once a row group" is impossible with this
reader — the engine cannot know the dictionary has not changed, because the reader gives it a
different one every time.

So the engine cannot have the dictionary, and the alternative is measured rather than argued about.

### 2. The residual is the crate's

Across the three files — six, twelve and twenty-four row groups, the medians of three campaigns
each — the crate's read of the three predicate columns rises by **0.07 ms a row group**, and the
engine's comparison on top of it adds **−0.01 ms a row group**: it does not grow with the file at
all. The engine's own read path adds nothing measurable either: the engine's read and the crate's
read with the comparison on top are the same number to within the noise.

So **the residual that is left is the crate's**, and the engine's share of it is flat rather than a
share to be divided. The engine publishes that as a fact about the reader rather than fixing it,
because it cannot fix it.

### 3. The engine's own dictionary path is one implementation, and it is not paying for an enum

What *is* the engine's is the comparison, and two things in it were duplicated or done the hard
way:

- the **map** from a dictionary's answer to the rows' mask was written **twice** — once in the
  pushdown, over the reader's arrays, and once in the filter, over the engine's own columns. The two
  copies were the same algorithm, and the filter's comment records that the first version of it
  cost **41 ms** on the Q6's predicate set.
- the **distinct values** were compared with a match on an `enum` **per distinct value**, because the
  threshold was a `ScalarValue`. The numeric paths had already resolved their bounds before the
  loop; the dictionary path had not.

Both are now one implementation, in `crates/columnar-engine/src/dictionary.rs`, shared by the two
callers: the map is one function, the string comparison is one function, and the bounds are
resolved **once**, before the loop over the values.

**And the change is a decomposition, not a speed-up.** The old code, commit `3d5b220`, was measured
back to back with the new one, on this host, on the same file, with the same probe. The engine's
comparison on the six-row-group file was **5.32 ms** before and is **5.94 ms** after; the three
campaigns behind them were 5.32, 5.48 and 5.27 before, and 5.33, 5.50 and 5.94 after. The two sets
overlap, and the change moved the number by less than the spread of the measurement. **There is no
measurable win, and this ADR does not claim one.** What the decomposition buys is one
implementation of the path, not a faster one.

## Consequences

- **The engine's dictionary path is one implementation, and it is not measurably faster.** The
  comparison was 5.32 ms before the decomposition and is 5.94 ms after, on the six-row-group file,
  and the difference is inside the spread of the measurement. What is published is the map, shared
  rather than duplicated.
- **The pushdown and the filter cannot drift apart.** They were the same algorithm in two places,
  and one of them had been fixed for a mispredicted branch the other had not.
- **The crate's dictionary reader is not changed.** It is a dependency. The residual that belongs to
  it is published as a fact about the reader.
- **The file's writer is not changed.** `l_quantity` is written plain and the Parquet writer falls
  back to dictionary-encoding it; that is a fact about the file, the same call ADR-0021 and ADR-0022
  made about the generator's writer.
- **The pushdown does not learn to compare a numeric dictionary.** The crate never hands the engine
  one, so a path that cannot be exercised is a path with no evidence — and the instrument is what
  shows it cannot be exercised.
- **A question that has been open since spec 016 is closed**, with the numbers committed: the
  per-row-group residual is the crate's, and the engine's alternative to it is closed by an **18.3×**
  measurement.

## The rule this adds

**An alternative is measured, not argued about.** The engine's own instinct was to ask the crate
for the dictionary and compare 4,901 values instead of six million. That instinct was wrong by a
factor of **18.3**, and no amount of reasoning about it would have said so: the crate's dictionary
reader had to be *run* on this file to be seen. The same rule as ADR-0028's — an instrument that
checks its own answers — with the other half added: an instrument that checks the *alternative* too.

## References

| principle | how this ADR serves it |
|---|---|
| I. One Predicate, Six Implementations, One Answer | The dictionary map and the string comparison were the same algorithm in two places; they are now one implementation, shared by the pushdown and the filter, and the differential suite holds the two to the same answer, row for row. |
| III. NULL Is an Absent Bit, and It Never Matches | A dictionary entry with no value is not a match, in either path, for every operator. |
| IV. Evidence over claims | Every number here is the median of three campaigns over a committed report, and the instrument that produced them is in the tree and re-runnable. |
| VII. Explicit Scope, No Silent Deferral | The exclusions above, each with its reason. |
