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
//!
//! ## The table
//!
//! The table holds, for each key, how many build rows carry it. **How** it
//! holds them depends on the key:
//!
//! - An **integer** key whose range is narrow enough gets a **direct-addressed**
//!   table: the key *is* the index, the count is one byte, and a probe is a
//!   single load. There is no hash and no probe chain.
//! - Every other key gets an **open-addressed hash table** over the key's
//!   bits.
//!
//! The direct table is `O(range)` rather than `O(keys)`, so it is only used
//! when the range fits [`crate::memory::DIRECT_TABLE_LIMIT`]. The
//! measurement that chose this shape is in `crates/columnar-engine/examples/
//! operator_probe.rs`: over six million probe rows against a table of
//! 216,345 keys, the direct table ran at 16.4 ms and the hash table at
//! 66.2 ms, against 102.7 ms for the enum-keyed `HashMap` this replaced.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hash, Hasher};

use columnar_core::ScalarValue;
use columnar_kernels::Flavor;

use crate::dataset::DatasetError;
use crate::memory::direct_table_fits;
use crate::operators::filter::filter_all;
use crate::operators::key::{ArrowKeys, KeyColumn, KeySource, KeyValue};
use crate::plan::{JoinSpec, ResolvedFilter};
use crate::pushdown::RowMask;
use crate::source::{BatchFilter, ParquetSource};
use crate::EngineError;

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
#[derive(Debug, Clone, PartialEq, Eq)]
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

impl Hash for JoinKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // The payload, and only the payload. The derived hash would hash the
        // discriminant as well, and the discriminant is the same for every
        // key of a given type -- so it would be a second round of arithmetic
        // over the same bytes, once for every row of the probe side.
        match self {
            Self::Int(value) => state.write_i64(*value),
            Self::Float(bits) => state.write_u64(*bits),
            Self::Text(text) => state.write(text.as_bytes()),
        }
    }
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

    /// The key's integer, when it is one.
    fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            Self::Float(_) | Self::Text(_) => None,
        }
    }
}

/// The join, as a filter over the reader's batches.
///
/// The scan hands every batch to this **before** it copies anything, so the
/// rows the join does not match are never copied into the dataset. The probe
/// reads the key where it lies -- in the reader's own Arrow array -- and the
/// lookup is the same one the materialising path uses.
///
/// See [ADR-0030](../../../docs/adr/0030-the-scan-that-feeds-the-join.md).
#[derive(Debug)]
pub struct JoinFilter<'a> {
    /// The build side's table.
    table: &'a BuildTable,
    /// The position of the probe's key column in the reader's **projection**.
    key: usize,
}

impl<'a> JoinFilter<'a> {
    /// The filter for a join, over the reader's batches.
    ///
    /// `key` is the position of the probe's key column in the columns the
    /// reader hands over -- the projection -- and not the file's own index.
    pub fn new(table: &'a BuildTable, key: usize) -> Self {
        Self { table, key }
    }
}

impl BatchFilter for JoinFilter<'_> {
    fn keep(
        &self,
        batch: &arrow::record_batch::RecordBatch,
        survivors: Option<&RowMask>,
    ) -> Result<Vec<u32>, DatasetError> {
        let Some(array) = batch.columns().get(self.key) else {
            return Err(DatasetError::Parquet(
                "the record batch holds fewer columns than were projected".to_owned(),
            ));
        };
        let Some(keys) = ArrowKeys::new(array.as_ref()) else {
            return Err(DatasetError::Parquet(
                "the join's key column has a type the probe cannot read".to_owned(),
            ));
        };
        match survivors {
            Some(mask) => Ok(probe_rows(self.table, &keys, mask.positions())),
            None => Ok(probe_rows(
                self.table,
                &keys,
                0..u32::try_from(batch.num_rows()).unwrap_or(u32::MAX),
            )),
        }
    }
}

/// The table the join probes.
///
/// The build side's rows are hashed on their key, and the table holds, for
/// each key, how many build rows carry it. The build side's columns are not
/// carried into the output, so the count is all the join needs: a probe row
/// with that key produces that many output rows.
///
/// Which table it is depends on the key: an integer key in a narrow enough
/// range is addressed by the key itself, and everything else is hashed.
#[derive(Debug)]
pub struct BuildTable {
    /// How the table holds its keys.
    inner: TableInner,
}

/// How the table holds its keys.
#[derive(Debug)]
enum TableInner {
    /// A table addressed by the key itself.
    Direct(DirectTable),
    /// A hash table.
    Hashed(HashedTable),
}

impl BuildTable {
    /// How many build rows the table holds.
    #[must_use]
    pub fn rows(&self) -> usize {
        match &self.inner {
            TableInner::Direct(direct) => direct.rows,
            TableInner::Hashed(hashed) => hashed.rows,
        }
    }
}

