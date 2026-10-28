//! The sort.
//!
//! The sort is `ORDER BY <column> [DESC] [LIMIT n]`, and it is the last
//! stage before the aggregate: a plan with a sort and an aggregation
//! aggregates over the **sorted** rows, not over all of them.
//!
//! ## The limit, and the heap behind it
//!
//! With a limit the sort keeps a **bounded heap** of `n` rows rather than
//! sorting everything. The heap holds the best `n` rows seen so far, and the
//! row at its top is the *worst* of them -- the one that would be dropped
//! first. A row that is not better than that one is dropped on the spot, so
//! the sort's working set is `n` rows however many rows it is handed. The
//! published query takes ten rows out of six million, and this is what makes
//! that ten rows' worth of memory rather than six million rows' worth.
//!
//! Without a limit every row is sorted.
//!
//! ## The order, and what makes it deterministic
//!
//! Ties are broken by the **row index**: two rows with equal keys come out
//! in the order they went in, whichever way the sort is going. The order is
//! therefore total, and the same plan over the same data always produces the
//! same rows in the same order.
//!
//! A **NULL** key sorts after every key that holds a value, in both
//! directions. A row with no key has no place in the order, and SQL's
//! `NULLS LAST` is the rule this engine follows.
//!
//! Floats are ordered with `total_cmp`, the same rule the aggregate's `MIN`
//! and `MAX` use, so a NaN is *ordered* rather than refused.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::operators::key::{KeyColumn, KeyValue};

/// Sorts the rows `source` names, by their key.
///
/// `key_at` gives the key of a row, or `None` when the row holds no value.
/// The rows are the positions of the rows to sort, and the result holds the
/// same positions in the sorted order.
///
/// The rows are a **source**, not a list: a sort with a limit keeps the best
/// `limit` rows and drops the rest, so it never needs more than the heap and
/// the row it is looking at. A list of a million row numbers is a megabyte of
/// memory the allocator has to hand out for rows the sort reads once.
///
/// With a `limit` the sort keeps the best `limit` rows and drops the rest;
/// without one it sorts every row.
///
/// The order is deterministic: equal keys are broken by the row's own
/// position, so the same input always produces the same output.
#[must_use]
pub fn sort_rows<I>(
    rows: I,
    keys: &KeyColumn<'_>,
    descending: bool,
    limit: Option<usize>,
) -> Vec<u32>
where
    I: IntoIterator<Item = u32>,
{
    // The **fast path**: a plain number column with no NULLs. The key is
    // read as the column's own value -- a number, compared as a number --
    // with no enum built for it and no `Option` wrapped around it. The
    // general path below reads a `KeyValue`, which is 24 bytes wide because
    // one of its variants is a string, and building one of those a row was
    // the sort's largest cost after the key read itself.
    if !keys.has_nulls() {
        if let Some(values) = keys.slice().doubles() {
            return sort_values(
                rows,
                |row| {
                    TotalOrd(
                        values
                            .get(usize::try_from(row).unwrap_or(usize::MAX))
                            .copied()
                            .unwrap_or_default(),
                    )
                },
                descending,
                limit,
            );
        }
        if let Some(values) = keys.slice().integers() {
            return sort_values(
                rows,
                |row| {
                    values
                        .get(usize::try_from(row).unwrap_or(usize::MAX))
                        .copied()
                        .unwrap_or_default()
                },
                descending,
                limit,
            );
        }
    }
    let mut key_at = |row: u32| keys.value_at(usize::try_from(row).unwrap_or(usize::MAX));
    match limit {
        None => full_sort(rows, &mut key_at, descending),
        Some(0) => Vec::new(),
        Some(limit) => bounded_sort(rows, &mut key_at, descending, limit),
    }
}

/// A double, ordered by its bits.
///
/// `f64` is not `Ord`, and its `PartialOrd` is not a total order -- `NaN`
/// compares unequal to everything, including itself. The sort needs a total
/// order, so a double is ordered by its bits: `total_cmp` puts the NaNs
/// together, at the end, and orders the numbers the way they compare.
#[derive(Debug, Clone, Copy)]
struct TotalOrd(f64);

