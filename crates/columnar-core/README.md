# columnar-core

**The shared vocabulary of the columnar engine.** This crate holds the types that the storage
layer produces, the operators consume, and the comparison kernels are measured against: physical
types and scalar values, validity masks, selection vectors, column chunks and their encodings,
batches, the comparison predicates, and the arithmetic expressions an aggregate can sum.

It has **no dependencies**, on purpose. The vocabulary must not be able to drift, and a vocabulary
with dependencies is a vocabulary with a version history. The crate root is flat: every type it has
is re-exported at the root.

---

## A runnable example

```rust
use columnar_core::{
    ComparisonOp, DataType, NullBitmap, Predicate, ScalarValue, SelectionKind, SelectionVector,
};

fn main() {
    // A NULL is the absence of a bit, never a sentinel inside a value array.
    let validity = NullBitmap::with_nulls(4, &[1]);
    assert!(validity.is_valid(0));
    assert!(!validity.is_valid(1));
    assert_eq!(validity.null_count(), 1);

    // A predicate is a column, an operator and a threshold.
    let predicate = Predicate::new("l_discount", ComparisonOp::GtEq, 0.04_f64);
    assert_eq!(predicate.column, "l_discount");
    assert_eq!(predicate.op, ComparisonOp::GtEq);
    assert_eq!(predicate.value, ScalarValue::Float64(0.04));
    assert_eq!(predicate.value.data_type(), DataType::Float64);

    // A selection holds the rows that survived, in one of three representations.
    let selection = SelectionVector::all(4);
    assert_eq!(selection.kind(), SelectionKind::All);
    assert_eq!(selection.iter().collect::<Vec<u32>>(), vec![0, 1, 2, 3]);
}
```

---

## The guarantees

**A NULL is an absent bit.** The value array is dense and always holds a real value for every row;
the validity mask says which of those values a predicate is allowed to look at. That is the whole
reason the comparison kernels can be branchless on the null path: there is no sentinel to test for.

**The mask layout is part of the contract.** Row `i` lives in word `i / 64` at bit `i % 64`,
least-significant-bit first. It is the layout `DuckDB`, `Arrow` and the Parquet validity encoding
all use, so a mask can be handed to a comparator without a repack — and changing the layout would
change the bytes a published hash covers.

**The type rules are short, and they are not negotiable.** An expression's type is `int64` only if
every operand is `int64`, and `float64` otherwise; an `int64` in a `float64` expression is
widened, and a `float64` in an `int64` expression is not. A predicate's threshold must have the
column's physical type, because an implicit cast is where a query planner starts lying about what
it did.

**The selection's rows are read, not copied.** A selection that holds every row of a batch *is* the
range: reading its rows costs nothing, because there is no bitmap to walk and no vector of row
numbers to build. A selection that holds some of them is read through the iterator, a row at a
time.

**The representation is chosen at runtime, by a named threshold.** A selection below
`BITMAP_DISPATCH_DENSITY` is an index list of `u32` row numbers; at or above it, a bitmap of one
bit per source row. The threshold is a named constant so that the code, the benchmark and the
comment cannot drift apart.

---

## The boundaries

**Three physical types.** `int64`, `float64` and `utf8`. There is no `Decimal` and no `Date`,
because a richer type system than the engine implements would be a claim rather than evidence.

**No NULL as a value.** A NULL is an absence recorded in a validity mask. `ScalarValue` carries the
values that exist; the mask says which rows hold one.

**No dependencies.** Not on `serde`, and not on anything else. Deriving `Serialize` on these types
would make it easy to serialise a column chunk in a format that is not the engine's canonical
encoding — exactly the class of bug that makes a replay hash unstable across toolchain versions.

**Not a general expression language.** An expression computes a number. There are no comparisons
in an expression, no `CASE`, no function calls, no casts and no subqueries.

**Integer division is refused.** The engines disagree — `5 / 2` is `2` in `PostgreSQL` and `2.5`
in `DuckDB` — and an engine that silently picks one of them is an engine whose answers are not
portable. The engine refuses it, and says why.

**The types that will grow are closed for construction.** `ScalarValue` and `Expr` are
`#[non_exhaustive]` enums, and `Predicate` is a `#[non_exhaustive]` struct: a type this general
grows, and an exhaustive match — or a struct literal — on it is a breaking change the moment it
does. `Predicate::new` is the one way in, and the fields stay public and readable.

---

## Where to go next

The **embedding guide** —
[`docs/embedding.md`](https://github.com/QuickLeopard/columnar-engine/blob/main/docs/embedding.md) —
is the walk-through of the crate that uses this vocabulary: opening the data, building a plan,
running it, reading the result, and the errors.

The **rustdoc** carries the full API, and `ENGINE_VERSION` is stamped into every published
benchmark result so that a number can be traced to the code that produced it.
