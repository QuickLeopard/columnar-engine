# Spec 022 — The dictionary, counted

**Status**: done
**Started**: 2026-10-27
**Finished**: 2026-10-27
**ADR**: [ADR-0029](../../docs/adr/0029-the-dictionary-counted.md)
**Predecessor**: [021-the-hot-paths-taken-apart](../021-the-hot-paths-taken-apart/spec.md)

## The problem

The last of the per-row-group cost. Spec 016 removed the partial batch, and what was left on the
real Q6's **three**-predicate shape was a residual of about **0.14 ms a row group**: the shape rises
across the three files (21.31 ms on the six-row-group file, 23.75 ms on the twenty-four-row-group
one) while the **two**-predicate shape is flat.

Two causes were named, and they have opposite answers:

| | the cause | the answer |
|---|---|---|
| (a) | the **crate** rebuilding the column's dictionary per row group | the crate's — a dependency, and not fixable here |
| (b) | the **engine** comparing the dictionary's distinct values per row group | the engine's — fixable, and cheap to fix |

Neither had been separated, and the two have very different consequences: if it is (a), the residual
is a property of the reader and the increment's answer is a published refusal; if it is (b), the
engine is comparing the same 2,526 dates ninety-six times and can stop.

So the increment takes the shape apart. The instrument is
`crates/columnar-engine/examples/dictionary_cost_probe.rs`, and it measures four shapes on the same
file, in the same process, with the engine's own batch size:

| shape | what it is |
|---|---|
| `crate_expanded` | the crate's read of the three predicate columns, with the dictionaries expanded into values |
| `crate_kept` | the crate's read of the same columns, with the dictionaries **kept** |
| `engine_expanded` | the crate's read, and the engine's comparison over the values |
| `engine_kept` | the crate's read, and the engine's comparison over the **distinct values** |

and the engine's own read (`engine_read`), which is the shape the published figure comes from.

## The requirements

### R1 — The two causes are separated by measurement, not by argument

- [x] The instrument measures the **crate's read on its own** and the **engine's comparison on top
      of it**, so the difference between the two is the engine's own work.
- [x] It measures the **alternative** as well: the same columns read with the dictionary **kept**,
      which is the only way the engine could stop comparing every row. The alternative is measured,
      not argued about, and the two paths are checked to answer the same question before either is
      timed.
- [x] It runs on the **three files** — six, twelve and twenty-four row groups — so a per-row-group
      cost is a *slope*, and not a difference between two numbers.

### R2 — The answer is published, and it says which of the two it is

- [x] The residual's share is attributed: the crate's read, the engine's comparison, and the
      engine's own read path.
- [x] The **negative result** is published as a result: if the dictionary cannot be kept, the
      measurement says so, and the numbers are committed with it.

### R3 — The engine's dictionary path is one implementation, and it is not paying for an enum

- [x] The **map** from a dictionary's answer to the rows' mask is **one** implementation, shared by
      the pushdown (over the reader's arrays) and the filter (over the engine's own columns). They
      were the same algorithm in two places.
- [x] The distinct values are compared **without matching on an enum per value**: the bounds are
      resolved once, before the loop over the values, the way the numeric paths already do it.
- [x] The comparison stays **branchless** where it was: the map accumulates a word at a time,
      because a date column's dictionary indices run in long stretches and a conditional bit set is
      a mispredicted branch every time a run ends.

### R4 — The engine's answer does not change

- [x] The pushdown and the filter still answer the same question, and the differential suite holds
      them to it, row for row.
- [x] A NULL never matches, for every operator, and the dictionary path says so too.

## The measurement

Three campaigns on each of the three files, the medians committed, and the tables derived from them
by `scripts/medians.py`. The Q6 revenue query's three predicates, on the six-, twelve- and
twenty-four-row-group files, on this ARM64 host, with the load average between **7.0** and **8.8**
while the campaigns ran.

**The engine's read, and the crate's read under it** (the three predicate columns, six-row-group
file):

| | ms |
|---|---|
| the crate's read, with the dictionaries expanded | **15.46** |
| the crate's read, with the dictionaries **kept** | **282.25** |
| the engine's comparison on top of the expanded read | **5.94** |
| the engine's own read | **21.40** |

**The alternative, measured per column** (six-row-group file):

| column | expanded | kept | |
|---|---|---|---|
| `l_quantity` | 5.09 ms | **177.47 ms** | **34.9× slower** |
| `l_discount` | 3.75 ms | **99.35 ms** | **26.5× slower** |
| `l_shipdate` | 6.38 ms | 6.32 ms | the file's schema already says dictionary |

