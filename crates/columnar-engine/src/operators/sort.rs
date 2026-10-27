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

use crate::operators::key::KeyValue;

/// Sorts `rows` by their key.
///
/// `key_at` gives the key of a row, or `None` when the row holds no value.
/// The rows are the positions of the rows to sort, and the result holds the
/// same positions in the sorted order.
///
/// With a `limit` the sort keeps the best `limit` rows and drops the rest;
/// without one it sorts every row.
///
/// The order is deterministic: equal keys are broken by the row's own
/// position, so the same input always produces the same output.
#[must_use]
pub fn sort_rows<'a>(
    rows: Vec<u32>,
    mut key_at: impl FnMut(u32) -> Option<KeyValue<'a>>,
    descending: bool,
    limit: Option<usize>,
) -> Vec<u32> {
    match limit {
        None => full_sort(rows, &mut key_at, descending),
        Some(0) => Vec::new(),
        Some(limit) => bounded_sort(rows, &mut key_at, descending, limit),
    }
}

/// Sorts every row.
fn full_sort<'a>(
    mut rows: Vec<u32>,
    key_at: &mut impl FnMut(u32) -> Option<KeyValue<'a>>,
    descending: bool,
) -> Vec<u32> {
    rows.sort_unstable_by(|left, right| {
        let left_key = key_at(*left);
        let right_key = key_at(*right);
        compare(left_key.as_ref(), right_key.as_ref(), descending, *left, *right)
    });
    rows
}

/// Sorts the rows, keeping the best `limit` of them.
///
/// The heap holds the best rows seen so far, and its top is the worst of
/// them: the row that would be dropped first. A row that is not better than
/// the top is dropped on the spot.
fn bounded_sort<'a>(
    rows: Vec<u32>,
    key_at: &mut impl FnMut(u32) -> Option<KeyValue<'a>>,
    descending: bool,
    limit: usize,
) -> Vec<u32> {
    let mut heap: BinaryHeap<Entry<'a>> = BinaryHeap::new();
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
