# Spec 005 — Parallel decode

**Status**: accepted
**Date**: 2026-10-25
**Extends**: spec 002 (the lazy scan), spec 003 (the pushdown runs the engine's kernel)
**Related**: ADR-0007 (the incumbents are pinned to one thread), ADR-0008 (the eager decode)

---

## 1. The problem, measured

Spec 003 left one number, and it is the last big one.

The lazy path's cold `q6` query takes **89.1 ms**. DuckDB's takes **32.0 ms** on the same file,
with the same thread count. The gap is **2.8×**.

That gap is not pruning — DuckDB prunes nothing on this file either, and reads the same 12.3 MB.
It is not the reader's speed — the engine's full-file decode is now *ahead* of DuckDB's. It is
this, measured on the identical query and the identical file:

| DuckDB, `q6` cold | one thread | fourteen threads |
|---|---|---|
| | **32.0 ms** | **10.0 ms** |

**DuckDB gets 3.3× from fourteen cores.** The engine has one thread, because the comparison pins
both incumbents to one thread so that the comparison is like-for-like. That pinning is this
repository's own decision, and it is the entire remaining gap.

The comparison is fair — both sides run the same file, in the same harness, with the same pinned
thread count — and it is *unfavourable to the engine* by construction. That is what a
like-for-like comparison is for: it holds the comparison constant so the thing being compared can
be seen. It is also a measurement of a one-thread engine against a one-thread DuckDB, which is
not the same question as "is the engine faster than DuckDB".

## 2. What this spec changes

The engine reads a file with **N threads**, and the benchmark measures what that is worth.

The unit of parallelism is the **row group**. A row group is decoded independently of every other
row group, and the TPC-H `lineitem` file has six of them, so the file can be read six ways at
once. The predicate is evaluated per batch, inside the reader, and a batch never spans a row
group, so each thread's pushdown is independent too.

The engine's own limit is the **merge**. Six threads produce six partial datasets, and the
result has to be one dataset with one chunk per column. For most columns that is a concatenation.
For a dictionary column it is not: two partial datasets may hold different dictionaries, and
merging them means remapping one into the other — which the engine already does, per batch, when
it merges a file's row groups into one dictionary.

### The order is not negotiable

**The merged dataset holds the file's rows in the file's order.** Not in completion order, not in
thread order — in row-group order, as the file has them.

That is a correctness requirement, not a preference:

- `SUM` is not associative. The engines sum in different orders and their answers differ in the
  last bits — which the comparison harness already tolerates at a relative tolerance of 1e-9.
  A parallel engine that summed in completion order would make its own answer depend on the
  scheduler, and the same query would return different bits on different runs. The engine's
  answers must be **reproducible**, so the order is fixed.
- The eager and lazy paths must agree, row for row. A parallel read that reordered rows would
  break the equivalence the differential test checks.

So the parallel path is required to produce a dataset **bit-identical** to the single-threaded
one, and a test holds it to that.

## 3. Requirements

| | requirement |
|---|---|
| **R1** | The engine MUST read a file with a configurable number of threads, and one thread MUST mean exactly what it means today. |
| **R2** | The merged dataset MUST hold the file's rows in the file's order, so that the parallel path's answers are bit-identical to the single-threaded path's. |
| **R3** | The parallel and single-threaded paths MUST agree, row for row, on every query — the same differential test that holds the eager and lazy paths together. |
| **R4** | The merge MUST handle a dictionary column whose partial dictionaries differ, by remapping one into the other — the same operation the engine already performs per batch. |
| **R5** | The engine MUST publish the thread count it used, in the plan output, in the benchmark report, and in every published figure. |
| **R6** | The comparison MUST remain like-for-like: the incumbents MUST run at the same thread count as the engine, at every point of the sweep. |
| **R7** | The headline comparison MUST stay the **pinned, one-thread** one. It is the like-for-like claim, and a headline that moved to fourteen threads would be a claim about a different question. |
| **R8** | The scaling sweep MUST publish the engine and both incumbents at the same set of thread counts, so the reader can see the shape of the curve and not only its endpoint. |
| **R9** | The peak working set MUST be estimated for the parallel path, and the memory budget MUST still be checked before any work is done. |
| **R10** | A run with `--threads 1` MUST be bit-identical to a run before this spec, on every query. The parallel path is an addition, not a replacement. |

## 4. The two tables

This spec publishes **two** comparison tables, and the distinction between them is the point.

**The pinned table** — one thread on both sides. The like-for-like claim, unchanged in shape.
It is the number a reader should quote when they ask "is this engine faster than DuckDB", and
after this spec it will show the engine's *query* winning and its *cold path* still behind,
because the cold path is where the parallelism would help and the cold path is pinned.

**The scaling table** — the engine and both incumbents at 1, 2, 4, 8 and 14 threads, on the
same file, in the same harness. This is where the parallelism becomes visible, and it is the
table that shows whether the gap was ever really about the engine.

The framing rule still holds for both: **"kernel X on query Y"**, never "this database versus
DuckDB". A scaling table is a statement about how a number moves with the thread count, on one
machine, on one file.

## 5. What this spec is not

| | why |
|---|---|
| **Changing the headline comparison** | The pinned table stays. Principle V requires the incumbents to run at the engine's thread count, and the headline's job is to be like-for-like. |
| **A new dependency** | `std::thread::scope` and a work counter are enough. The workspace bans `rand` and keeps its dependency graph small; a thread pool crate would be the largest dependency in the project for the smallest possible gain. |
| **Parallel operators** | The filter, project and aggregate stay single-threaded. The scan is where the bytes are, and the scan is 90% of the cold path. |
| **More row groups** | The file has six row groups, which caps the parallelism at six — enough, since DuckDB's observed speedup from fourteen cores is 3.3×. Splitting the file into more, smaller row groups would change the dataset, and the dataset is meant to model a real TPC-H file. |
| **Parallel decode for the eager path** | The same machinery applies, and it is worth doing: the eager path reads 231 MiB. Whether it lands in this spec or the next is a question of scope, and it is written down here as a follow-on rather than quietly dropped. |
| **A vectorised dictionary gather** | Unchanged from spec 003: AVX2 has no gather instruction. |

## 6. Acceptance criteria

1. `cargo test --workspace --all-targets` is green, including a new test that the parallel and
   single-threaded paths produce **bit-identical** results, over every built-in query.
2. `just compare --threads 1` produces the same answers as before this spec — checked by the
   harness's own cross-check, which refuses to report a disagreement.
3. A scaling table exists, with the engine and both incumbents at the same thread counts.
4. The plan output reports the thread count.
5. Every published figure carries its thread count, as constitution principle IV requires.

## 7. References

| requirement | principle |
|---|---|
| R1, R2, R3 | **IV.** Evidence Over Claims — the parallel answer must be the same answer, bit for bit. |
| R4 | **I.** One Predicate, Five Implementations, One Answer — the dictionary merge is the same operation, wherever it happens. |
| R5 | **IV.** Evidence Over Claims — a number without its thread count is a claim, not evidence. |
| R6, R7, R8 | **V.** The Comparison Is Reproducible by a Stranger — and fair, which means symmetric. |
| R9 | The performance standards: the per-query working set is bounded by a declared budget. |
| R10 | **VII.** Explicit Scope, No Silent Deferral — the addition changes nothing that already worked. |
