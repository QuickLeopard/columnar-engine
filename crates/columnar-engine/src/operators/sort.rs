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
//! ## The comparison, and the blocks it is read in
//!
//! A sort with a limit reads **every** key: a top-k over six million keys has
//! to look at all of them, so the read is the floor and no algorithm gets
//! under it. What is left is the **comparison** -- one per key -- and that is
//! where the work is. Compared a row at a time, it is a scalar loop: the
//! comparison, the row's own number, and a branch, six million times over.
//! That costs more than the memory it reads.
//!
//! So the keys are compared in **blocks**. A block is compared against the
//! **threshold** -- the heap's worst key, which is the row the heap would
//! drop next -- and only a block that holds a row that beats the threshold is
//! walked a row at a time. The block comparison is a reduction over a
//! comparison, which is the shape a compiler vectorises; the row at a time
//! walk then touches a handful of rows out of six million.
//!
//! The threshold is the heap's own worst key, so the blocks that are walked
//! are the blocks that hold a row the heap would keep, and the sort's answer
//! is the answer the row at a time walk gives -- the same rows, in the same
//! order.
//!
//! The blocks are only possible when the rows are a **contiguous run**: the
//! keys of a run are a slice of the column, and a slice can be compared in
//! blocks. A sort that is handed a scattered selection reads its keys a row
//! at a time, as it always did.
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

use crate::operators::key::{KeyColumn, KeyValue, PlainNumbers};

/// How many keys the block comparison reads at a time.
///
/// The block is the unit the comparison is vectorised over, and it is also
/// the unit that is walked a row at a time when it holds a row the heap
/// would keep. A small block finds its winners sooner and walks more of
/// them; a large one walks less and finds them later. The published query
/// keeps ten rows out of six million, and every size in between finds the
/// same handful of rows.
const BLOCK: usize = 1024;

/// The rows a sort is given.
///
/// The rows are a **source**, not a list: a sort with a limit keeps the best
/// `limit` rows and drops the rest, so it never needs more than the heap and
/// the row it is looking at. A list of a million row numbers is a megabyte of
/// memory the allocator has to hand out for rows the sort reads once.
///
/// A source that holds a **contiguous run** of the dataset's rows says so,
/// and the sort then compares the keys in blocks. A source that does not is
/// read a row at a time.
pub trait RowSource: IntoIterator<Item = u32> {
    /// The rows, as a contiguous run, when the source holds one.
    fn run(&self) -> Option<std::ops::Range<u32>>;
}

impl RowSource for std::ops::Range<u32> {
    fn run(&self) -> Option<std::ops::Range<u32>> {
        Some(self.clone())
    }
}

impl RowSource for Vec<u32> {
    fn run(&self) -> Option<std::ops::Range<u32>> {
        None
    }
}

impl RowSource for std::iter::Copied<std::slice::Iter<'_, u32>> {
    fn run(&self) -> Option<std::ops::Range<u32>> {
        None
    }
}

impl RowSource for columnar_core::SelectionIter<'_> {
    fn run(&self) -> Option<std::ops::Range<u32>> {
        None
    }
}

/// A value the sort can put in its total order.
///
/// The key is the value's place in the **total** order, and the order is
/// total: two values are equal only when they are the same value, so the
/// sort's tie-break -- the row's own number -- decides every pair, in both
/// directions.
trait TotalKey: Copy + Default {
    /// The value's key in the total order.
    fn total_key(self) -> i64;
}

impl TotalKey for f64 {
    /// The transform `total_cmp` uses.
    ///
    /// A negative double's bits are flipped, so that the integers' order is
    /// the doubles' order: a NaN is above every number, and a negative zero
    /// is below a positive one.
    fn total_key(self) -> i64 {
        let bits = i64::from_ne_bytes(self.to_bits().to_ne_bytes());
        bits ^ ((bits >> 63) & i64::MAX)
    }
}