/// A table addressed by the key itself.
///
/// The keys are integers and they lie in a range, and the range is narrow
/// enough that the table can be **one byte a key**: the byte says how many
/// build rows carry that key. A probe is then a single load -- there is no
/// hash to compute and no chain to walk, and the byte is a byte of the
/// table's own memory rather than a 32-byte key in a hash map.
///
/// The table is `O(range)` rather than `O(keys)`, so it is only used when the
/// range fits [`crate::memory::DIRECT_TABLE_LIMIT`]. A key that more than 255
/// build rows carry does not fit in a byte, and its count goes to a small
/// **overflow map** rather than abandoning the table: every entry there
/// accounts for at least 256 build rows, so there are few of them.
#[derive(Debug)]
struct DirectTable {
    /// The count a key accounts for, one byte a key.
    counts: Vec<u8>,
    /// The keys more than 255 build rows carry.
    ///
    /// A byte cannot hold more than 255, and a key that many build rows
    /// carry is **not** a reason to abandon the whole table: the count goes
    /// here instead, and the table keeps the shape that made the probe a
    /// single load. There are few enough of them to bound: every entry here
    /// accounts for at least 256 build rows.
    ///
    /// **`None` when no key overflowed.** The map is then not allocated at
    /// all, and the lookup never calls it -- no `get`, no hash of the key,
    /// and no read of any field of the map. The fact is the table's own
    /// state, and it is captured **after** the build loop, because the map
    /// fills *during* the build.
    overflow: Option<HashMap<i64, u32>>,
    /// The smallest key.
    base: i64,
    /// The build rows the table accounts for.
    rows: usize,
}

impl DirectTable {
    /// Builds the table over `keys`.
    ///
    /// The table's width comes from the **file's own statistics** for the key
    /// column, not from the keys themselves: the estimate has to know the
    /// table's size before a single row is read, so the two must be made from
    /// the same number or the estimate is not an upper bound.
    ///
    /// `None` when the range is too wide to be worth addressing directly, or
    /// when a key falls outside the range the statistics claim -- in which
    /// case the statistics are wrong and the hash table takes over.
    fn build(keys: &[i64], range: Option<&(ScalarValue, ScalarValue)>) -> Option<Self> {
        let (min, max) = range?;
        let (ScalarValue::Int64(min), ScalarValue::Int64(max)) = (min, max) else {
            return None;
        };
        if max < min {
            return None;
        }
        let span = i128::from(*max) - i128::from(*min) + 1;
        let span = u64::try_from(span).ok()?;
        if !direct_table_fits(span) {
            return None;
        }
        let width = usize::try_from(span).ok()?;
        let mut counts = vec![0_u8; width];
        let mut overflow: HashMap<i64, u32> = HashMap::new();
        for key in keys {
            let index = usize::try_from(i128::from(*key) - i128::from(*min)).ok()?;
            let count = counts.get_mut(index)?;
            if let Some(next) = count.checked_add(1) {
                *count = next;
            } else {
                // The byte is full. The rest of the count goes to the
                // overflow map, and the table keeps its shape.
                *count = u8::MAX;
                let entry = overflow.entry(*key).or_insert(0);
                *entry = entry.saturating_add(1);
            }
        }
        // The fact that the map holds something is captured **after** the
        // build loop, because the map fills *during* it. A fact read before
        // the loop is false for every table -- the map is empty until the
        // build fills it -- and a key with three hundred build rows would
        // count two hundred and fifty-five of them instead of three
        // hundred.
        let overflow = if overflow.is_empty() { None } else { Some(overflow) };
        Some(Self { counts, overflow, base: *min, rows: keys.len() })
    }

    /// How many build rows carry `key`.
    ///
    /// The overflow map is consulted **only when it holds something**. When
    /// it holds nothing, the lookup does not call it at all: the count is
    /// the byte, and the byte is the whole answer.
    fn count(&self, key: i64) -> u32 {
        let index = usize::try_from(key.saturating_sub(self.base)).unwrap_or(usize::MAX);
        let byte = self.counts.get(index).copied().map_or(0, u32::from);
        let Some(overflow) = &self.overflow else {
            return byte;
        };
        let extra = overflow.get(&key).copied().unwrap_or(0);
        byte.saturating_add(extra)
    }
}

/// A hash table over the keys.
///
/// The keys are the enum's own values, and the table is the `HashMap` the
/// engine has always used -- with one change: the key's hash is written by
/// hand, over the payload alone.
#[derive(Debug, Default)]
struct HashedTable {
    /// The build rows each key accounts for.
    counts: JoinMap,
    /// The build rows the table accounts for.
    rows: usize,
}

