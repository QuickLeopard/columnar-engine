//! The hash join.
//!
//! The join is an **inner equi-join** on **one** key column, and it is run as
//! a hash join: the **build side** is the right table, and it is read
//! through the engine's own [`ParquetSource`], so the same reader settings
//! apply to it as to any other file. The build side's own predicates are
//! applied **before** the table is hashed, so a row the build filter rejects
//! is never in the hash table and never matches anything.
//!
//! The **probe side** is the left table, and it stays lazy: the plan's own
//! predicates are pushed into the scan, so a row that fails them never
//! reaches the join, and the probe side is never materialised as a table of
//! its own.
//!
//! ## What the output holds
//!
//! The output is the **probe side's** rows, in the probe side's order: the
//! join is deterministic, and the same plan over the same data produces the
//! same rows in the same order, every time.
//!
//! A probe row that matches several build rows produces one output row per
//! match -- an inner join, not a semi-join. The build side's *columns* are
//! not carried into the output: the plan's projection is the probe side's,
//! and the plan has nowhere to name a build-side column, so the hash table
//! holds, for each key, how many build rows carry it. That count is all the
//! join needs, and it is what makes the output's multiplicity right.
//!
//! ## NULLs
//!
//! A NULL key never matches, on either side. That is SQL's rule, and it is
//! the engine's rule everywhere else: the filter drops a NULL row too.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use columnar_core::Predicate;
use columnar_kernels::Flavor;

use crate::operators::filter::filter_all;
use crate::operators::key::{KeyColumn, KeyValue};
use crate::plan::{JoinSpec, ResolvedFilter};
use crate::pushdown::{Pushdown, PushdownPlan};
use crate::source::ParquetSource;
use crate::{operators, EngineError};

/// The hash the join's table uses.
///
/// `std`'s default hasher is `SipHash`, which is built to be resistant to
/// collision attacks -- a property a query engine's hash table has no use
/// for, and a cost it does: the probe is the join's hot loop, and it runs
/// once for every row of the probe side. Measured on the join query at
/// SF-1, the default hasher was the whole of the join's cost: the probe
/// ran at about 36 ns a row, and the same probe with this hash runs at
/// about a tenth of that.
///
/// This is `FxHash`'s arithmetic -- a rotate, an exclusive-or and a
/// multiply -- in twenty lines. The keys come from the dataset, not from an
/// adversary, and the table's equality check is exact either way: a hash
/// collision costs a comparison, never a wrong answer.
#[derive(Debug, Default, Clone, Copy)]
struct JoinHasher {
    /// The running hash.
    state: u64,
}

impl JoinHasher {
    /// The multiplier: the 64-bit golden ratio's odd companion.
    const SEED: u64 = 0x517c_c1b7_2722_0a95;
}

impl Hasher for JoinHasher {
    fn finish(&self) -> u64 {
        self.state
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for chunk in &mut chunks {
            let mut buffer = [0_u8; 8];
            buffer.copy_from_slice(chunk);
            self.write_u64(u64::from_le_bytes(buffer));
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut buffer = [0_u8; 8];
            buffer[..rest.len()].copy_from_slice(rest);
            self.write_u64(u64::from_le_bytes(buffer));
        }
    }

    fn write_u64(&mut self, value: u64) {
        self.state = (self.state.rotate_left(5) ^ value).wrapping_mul(Self::SEED);
    }
}

/// The join's table: the keys, and how many build rows carry each of them.
type JoinMap = HashMap<JoinKey, u32, BuildHasherDefault<JoinHasher>>;

/// A join key, in a form a hash table can hold.
///
/// The key is **owned**: it goes into the hash table, and the table outlives
/// the column the key was read out of.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum JoinKey {
    /// A 64-bit signed integer.
    Int(i64),
    /// A double, held by its bits.
    ///
    /// The bits are what the key is, so a NaN cannot be a key at all -- and
    /// the engine's rule is that a comparison involving a NaN never matches.
    /// `-0.0` is folded onto `0.0`, because they are the same number.
    Float(u64),
    /// A string.
    Text(String),
}