impl TotalKey for i64 {
    /// The transform `total_cmp` uses.
    ///
    /// It is the same order the integers compare in, so the transform changes
    /// nothing about how two of them are ordered.
    fn total_key(self) -> i64 {
        self ^ ((self >> 63) & i64::MAX)
    }
}

/// A value in the sort's total order.
#[derive(Debug, Clone, Copy)]
struct Ordered<T>(T);

impl<T: TotalKey> PartialEq for Ordered<T> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<T: TotalKey> Eq for Ordered<T> {}

impl<T: TotalKey> PartialOrd for Ordered<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: TotalKey> Ord for Ordered<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_key().cmp(&other.0.total_key())
    }
}

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
    I: RowSource,
{
    // The **fast path**: a plain number column with no NULLs. The key is
    // read as the column's own value -- a number, compared as a number --
    // with no enum built for it and no `Option` wrapped around it. The
    // general path below reads a `KeyValue`, which is 24 bytes wide because
    // one of its variants is a string, and building one of those a row was
    // the sort's largest cost after the key read itself.
    match keys.plain_numbers() {
        Some(PlainNumbers::Doubles(values)) => {
            return sort_numbers(rows, values, descending, limit);
        }
        Some(PlainNumbers::Integers(values)) => {
            return sort_numbers(rows, values, descending, limit);
        }
        None => {}
    }
    let mut key_at = |row: u32| keys.value_at(usize::try_from(row).unwrap_or(usize::MAX));
    match limit {
        None => full_sort(rows, &mut key_at, descending),
        Some(0) => Vec::new(),
        Some(limit) => bounded_sort(rows, &mut key_at, descending, limit),
    }
}

/// Sorts the rows over a column of plain numbers.
///
/// The numbers are compared in **blocks** when the rows are a contiguous run:
/// a block is compared against the threshold -- the heap's worst key -- and
/// only a block that holds a row that beats it is walked a row at a time.
/// The comparison is the whole of the sort's work, and a block comparison is
/// the shape a compiler vectorises.
///
/// The rows that are not a run are read a row at a time, as they always
/// were.
fn sort_numbers<T: TotalKey, I: RowSource>(
    rows: I,
    values: &[T],
    descending: bool,
    limit: Option<usize>,
) -> Vec<u32> {
    match limit {
        None => {
            let mut entries: Vec<ValueEntry<Ordered<T>>> = rows
                .into_iter()
                .map(|row| ValueEntry { key: Ordered(value_at(values, row)), row, descending })
                .collect();
            entries.sort_unstable_by(|left, right| {
                order_keys(left.key.cmp(&right.key), descending, left.row, right.row)
            });
            entries.into_iter().map(|entry| entry.row).collect()
        }
        Some(0) => Vec::new(),
        Some(limit) => {
            if let Some(run) = rows.run() {
                if let Some(window) = window_of(values, &run) {
                    return bounded_blocks(window, run.start, descending, limit);
                }
            }
            bounded_rows(rows, values, descending, limit)
        }
    }
}

/// The value at `row`, or the default when the row is out of range.
fn value_at<T: Copy + Default>(values: &[T], row: u32) -> T {
    values.get(usize::try_from(row).unwrap_or(usize::MAX)).copied().unwrap_or_default()
}

/// The values of a run of rows, when the run is inside the column.
fn window_of<'a, T>(values: &'a [T], run: &std::ops::Range<u32>) -> Option<&'a [T]> {
    let start = usize::try_from(run.start).ok()?;
    let end = usize::try_from(run.end).ok()?;
    values.get(start..end)
}

/// The bounded sort over a column of plain numbers, a row at a time.
///
/// It is the shape the sort has always had, and it is what the blocks fall
/// back to for a row set that is not a run.
fn bounded_rows<T: TotalKey, I: IntoIterator<Item = u32>>(
    rows: I,
    values: &[T],
    descending: bool,
    limit: usize,
) -> Vec<u32> {
    let mut heap: BinaryHeap<ValueEntry<Ordered<T>>> = BinaryHeap::with_capacity(limit + 1);
    for row in rows {
        let entry = ValueEntry { key: Ordered(value_at(values, row)), row, descending };
        if heap.len() >= limit && !better_than(&heap, &entry, descending) {
            continue;
        }
        if heap.len() >= limit {
            heap.pop();
        }
        heap.push(entry);
    }
    ordered_rows(heap)
}