impl HashedTable {
    /// Adds a build row carrying `key`.
    fn add(&mut self, key: JoinKey) {
        self.counts.entry(key).and_modify(|count| *count = count.saturating_add(1)).or_insert(1);
        self.rows = self.rows.saturating_add(1);
    }

    /// How many build rows carry `key`.
    fn count(&self, key: &JoinKey) -> u32 {
        self.counts.get(key).copied().unwrap_or(0)
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
/// **The read is the reader's decision.** The build side asks the reader the
/// same question the probe side's scan asks it -- what to read, and what to
/// push down -- with its own key as its materialised set, so the two reads
/// cannot disagree about the rules.
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
    let filters = crate::plan::resolve_filters(source, &spec.build_filter)?;
    let decision = source.decide_read(&[key_index], &filters)?;

    let dataset = source.read(&decision.read, &decision.pushdown, flavor, threads)?;

    // The predicates the reader could not take are applied here, over the
    // rows that survived the scan. The conjunction is separable, so the
    // answer is the same either way -- and each of them is applied **once**,
    // by the filter, because the reader's decision said which ones it did
    // not take.
    //
    // The dataset holds the read set's columns, and the declined
    // predicates' columns are in it -- so their indices have to move from
    // the file's to the dataset's. The mapping is the plan's.
    let mapping = decision.mapping(source.columns().len());
    let declined: Vec<ResolvedFilter> = decision
        .declined
        .iter()
        .map(|filter| {
            Ok(ResolvedFilter {
                column: mapping.require(filter.column)?,
                predicate: filter.predicate.clone(),
            })
        })
        .collect::<Result<Vec<ResolvedFilter>, EngineError>>()?;
    let selection = if declined.is_empty() {
        columnar_core::SelectionVector::all(dataset.len())
    } else {
        filter_all(&dataset, &declined, flavor)?.0
    };

    let name = source
        .columns()
        .get(key_index)
        .map_or_else(|| spec.build_key.clone(), |column| column.name.clone());
    let chunk = dataset.chunk(&name).ok_or_else(|| EngineError::UnknownColumn(name.clone()))?;
    let keys = KeyColumn::from_chunk(chunk);

    // The build side's keys. An integer key can be addressed by the key
    // itself, so it is collected on its own and the table is built from it.
    let mut integers: Vec<i64> = Vec::new();
    let mut general: Vec<JoinKey> = Vec::new();
    selection.for_each(|row| {
        let row = usize::try_from(row).unwrap_or(usize::MAX);
        let Some(value) = keys.value_at(row) else {
            return;
        };
        match value {
            KeyValue::Int(value) => integers.push(value),
            other => {
                if let Some(key) = JoinKey::from_value(other) {
                    general.push(key);
                }
            }
        }
    });

    if general.is_empty() {
        let range = source.column_range(&spec.build_key);
        if let Some(direct) = DirectTable::build(&integers, range.as_ref()) {
            return Ok(BuildTable { inner: TableInner::Direct(direct) });
        }
        let mut hashed = HashedTable::default();
        for key in integers {
            hashed.add(JoinKey::Int(key));
        }
        return Ok(BuildTable { inner: TableInner::Hashed(hashed) });
    }

    let mut hashed = HashedTable::default();
    // The integers go in too, even though a column of one type gives one kind
    // of key: a table that silently dropped a key would answer wrongly, and
    // the cost of the insurance is one loop over a vector that is empty in
    // every case but the mixed one.
    for key in integers {
        hashed.add(JoinKey::Int(key));
    }
    for key in general {
        hashed.add(key);
    }
    Ok(BuildTable { inner: TableInner::Hashed(hashed) })
}

/// The rows the join produces, in the probe side's order.
///
/// `rows` are the probe side's surviving rows, as the dataset's own row
/// numbers. The result holds those same row numbers, so a row that matched
/// several build rows appears once per match, and a stage behind the join
/// reads them without translating them.
///
/// A probe row whose key is NULL, or whose key is not in the table,
/// produces no output row at all.
#[must_use]
pub fn probe<S>(table: &BuildTable, keys: &S, rows: &[u32]) -> Vec<u32>
where
    S: KeySource + ?Sized,
{
    probe_rows(table, keys, rows.iter().copied())
}

/// Probes the table over rows taken one at a time.
///
/// The rows are an **iterator**, not a list: the scan that feeds the join
/// probes a whole batch, and a list of a batch's row numbers is a quarter of
/// a megabyte of memory the allocator has to hand out and take back, batch
/// after batch, for rows the probe reads once.
pub fn probe_rows<S, I>(table: &BuildTable, keys: &S, rows: I) -> Vec<u32>
where
    S: KeySource + ?Sized,
    I: IntoIterator<Item = u32>,
{
    match &table.inner {
        TableInner::Direct(direct) => probe_direct(direct, keys, rows),
        TableInner::Hashed(hashed) => probe_hashed(hashed, keys, rows),
    }
}

/// The rows that matched, when the table is addressed by the key itself.
///
/// The fast path reads the key column's own integers: no enum, no validity
/// bitmap, and no hash. A column that holds NULLs, or that is
/// dictionary-encoded, takes the general path instead.
fn probe_direct<S, I>(direct: &DirectTable, keys: &S, rows: I) -> Vec<u32>
where
    S: KeySource + ?Sized,
    I: IntoIterator<Item = u32>,
{
    let mut out: Vec<u32> = Vec::new();
    if let Some(values) = keys.integer_values() {
        if !keys.has_nulls() {
            for row in rows {
                let index = usize::try_from(row).unwrap_or(usize::MAX);
                let Some(&key) = values.get(index) else {
                    continue;
                };
                let count = direct.count(key);
                for _ in 0..count {
                    out.push(row);
                }
            }
            return out;
        }
    }
    for row in rows {
        let index = usize::try_from(row).unwrap_or(usize::MAX);
        let Some(value) = keys.value_at(index) else {
            continue;
        };
        let Some(key) = JoinKey::from_value(value).and_then(|key| key.as_int()) else {
            continue;
        };
        let count = direct.count(key);
        for _ in 0..count {
            out.push(row);
        }
    }
    out
}

/// The rows that matched, when the table is hashed.
fn probe_hashed<S, I>(hashed: &HashedTable, keys: &S, rows: I) -> Vec<u32>
where
    S: KeySource + ?Sized,
    I: IntoIterator<Item = u32>,
{
    let mut out: Vec<u32> = Vec::new();
    for row in rows {
        let index = usize::try_from(row).unwrap_or(usize::MAX);
        let Some(value) = keys.value_at(index) else {
            continue;
        };
        let Some(key) = JoinKey::from_value(value) else {
            continue;
        };
        let count = hashed.count(&key);
        for _ in 0..count {
            out.push(row);
        }
    }
    out
}

/// The tables' own tests.
///
/// The two tables are **private to this module**, so the comparison between
/// them is a unit test here and not a test of the public API: the public
/// API picks the table from the file's own statistics, and it cannot build
/// the same keys both ways.
#[cfg(test)]
mod tests {
    use super::{DirectTable, HashedTable, JoinKey};
    use columnar_core::ScalarValue;