impl JoinKey {
    /// The key for a value, if the value can be one.
    ///
    /// A NaN is not a key: it never matches anything, so a row that holds
    /// one has no partner on either side.
    fn from_value(value: KeyValue<'_>) -> Option<Self> {
        match value {
            KeyValue::Int(value) => Some(Self::Int(value)),
            KeyValue::Float(value) => {
                if value.is_nan() {
                    return None;
                }
                // `-0.0` and `0.0` are the same number, so they are the same
                // key. The comparison is on the bits, not on the float, so
                // that the key never depends on how a float compares.
                let bits = if value.to_bits() == (-0.0f64).to_bits() {
                    0.0f64.to_bits()
                } else {
                    value.to_bits()
                };
                Some(Self::Float(bits))
            }
            KeyValue::Text(text) => Some(Self::Text(text.to_owned())),
        }
    }
}

/// The hash table the join probes.
///
/// The build side's rows are hashed on their key, and the table holds, for
/// each key, how many build rows carry it. The build side's columns are not
/// carried into the output, so the count is all the join needs: a probe row
/// with that key produces that many output rows.
#[derive(Debug, Default)]
pub struct BuildTable {
    /// The build rows each key accounts for.
    rows: JoinMap,
}

impl BuildTable {
    /// How many build rows carry `key`.
    fn matches(&self, key: &JoinKey) -> u32 {
        self.rows.get(key).copied().unwrap_or(0)
    }

    /// How many build rows the table holds.
    #[must_use]
    pub fn rows(&self) -> usize {
        self.rows.values().map(|&count| usize::try_from(count).unwrap_or(usize::MAX)).sum()
    }
}

/// Reads the build side and hashes it.
///
/// The build side is read through the engine's own source, so the reader
/// settings -- the batch size, the thread count, the kernel flavor -- are the
/// ones the rest of the engine uses. The build side's predicates are pushed
/// into the reader where the reader can evaluate them exactly, and applied
/// by the engine's own filter where it cannot; either way they are applied
/// **before** the table is built.
///
/// # Errors
///
/// Returns [`EngineError::UnknownColumn`] when the build side has no such
/// column, and [`EngineError::TypeMismatch`] when a predicate's threshold
/// does not have the column's type.
pub fn build_table(
    source: &ParquetSource,
    spec: &JoinSpec,
    flavor: Flavor,
    threads: usize,
) -> Result<BuildTable, EngineError> {
    let key_index = source
        .column_index(&spec.build_key)
        .ok_or_else(|| EngineError::UnknownColumn(spec.build_key.clone()))?;
    let filters = resolve_filters(source, &spec.build_filter)?;
    let (read, pushdown, declined) = decide_read(&filters, source, key_index);

    let dataset = source.read(&read, &pushdown, flavor, threads)?;

    // The predicates the reader could not take are applied here, over the
    // rows that survived the scan. The conjunction is separable, so the
    // answer is the same either way.
    //
    // The dataset holds the read set's columns, and the declined
    // predicates' columns are in it -- so their indices have to move from
    // the file's to the dataset's.
    let indices = dataset_indices(&read, source.columns().len());
    let declined: Vec<ResolvedFilter> = declined
        .iter()
        .map(|filter| ResolvedFilter {
            column: indices.get(filter.column).copied().flatten().unwrap_or(filter.column),
            predicate: filter.predicate.clone(),
        })
        .collect();
    let selection = if declined.is_empty() {
        operators::all_rows(dataset.len())
    } else {
        filter_all(&dataset, &declined, flavor)?.0
    };

    let name = source
        .columns()
        .get(key_index)
        .map_or_else(|| spec.build_key.clone(), |column| column.name.clone());
    let chunk = dataset.chunk(&name).ok_or_else(|| EngineError::UnknownColumn(name.clone()))?;
    let keys = KeyColumn::from_chunk(chunk);

    let mut table = BuildTable::default();
    selection.for_each(|row| {
        let row = usize::try_from(row).unwrap_or(usize::MAX);
        let Some(value) = keys.value_at(row) else {
            return;
        };
        let Some(key) = JoinKey::from_value(value) else {
            return;
        };
        table.rows.entry(key).and_modify(|count| *count = count.saturating_add(1)).or_insert(1);
    });
    Ok(table)
}