/// The bounded sort over a contiguous run of plain numbers, in blocks.
///
/// A block is compared against the threshold -- the heap's worst key -- and
/// only a block that holds a row that beats it is walked a row at a time.
/// The heap, the threshold and the order are the ones the row at a time walk
/// uses, so the two keep the same rows in the same order.
fn bounded_blocks<T: TotalKey>(
    values: &[T],
    start: u32,
    descending: bool,
    limit: usize,
) -> Vec<u32> {
    let mut heap: BinaryHeap<ValueEntry<Ordered<T>>> = BinaryHeap::with_capacity(limit + 1);
    // The threshold: the heap's worst key, in the direction the sort is
    // going. It stays the lowest until the heap holds as many rows as the
    // limit asks for, so that every row is kept until then.
    let mut threshold = i64::MIN;
    let mut row = start;
    let mut rest = values;
    while !rest.is_empty() {
        let take = BLOCK.min(rest.len());
        let (block, tail) = rest.split_at(take);
        rest = tail;
        if count_at_least(block, threshold, descending) == 0 {
            row = row.saturating_add(u32::try_from(take).unwrap_or(u32::MAX));
            continue;
        }
        for (offset, value) in block.iter().enumerate() {
            let candidate = row.saturating_add(u32::try_from(offset).unwrap_or(u32::MAX));
            let entry = ValueEntry { key: Ordered(*value), row: candidate, descending };
            if heap.len() >= limit {
                if !better_than(&heap, &entry, descending) {
                    continue;
                }
                heap.pop();
            }
            heap.push(entry);
            if heap.len() >= limit {
                if let Some(worst) = heap.peek() {
                    threshold = directed_key(worst.key.0, descending);
                }
            }
        }
        row = row.saturating_add(u32::try_from(take).unwrap_or(u32::MAX));
    }
    ordered_rows(heap)
}

/// Whether the entry is better than the heap's worst row.
fn better_than<T: TotalKey>(
    heap: &BinaryHeap<ValueEntry<Ordered<T>>>,
    entry: &ValueEntry<Ordered<T>>,
    descending: bool,
) -> bool {
    heap.peek().is_some_and(|worst| {
        order_keys(entry.key.cmp(&worst.key), descending, entry.row, worst.row) == Ordering::Less
    })
}

/// The heap's rows, in the order they come out.
fn ordered_rows<T: TotalKey>(heap: BinaryHeap<ValueEntry<Ordered<T>>>) -> Vec<u32> {
    let mut entries = heap.into_vec();
    entries.sort_unstable_by(|left, right| {
        order_keys(left.key.cmp(&right.key), left.descending, left.row, right.row)
    });
    entries.into_iter().map(|entry| entry.row).collect()
}

/// Counts the values in the block that are at least as good as the threshold.
///
/// It is the whole of the comparison's work for the block, and it is a
/// reduction over a comparison -- the shape a compiler vectorises.
#[inline]
fn count_at_least<T: TotalKey>(values: &[T], threshold: i64, descending: bool) -> u64 {
    let mut count = 0u64;
    for value in values {
        count += u64::from(directed_key(*value, descending) >= threshold);
    }
    count
}

/// The value's key in the direction the sort is going.
///
/// The direction is folded into the key, so that the comparison is always
/// "at least as good": the sort's own order reverses the key's order for a
/// descending sort, and nothing else.
#[inline]
fn directed_key<T: TotalKey>(value: T, descending: bool) -> i64 {
    let key = value.total_key();
    if descending {
        key
    } else {
        !key
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