    /// A direct table over `keys`, addressed by the key itself.
    fn direct(keys: &[i64], min: i64, max: i64) -> DirectTable {
        let range = (ScalarValue::Int64(min), ScalarValue::Int64(max));
        DirectTable::build(keys, Some(&range)).expect("the range fits a direct table")
    }

    /// A hash table over the same keys.
    fn hashed(keys: &[i64]) -> HashedTable {
        let mut table = HashedTable::default();
        for key in keys {
            table.add(JoinKey::Int(*key));
        }
        table
    }

    /// The direct table's answer is the hash table's answer, key by key.
    ///
    /// The keys are the interesting ones: one that carries more rows than a
    /// byte holds, one that carries a few, and one that carries none at all
    /// -- and a key below the table's base, which the direct table must
    /// refuse as well as the hash table.
    #[test]
    fn the_direct_table_agrees_with_the_hash_table() {
        let mut keys = Vec::new();
        keys.extend(std::iter::repeat_n(11_i64, 300));
        keys.extend(std::iter::repeat_n(12_i64, 3));
        keys.push(13);
        let direct = direct(&keys, 11, 13);
        let hashed = hashed(&keys);
        for key in 9..=15 {
            assert_eq!(
                direct.count(key),
                hashed.count(&JoinKey::Int(key)),
                "the key {key} counts the same in both tables"
            );
        }
    }

    /// The overflow map is there only when a key overflowed its byte.
    ///
    /// And the fact is captured **after** the build loop: a fact read before
    /// it would be false for every table, and a key that overflowed would
    /// count 255 times instead of 300.
    #[test]
    fn the_overflow_map_is_allocated_only_when_a_key_overflows() {
        let small = direct(&[11, 12, 13], 11, 13);
        assert!(small.overflow.is_none(), "no key overflowed, so there is no map");

        let mut keys = Vec::new();
        keys.extend(std::iter::repeat_n(11_i64, 300));
        let large = direct(&keys, 11, 11);
        assert!(large.overflow.is_some(), "a key overflowed, so there is a map");
        assert_eq!(large.count(11), 300, "every one of the 300 rows is counted");
    }
}