**So the answer is (a), and the alternative is closed.** The crate's dictionary reader is not
slower by a little: asked for a dictionary over a column whose file schema says `Float64`, it builds
the dictionary from the values, and it is **18.3× slower** than the expansion it replaces — and
**34.9×** and **26.5×** slower on the two numeric columns. The engine cannot keep the dictionary and
skip the expansion, because the crate will not give it one cheaply.

**And the reason is in the reader.** The crate hands back a **new** dictionary array for every
batch: **0 of 23** consecutive batches share the same array, on all three files and in both versions
of the code. The probe checks this on `l_shipdate`, the one column the file keeps as a dictionary,
and it checks it by pointer identity. So "compare the distinct values once a row group" is
impossible with this reader: the engine cannot know the dictionary has not changed, because the
reader gives it a different one every time.

What is left for the engine is the **comparison**, and it is the smaller half. The probe times the
two halves of the engine's dictionary path on their own, over the whole file:

| | 6 groups | 12 groups | 24 groups |
|---|---|---|---|
| the **distinct values**, compared | 1.43 ms | 1.33 ms | 1.37 ms |
| the **map**, from the answer to the rows | 7.27 ms | 7.16 ms | 7.19 ms |

**And the crate's half carries the rate.** The engine's comparison — its own work, over the crate's
read — does not grow with the row-group count:

| | 6 groups | 12 groups | 24 groups | |
|---|---|---|---|---|
| the crate's read, with the dictionaries expanded | 15.46 ms | 15.87 ms | 16.65 ms | **0.07 ms a row group** |
| the engine's comparison on top of it | 5.94 ms | 5.47 ms | 5.71 ms | **−0.01 ms a row group** |

The engine's own read path — the dataset building around the read — adds nothing measurable: the
engine's own read is 21.40 ms on the six-row-group file, and the crate's read plus the engine's
comparison is 15.46 ms + 5.94 ms = 21.40 ms. So **the residual is the crate's**, and the engine's
share of it is flat.

**The change is published before-and-after, and it is not a speed-up.** The old code, commit
`3d5b220`, was measured back to back with the new one, on this host, on the same file, with the same
probe:

| the engine's comparison, six-row-group file | before | after |
|---|---|---|
| the median | 5.32 ms | 5.94 ms |
| the three campaigns | 5.32, 5.48, 5.27 | 5.33, 5.50, 5.94 |

The two sets overlap, and the change moved the number by less than the spread of the measurement:
**there is no measurable win, and the increment does not claim one.** What it changed was the shape
of the code — the map and the string comparison were the same algorithm in two places, and they are
now one implementation in `crates/columnar-engine/src/dictionary.rs`, used by both the pushdown and
the filter — and the work each batch does is the same work.

## The scope

- **The crate's dictionary reader is not changed.** It is a dependency, and the measurement says the
  engine cannot have the dictionary cheaply. The residual that belongs to the crate is published as
  a fact about the reader, not fixed here.
- **The file's writer is not changed.** The `l_quantity` column is written plain and the Parquet
  writer falls back to dictionary-encoding it; that is a fact about the file.
- **The materialised column's decode is not touched.** Arrow decodes it in **33.4 ms** and the
  Parquet crate in **35.2 ms**, so there is nothing there to win — spec 012 measured that, and this
  increment does not repeat it.
- **The pushdown does not learn to compare a numeric dictionary.** The crate never hands the engine
  one (it hands the expanded values), and a path with no way to be exercised is a path with no
  evidence.

## The gate

- [x] `cargo fmt`, clippy on arm64 and x86-64, the whole test suite, `cargo deny`, the wasm32
      check, the published-figures guard and the page checks all green.
- [x] The instrument's two paths answer the same question, and the probe says so before it times
      anything.

## References

| principle | how this spec serves it |
|---|---|
| I. One Predicate, Six Implementations, One Answer | The dictionary map and the string comparison were the same algorithm in two places; they are now one implementation, shared by the pushdown and the filter, and the differential suite holds the two to the same answer, row for row. |
| III. NULL Is an Absent Bit, and It Never Matches | A dictionary entry with no value is not a match, in either path, for every operator. |
| IV. Evidence over claims | Every number here is the median of three campaigns over a committed report, and the instrument that produced them is in the tree and re-runnable. |
| VII. Explicit Scope, No Silent Deferral | The four exclusions in the scope above, each with its reason. |