impl PartialEq for TotalOrd {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for TotalOrd {}

impl PartialOrd for TotalOrd {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TotalOrd {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

/// Sorts the rows by their keys, keeping the best `limit` of them.
///
/// The key is read once a row and compared as the number it is. The
/// direction and the tie-break are the sort's own, so the order is the same
/// the general path produces.
fn sort_values<K: Copy + Ord, I>(
    rows: I,
    key_at: impl Fn(u32) -> K,
    descending: bool,
    limit: Option<usize>,
) -> Vec<u32>
where
    I: IntoIterator<Item = u32>,
{
    match limit {
        None => {
            let mut entries: Vec<ValueEntry<K>> = rows
                .into_iter()
                .map(|row| ValueEntry { key: key_at(row), row, descending })
                .collect();
            entries.sort_unstable_by(|left, right| {
                order_keys(left.key.cmp(&right.key), descending, left.row, right.row)
            });
            entries.into_iter().map(|entry| entry.row).collect()
        }
        Some(0) => Vec::new(),
        Some(limit) => {
            let mut heap: BinaryHeap<ValueEntry<K>> = BinaryHeap::with_capacity(limit + 1);
            for row in rows {
                let key = key_at(row);
                if heap.len() >= limit {
                    let better = match heap.peek() {
                        Some(worst) => {
                            order_keys(key.cmp(&worst.key), descending, row, worst.row)
                                == Ordering::Less
                        }
                        None => false,
                    };
                    if !better {
                        continue;
                    }
                    heap.pop();
                }
                heap.push(ValueEntry { key, row, descending });
            }
            let mut entries = heap.into_vec();
            entries.sort_unstable_by(|left, right| {
                order_keys(left.key.cmp(&right.key), descending, left.row, right.row)
            });
            entries.into_iter().map(|entry| entry.row).collect()
        }
    }
}

/// One row in the sort's bounded heap, with its key.
#[derive(Debug, Clone)]
struct ValueEntry<K> {
    /// The row's key.
    key: K,
    /// The row's own number.
    row: u32,
    /// The order the heap is keeping.
    descending: bool,
}

impl<K: Ord> PartialEq for ValueEntry<K> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<K: Ord> Eq for ValueEntry<K> {}

impl<K: Ord> PartialOrd for ValueEntry<K> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<K: Ord> Ord for ValueEntry<K> {
    /// The greatest entry is the one that sorts **last**: the row that would
    /// be dropped first. That is what the heap needs, because the row it
    /// drops is the one at its top.
    fn cmp(&self, other: &Self) -> Ordering {
        order_keys(self.key.cmp(&other.key), self.descending, self.row, other.row)
    }
}

/// The order of two rows by their keys: the key's order, the direction, and
/// the row's own number as the tie-break.
fn order_keys(ordering: Ordering, descending: bool, left_row: u32, right_row: u32) -> Ordering {
    let ordering = if descending { ordering.reverse() } else { ordering };
    if ordering == Ordering::Equal {
        left_row.cmp(&right_row)
    } else {
        ordering
    }
}

/// Sorts every row.
///
/// Each row's key is read **once**, before the sort begins. The sort used
/// to read both keys of every comparison, and a comparison is made
/// `O(n log n)` times -- so a row's key was read `O(log n)` times over, and
/// the key read was the sort's only super-linear term.
fn full_sort<'a, I>(
    rows: I,
    key_at: &mut impl FnMut(u32) -> Option<KeyValue<'a>>,
    descending: bool,
) -> Vec<u32>
where
    I: IntoIterator<Item = u32>,
{
    let mut entries: Vec<Entry<'a>> =
        rows.into_iter().map(|row| Entry { key: key_at(row), row, descending }).collect();
    entries.sort_unstable_by(|left, right| {
        compare(left.key.as_ref(), right.key.as_ref(), descending, left.row, right.row)
    });
    entries.into_iter().map(|entry| entry.row).collect()
}

/// Sorts the rows, keeping the best `limit` of them.
///
/// The heap holds the best rows seen so far, and its top is the worst of
/// them: the row that would be dropped first. A row that is not better than
/// the top is dropped on the spot.
fn bounded_sort<'a, I>(
    rows: I,
    key_at: &mut impl FnMut(u32) -> Option<KeyValue<'a>>,
    descending: bool,
    limit: usize,
) -> Vec<u32>
where
    I: IntoIterator<Item = u32>,
{
    let mut heap: BinaryHeap<Entry<'a>> = BinaryHeap::with_capacity(limit + 1);
    for row in rows {
        let key = key_at(row);
        if heap.len() >= limit {
            let better = match heap.peek() {
                Some(worst) => {
                    compare(key.as_ref(), worst.key.as_ref(), descending, row, worst.row)
                        == Ordering::Less
                }
                None => false,
            };
            if !better {
                continue;
            }
            heap.pop();
        }
        heap.push(Entry { key, row, descending });
    }

    let mut entries = heap.into_vec();
    entries.sort_unstable_by(|left, right| {
        compare(left.key.as_ref(), right.key.as_ref(), descending, left.row, right.row)
    });
    entries.into_iter().map(|entry| entry.row).collect()
}

/// One row in the sort's bounded heap.
#[derive(Debug, Clone)]
struct Entry<'a> {
    /// The row's key, if the row holds one.
    key: Option<KeyValue<'a>>,
    /// The row's own position.
    row: u32,
    /// The order the heap is keeping.
    descending: bool,
}

impl PartialEq for Entry<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Entry<'_> {}

impl PartialOrd for Entry<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Entry<'_> {
    /// The rows are ordered the way they come out, so the **greatest** row
    /// is the one that sorts last: the worst of them. That is what the heap
    /// needs, because the row it drops is the one at its top.
    fn cmp(&self, other: &Self) -> Ordering {
        compare(self.key.as_ref(), other.key.as_ref(), self.descending, self.row, other.row)
    }
}

/// Compares two rows by their keys.
///
/// `Ordering::Less` means the left row sorts **first**. Equal keys are
/// broken by the row's own position, so the order is total -- and the same
/// pair of rows always compares the same way, which is what lets the sort
/// use an unstable sort and still be deterministic.
fn compare(
    left: Option<&KeyValue<'_>>,
    right: Option<&KeyValue<'_>>,
    descending: bool,
    left_row: u32,
    right_row: u32,
) -> Ordering {
    let ordering = match (left, right) {
        // A row with no key sorts after every row that has one, in both
        // directions.
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(left), Some(right)) => {
            let ordering = compare_values(left, right);
            if descending {
                ordering.reverse()
            } else {
                ordering
            }
        }
    };
    match ordering {
        Ordering::Equal => left_row.cmp(&right_row),
        other => other,
    }
}

/// Compares two keys.
fn compare_values(left: &KeyValue<'_>, right: &KeyValue<'_>) -> Ordering {
    match (left, right) {
        (KeyValue::Int(left), KeyValue::Int(right)) => left.cmp(right),
        (KeyValue::Float(left), KeyValue::Float(right)) => left.total_cmp(right),
        (KeyValue::Text(left), KeyValue::Text(right)) => left.cmp(right),
        // The two keys come from the same column, so their types agree. The
        // arm is here because the compiler does not know that.
        _ => Ordering::Equal,
    }
}
