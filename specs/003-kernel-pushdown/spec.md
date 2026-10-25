# Spec 003 — The pushdown runs the engine's kernel

**Status**: accepted
**Date**: 2026-10-25
**Extends**: spec 002 (the lazy scan and the pushdown)
**Related**: ADR-0010 (the pushdown implements the engine's semantics, not Arrow's)

---

## 1. The problem, measured

Spec 002 put the plan's predicates into the Parquet reader and got the cold `q6` query from
1972 ms to 121 ms. It left one number unexplained.

Spec 002's own measurement of the pushdown's defects put the reader's double decode at 358 ms,
the dictionary copy at 172 ms and the mask builder at 137 ms — but the number that survived,
**121.5 ms**, was never attributed. It was a total, and this project's constitution does not
accept a total as an explanation.

So the scan stage was instrumented. The pushdown runs *inside* the reader, so its cost was
folded into the scan's elapsed time and nothing said how much of that was the reader's decode
and how much was the comparison. Those have different fixes, so they were separated:

| query | scan | of which the pushdown | the reader's decode |
|---|---|---|---|
| `q6` | 121.2 ms | **42.9 ms** (35%) | 78.3 ms |
| `filter-numeric` | 40.7 ms | **21.8 ms** (54%) | 18.9 ms |
| `filter-string` | 41.7 ms | **21.3 ms** (51%) | 20.4 ms |

**The pushdown was the larger half of the single-column scans.**

Then the comparison with the engine's own filter:

> The engine's filter runs four predicates over 6,000,000 rows in **6.7 ms**.
> The pushdown runs the same four predicates in **42.9 ms**.

**6.4× slower, for the same work.** The reason was visible in the code: the pushdown compared
values one row at a time, in Rust, in a loop, while the filter ran the SIMD kernel over the
engine's own chunk format.

## 2. Why the pushdown could not simply call the kernel

The obvious fix — call the kernel — looks impossible at first, and the reason is worth writing
down:

- The kernel works on the engine's **own chunk format**: a dense `Vec<f64>` plus a `NullBitmap`.
- The pushdown is handed an **Arrow array**, inside Arrow's reader, and cannot change that.
- So the naive fix is to convert the Arrow array into the engine's format, and *then* compare.

That conversion is a copy of every value. It would cost more than the comparison it saves, and
spec 002's follow-ons said so: *"Using it here would mean converting the column first, which
costs more than the scalar comparison saves."*

**That is true of the chunk format, and false of the values.** An Arrow `Float64Array` holds its
values in a contiguous `&[f64]`, and `ScalarBuffer` folds any slice offset into the pointer, so
`array.values()` is exactly the array's values with no copy. The kernel takes `&[f64]` and writes
an LSB-first `Vec<u64>` mask — which is the layout `MatchMask` already used, and the layout
Arrow's boolean buffer wants.

The conversion that was assumed to be necessary is the *values* being in a different
representation. They are not. Arrow's `Float64Array` and the engine's `Float64` encoding hold the
same bytes.

## 3. What this spec changes

The pushdown's numeric comparisons run **the engine's own kernel**, over Arrow's raw value
slice, with the same flavor the filter would have used.

The consequence is larger than the speedup:

> **The pushdown and the filter stop being two implementations that have to be checked against
> each other, and become one implementation.**

ADR-0010's argument was that a pushdown using Arrow's comparisons would answer a different
question from the filter, and that the divergence had to be held down by a differential test. That
argument is now stronger than it was: for numeric columns the pushdown *is* the filter's
comparison code. The differential test has not been weakened — it still runs, and it still checks
every operator and every edge — but it is now checking two paths that share a function, rather
than two paths that must be made to agree.

The pushdown's own float comparison is **deleted**. It is not kept as a fallback and it is not
kept as a reference: the module no longer contains a float comparison of its own, so there is
nothing left to drift.

## 4. Requirements

| | requirement |
|---|---|
| **R1** | The pushdown's `int64` and `float64` comparisons MUST run the engine's comparison kernel, over Arrow's raw value slice, with no copy of the values. |
| **R2** | The pushdown MUST contain no float comparison of its own. The module's float semantics are the kernel's, by construction. |
| **R3** | The pushdown MUST use the same kernel flavor the filter would use for the same plan, so the two paths run the same code. |
| **R4** | The pushdown's dictionary map MUST be branchless and accumulated a word at a time, as the engine's own dictionary path is. |
| **R5** | A NULL row MUST never match, for every operator, including `!=`. The kernel's contract provides this; the pushdown MUST pass the array's validity to it. |
| **R6** | The scan stage MUST report what the pushdown cost, and over how many rows, so that the reader's decode and the comparison can be told apart. |
| **R7** | The eager and lazy paths MUST still agree, row for row, on every operator and every edge the rules mention. The differential test is not weakened by this spec. |
| **R8** | A comparison against a threshold of the wrong type MUST still be an error, not a silent false. |

## 5. What this spec is not

| | why |
|---|---|
| **A string kernel** | The kernel is numeric. Strings are compared byte-wise, which is a memcmp, and a SIMD string comparison would be a different project. The string path stays scalar. |
| **A vectorised dictionary gather** | The dictionary map is a gather — `mask[row] = distinct[key[row]]`. AVX2 has no gather; AVX-512's `vpgatherdd` is not in the kernel's target set. The map is made branchless instead, which is what the engine's own path does. |
| **Page-level pruning** | Unchanged from spec 002, and — as measured — not the gap. See `PERF.md` §2c. |
| **Parallel decode** | Unchanged. This spec makes one thread's work cheaper; it does not add threads. That is spec 005. |
| **Changing the engine's comparison semantics** | Not touched. The kernel's rules — NULL never matches, NaN never matches, `-0.0 == 0.0` — are the engine's rules, and the pushdown now inherits them rather than reimplementing them. |

## 6. Acceptance criteria

1. `cargo test --workspace --all-targets` is green, including the 15 differential tests in
   `crates/columnar-engine/tests/lazy_scan.rs` and the 4 end-to-end tests in
   `crates/columnar-cli/tests/lazy_scan.rs`.
2. The pushdown module contains no float comparison: `grep -n "fn compare_f64" pushdown.rs`
   finds nothing.
3. The plan output reports the pushdown's cost and the number of rows it was evaluated over.
4. The cold `q6` query is faster than the 121.5 ms recorded in `PERF.md`, and the improvement is
   attributed in the same file.

## 7. References

| requirement | principle |
|---|---|
| R1, R2, R3 | **I.** One Predicate, Five Implementations, One Answer — the pushdown stops being a sixth implementation. |
| R4 | **II.** The Baseline Is a First-Class Citizen — the pushdown is held to the engine's own standard, not a looser one. |
| R5 | **III.** NULL Is an Absent Bit, and It Never Matches. |
| R6 | **IV.** Evidence Over Claims — a total is not an explanation. |
| R7 | **IV.** Evidence Over Claims — the equivalence is checked, not asserted. |
| R8 | **VII.** Explicit Scope, No Silent Deferral — a wrong type is an error, not a quiet false. |