/// The rows the join produces, in the probe side's order.
///
/// `rows` are the probe side's surviving rows, as the dataset's own row
/// numbers. The result holds the **positions** of those rows among the
/// survivors -- the i-th survivor is position `i` -- so a row that matched
/// several build rows appears once per match.
///
/// A probe row whose key is NULL, or whose key is not in the table,
/// produces no output row at all.
#[must_use]
pub fn probe(table: &BuildTable, keys: &KeyColumn<'_>, rows: &[u32]) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::with_capacity(rows.len());
    for (index, &row) in rows.iter().enumerate() {
        let row = usize::try_from(row).unwrap_or(usize::MAX);
        let Some(value) = keys.value_at(row) else {
            continue;
        };
        let Some(key) = JoinKey::from_value(value) else {
            continue;
        };
        let matches = table.matches(&key);
        let position = u32::try_from(index).unwrap_or(u32::MAX);
        for _ in 0..matches {
            out.push(position);
        }
    }
    out
}

/// Resolves the build side's predicates, and type-checks their thresholds.
fn resolve_filters(
    source: &ParquetSource,
    predicates: &[Predicate],
) -> Result<Vec<ResolvedFilter>, EngineError> {
    predicates
        .iter()
        .map(|predicate| {
            let column = source
                .column_index(&predicate.column)
                .ok_or_else(|| EngineError::UnknownColumn(predicate.column.clone()))?;
            let column_type = source
                .columns()
                .get(column)
                .map_or(columnar_core::DataType::Int64, |column| column.data_type);
            let value_type = predicate.value.data_type();
            if column_type != value_type {
                return Err(EngineError::TypeMismatch(format!(
                    "the build side's column `{}` is {} but the predicate compares it against a {} threshold",
                    predicate.column,
                    column_type.name(),
                    value_type.name()
                )));
            }
            Ok(ResolvedFilter { column, predicate: predicate.clone() })
        })
        .collect()
}

/// Decides what the build side reads and what it pushes into the reader.
///
/// The rules are the scan's own: a predicate whose column is already being
/// read is declined, because the engine's filter would evaluate it again
/// over fewer rows; and a predicate the pushdown cannot evaluate **exactly**
/// is declined, because it is never approximated.
///
/// Returns the columns to read, the predicates to push into the reader, and
/// the predicates the engine's own filter has to apply.
fn decide_read(
    filters: &[ResolvedFilter],
    source: &ParquetSource,
    key: usize,
) -> (Vec<usize>, PushdownPlan, Vec<ResolvedFilter>) {
    let mut read = vec![key];
    let mut pushdown = PushdownPlan::new();
    let mut declined: Vec<ResolvedFilter> = Vec::new();

    for filter in filters {
        let Some(column) = source.columns().get(filter.column) else {
            continue;
        };
        if read.contains(&filter.column) || !crate::pushdown_supports(&column.arrow_type) {
            declined.push(filter.clone());
            continue;
        }
        // Two predicates on the same column are one conjunction over that
        // column, and the reader would decode the column once per predicate.
        match pushdown.for_column_mut(filter.column) {
            Some(existing) => {
                existing.and(filter.predicate.op, filter.predicate.value.clone());
            }
            None => {
                pushdown.push(Pushdown::new(
                    filter.column,
                    &column.name,
                    filter.predicate.op,
                    filter.predicate.value.clone(),
                ));
            }
        }
    }

    for filter in &declined {
        if !read.contains(&filter.column) {
            read.push(filter.column);
        }
    }
    read.sort_unstable();
    read.dedup();
    (read, pushdown, declined)
}

/// The dataset's own index for each of the file's columns.
///
/// The entry for a column that is not read is `None`.
fn dataset_indices(read: &[usize], columns: usize) -> Vec<Option<usize>> {
    (0..columns).map(|index| read.binary_search(&index).ok()).collect()
}
