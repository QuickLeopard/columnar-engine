//! The query memory budget.
//!
//! The engine estimates the working set of a plan before it runs it, and
//! refuses the plan if the estimate does not fit the budget. The check is a
//! *pre-flight* check, not a mechanism: a plan that does not fit is
//! rejected, not spilled. Spilling is explicitly out of scope for this
//! milestone (see the crate documentation).
//!
//! ## What the estimate counts
//!
//! | Component | Bytes |
//! |---|---|
//! | a materialised column | `rows * (value width + 1 validity bit)` |
//! | the filter's match mask | `rows / 8` |
//! | the selection vector | `rows / 8` |
//! | the aggregate hash table | `groups * 16` |
//! | the row positions | `rows * 4`, where a join or a sort is present |
//! | the join's build side | its dataset and the larger of its two tables |
//!
//! The aggregate hash table holds one entry per **group**, and the number of
//! groups cannot be known before the query runs. A **grouped** aggregate can
//! have as many groups as it has rows, so the row count bounds it. A
//! **global** aggregate has exactly one group, and charging it an entry per
//! row overstated the working set six million times -- 91.6 MiB of phantom
//! on every eager plan.
//!
//! The value width is 8 bytes for `int64` and `float64`, and 24 bytes
//! -- the size of a `String` header -- for `utf8`; the bytes a string's heap
//! holds are not counted, because they are the data's own size rather than
//! the working set's.
//!
//! ## The join's build side
//!
//! The build side is charged for what it really costs, and the file's own
//! statistics are what say so:
//!
//! - The **dataset** the build side's rows are read into is the **file's rows**
//!   -- the peak is during the read, when every row of the file is still in
//!   flight -- over every column the read takes, times the **copies** the read
//!   holds at once. The read set is the **reader's own decision**, so the
//!   estimate asks the reader what the build side's read takes and charges
//!   exactly the columns the join reads: the key, and every column of a
//!   predicate the reader declined.
//! - The **keys** the join collects out of that dataset, and the **filter's**
//!   mask and selection when the reader declined a predicate. The rows that
//!   can survive the build filter are bounded from the filter column's
//!   **row-group statistics**: a row group whose own range cannot satisfy a
//!   predicate holds no row that can, so its rows are not counted.
//! - The **table** the join builds. The estimate mirrors the join's own
//!   decision, because the join makes that decision from the same numbers: a
//!   direct-addressed table over the key's own range when the key is an
//!   integer and the range fits, and a hash table over the surviving rows
//!   otherwise. The direct table's **overflow map** -- the keys more than 255
//!   build rows carry -- is charged too: every entry in it accounts for at
//!   least 256 build rows, so there are at most one in every 256.
//!
//! The one case the estimate cannot cover is a file whose own statistics are
//! wrong: the join notices, and falls back to the hash table, which the
//! estimate did not charge for. The answer stays right; the memory may be
//! higher than the estimate said. A file that lies about its own contents is
//! outside the reach of any pre-flight estimate.
//!
//! See [`estimate_build_side`].
//!
//! ## The estimate is an upper bound, not a prediction
//!
//! It is meant to fail early and loudly, not to be accurate to the byte.
//!
//! ## The lazy path's estimate is a *pre-flight* one
//!
//! The eager path's dataset is already in memory when the plan runs, so its
//! estimate is made over the dataset. The lazy path's dataset does not exist
//! yet, so its estimate is made over the **file**: its schema and its row
//! count. That estimate is computed and checked *before* the read, so a plan
//! that does not fit the budget is refused having read nothing at all.
//!
//! The estimate also accounts for the **read's own peak**, which is where the
//! thread count shows up. A single-threaded read decodes straight into one
//! dataset and holds one copy of it. A parallel read holds every thread's part
//! at once -- which is one copy between them -- and then holds the merged
//! result as well, so it holds **two**. See [`estimate_lazy_read`].

use columnar_core::{ComparisonOp, DataType, Encoding, Predicate, ScalarValue};

use crate::plan::{QueryPlan, ResolvedJoin, ResolvedPlan, ResolvedSort};
use crate::source::ParquetSource;
use crate::Dataset;

/// The default memory budget: 256 MiB.
const DEFAULT_BUDGET_BYTES: usize = 256 * 1024 * 1024;

/// The estimated bytes per aggregate hash-table entry.
const GROUP_ENTRY_BYTES: usize = 16;

/// The estimated bytes per row of a build-side hash table.
///
/// The table holds the key and the number of build rows that carry it, and it
/// is open-addressed, so a row costs a key, a count and a probe slot.
const HASH_ENTRY_BYTES: usize = 48;

/// The bytes a row position takes: a `u32`.
const POSITION_BYTES: usize = 4;

/// The width of a `String` header, the value width of a `utf8` column.
const STRING_HEADER_BYTES: usize = 24;

/// The largest direct-addressed table the engine will build, in bytes.
///
/// A direct-addressed table is addressed by the key itself, and holds one
/// byte a key, so a key range wider than this is not worth building: the
/// table would cost more than the hash table it replaces.
pub const DIRECT_TABLE_LIMIT: usize = 32 * 1024 * 1024;

/// The per-query memory budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryBudget {
    /// The budget, in bytes.
    pub bytes: usize,
}

impl Default for MemoryBudget {
    fn default() -> Self {
        Self { bytes: DEFAULT_BUDGET_BYTES }
    }
}

/// Why a plan did not fit its memory budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MemoryError {
    /// The plan's estimated working set does not fit the budget.
    ///
    /// The plan is rejected before any work is done, so a plan that fails
    /// this check has read no rows.
    #[error("the estimated working set exceeds the query memory budget")]
    BudgetExceeded,
}

/// Estimates the working set of `plan` over `dataset`, in bytes.
///
/// A plan that does not resolve (an unknown column, say) has no working set
/// to estimate, and the estimate is zero: [`crate::execute`] reports the
/// resolution error long before it looks at the budget.
///
/// The plan knows whether its aggregation is grouped, so the estimate does
/// not have to be told.
#[must_use]
pub fn estimate_bytes(plan: &QueryPlan, dataset: &Dataset) -> usize {
    match plan.resolve(dataset) {
        Ok(resolved) => estimate_resolved(&resolved, dataset, is_grouped(&resolved)),
        Err(_) => 0,
    }
}

/// Estimates the working set of an already-resolved plan, in bytes.
///
/// `grouped` says whether the plan's aggregation has group-by columns: a
/// **grouped** aggregate can have as many groups as it has rows, and a
/// **global** one has exactly one.
#[must_use]
pub fn estimate_resolved(resolved: &ResolvedPlan, dataset: &Dataset, grouped: bool) -> usize {
    estimate_resolved_at(resolved, dataset, dataset.len(), grouped)
}

/// Estimates the working set of an already-resolved plan over `rows` rows.
///
/// The row count is a parameter because the two paths have different peaks.
/// The eager path's peak is the decoded dataset, which holds `dataset.len()`
/// rows. The lazy path's peak is *during* the read, when every row of the
/// file is still in flight even though only some of them survive the
/// pushed-down predicates -- so the lazy path estimates against the file's
/// row count, which is the conservative one.
///
/// `grouped` says whether the plan's aggregation has group-by columns. The
/// aggregate hash table holds one entry a **group**: a grouped aggregate can
/// have as many groups as it has rows, so the rows bound it, and a global
/// one has exactly one group, so it costs one entry whatever the row count
/// is.
#[must_use]
pub fn estimate_resolved_at(
    resolved: &ResolvedPlan,
    dataset: &Dataset,
    rows: usize,
    grouped: bool,
) -> usize {
    if rows == 0 {
        return 0;
    }

    let mut bytes = 0usize;
    for &index in &resolved.materialised {
        let chunk = dataset.chunks.get(index).expect("a materialised column index is in range");
        bytes = bytes.saturating_add(column_bytes(chunk, rows));
    }

    // The filter's match mask and the selection vector it becomes.
    bytes = bytes.saturating_add(rows.div_ceil(8).saturating_mul(2));

    // The aggregate hash table: one entry per group, bounded by the rows.
    bytes = bytes.saturating_add(group_bytes(resolved, rows, grouped));

    // The row positions the join and the sort carry.
    if resolved.join.is_some() || resolved.sort.is_some() {
        bytes = bytes.saturating_add(estimate_positions(rows));
    }

    bytes
}

/// The bytes the row-position vectors hold.
///
/// The join and the sort carry a `u32` a row: the sort's input, and the
/// join's output. They are memory the engine holds, so they are counted.
#[must_use]
pub fn estimate_positions(rows: usize) -> usize {
    rows.saturating_mul(POSITION_BYTES)
}

/// The bytes the aggregate's hash table takes.
///
/// The table holds one entry a group. A plan with no aggregation has no
/// groups at all; a **global** aggregate has exactly one; a **grouped** one
/// can have as many groups as it has rows.
///
/// The flag can only make the estimate larger: a plan that groups by a
/// column is charged for its groups whether the flag says so or not.
fn group_bytes(resolved: &ResolvedPlan, rows: usize, grouped: bool) -> usize {
    let groups = match &resolved.aggregate {
        None => 0,
        Some(aggregate) => {
            if grouped || !aggregate.group_by.is_empty() {
                rows
            } else {
                1
            }
        }
    };
    groups.saturating_mul(GROUP_ENTRY_BYTES)
}

/// Whether a plan's aggregation is grouped.
fn is_grouped(resolved: &ResolvedPlan) -> bool {
    resolved.aggregate.as_ref().is_some_and(|aggregate| !aggregate.group_by.is_empty())
}

/// The bytes a materialised column costs.
fn column_bytes(chunk: &columnar_core::ColumnChunk, rows: usize) -> usize {
    let value_width = match &chunk.encoding {
        Encoding::Int64 { .. }
        | Encoding::Float64 { .. }
        | Encoding::Int64Dict { .. }
        | Encoding::Float64Dict { .. }
        | Encoding::BoolRle { .. } => 8,
        Encoding::Utf8 { .. } | Encoding::Utf8Dict { .. } => STRING_HEADER_BYTES,
    };
    let validity = rows.div_ceil(8);
    rows.saturating_mul(value_width).saturating_add(validity)
}

/// Estimates the working set of a **lazy** read of `source`, in bytes.
///
/// This is the pre-flight estimate: it is made from the file's schema and its
/// row count, before a single row is read, so a plan that does not fit the
/// budget is refused having done no work.
///
/// `read` names the file's root columns the read will fetch -- the plan's
/// materialised columns **and** the columns the pushed-down predicates read,
/// because the reader fetches those too. The dataset the read produces holds
/// exactly those columns.
///
/// `threads` is the read's thread count, and it decides the **peak**:
///
/// - **One thread.** The reader decodes straight into one dataset, so the
///   read holds one copy of the columns.
/// - **More than one.** Every thread's part is alive at once -- one copy
///   between them -- and the merge then holds the merged result as well, so
///   the read holds **two** copies. That second copy is the price of the
///   parallel path, and a budget that did not account for it would let a
///   query through that the machine cannot hold.
///
/// The row count used is the **file's**, not the number of rows that survive
/// the pushed-down predicates, because the peak is during the read: every row
/// of the file is still in flight then. The estimate is an upper bound.
#[must_use]
pub fn estimate_lazy_read(
    source: &ParquetSource,
    read: &[usize],
    threads: usize,
    grouped: bool,
) -> usize {
    let rows = source.rows();
    if rows == 0 {
        return 0;
    }

    let mut dataset = 0usize;
    for &index in read {
        let width = source.columns().get(index).map_or(8, |column| type_width(column.data_type));
        dataset =
            dataset.saturating_add(rows.saturating_mul(width).saturating_add(rows.div_ceil(8)));
    }

    let mut bytes = dataset.saturating_mul(read_copies(threads));

    // The filter's match mask and the selection vector it becomes.
    bytes = bytes.saturating_add(rows.div_ceil(8).saturating_mul(2));

    // The aggregate hash table: one entry per group. A **grouped** aggregate
    // can have as many groups as it has rows, so the rows bound it; a
    // **global** aggregate has exactly one, and counting one entry per row
    // would overstate the working set by six million times.
    let groups = if grouped { rows } else { 1 };
    bytes = bytes.saturating_add(groups.saturating_mul(GROUP_ENTRY_BYTES));

    bytes
}

/// How many copies of the read set the read holds at once.
///
/// One thread decodes into one dataset and holds one copy. A parallel read
/// holds every part at once and then the merged result, so it holds two.
fn read_copies(threads: usize) -> usize {
    if threads <= 1 {
        1
    } else {
        2
    }
}

/// The width of one value of a column of the given type.
///
/// A dictionary-encoded string column is a `utf8` column to the engine, and
/// costs the same as one: the dictionary is shared by the whole column, and
/// the per-row cost is the index.
fn type_width(data_type: DataType) -> usize {
    match data_type {
        DataType::Int64 | DataType::Float64 => 8,
        DataType::Utf8 => STRING_HEADER_BYTES,
    }
}

/// Estimates the working set of a join's **build side**, in bytes.
///
/// The build side is read, filtered, and turned into a table. The estimate is
/// made over the **file**, before a single row of it is read, and it has
/// three parts:
///
/// - The **dataset** the read holds: the **file's rows**, over every column
///   the read takes, and over the **copies** the read holds at once.
/// - The **keys** the join collects out of that dataset, and the **filter's**
///   mask and selection when the reader declined a predicate.
/// - The **table** the join builds over the rows that survive: the
///   direct-addressed one when the key is an integer and its range fits, and
///   the hash table otherwise.
///
/// The **read set is the reader's own decision**: the estimate asks the reader
/// what the build side's read takes, so it charges exactly the columns the
/// join reads -- the key, and every column of a predicate the reader
/// declined -- rather than a copy of the rules that guesses at them.
///
/// The rows charged are the **file's**, not the rows that survive the build
/// filter, because the peak is during the read: every row of the file is
/// still in flight then. The estimate is an upper bound.
///
/// `threads` is the read's thread count, and it decides the **peak**: one
/// thread decodes straight into one dataset and holds one copy of it, and a
/// parallel read holds every part at once and then the merged result, so it
/// holds **two**.
#[must_use]
pub fn estimate_build_side(
    source: &ParquetSource,
    build_key: &str,
    build_filter: &[Predicate],
    threads: usize,
) -> usize {
    let rows = source.rows();
    if rows == 0 {
        return 0;
    }

    let read = build_read(source, build_key, build_filter);
    let survivors = surviving_rows(source, build_filter, rows);

    // The dataset the read holds: the file's rows, over every column of the
    // read set, and over the copies the read holds at once.
    let mut dataset = 0usize;
    for &index in &read.columns {
        let width = source.columns().get(index).map_or(8, |column| type_width(column.data_type));
        dataset =
            dataset.saturating_add(rows.saturating_mul(width).saturating_add(rows.div_ceil(8)));
    }
    let workers = source.worker_count(threads);
    let mut bytes = dataset.saturating_mul(read_copies(workers));

    // The keys the join collects out of the dataset, and the filter's mask
    // and selection when the engine's own filter has to apply a predicate.
    bytes = bytes.saturating_add(survivors.saturating_mul(key_width(source, build_key)));
    if read.declined {
        bytes = bytes.saturating_add(rows.div_ceil(8).saturating_mul(2));
    }

    bytes.saturating_add(build_table_bytes(source, build_key, survivors))
}

/// What the build side's read takes, and what the engine's filter has to do.
struct BuildRead {
    /// The file's root indices the read takes.
    columns: Vec<usize>,
    /// Whether the reader declined a predicate, so the engine's filter runs.
    declined: bool,
}

/// The build side's read, as the **reader** decides it.
///
/// The estimate asks the reader the same question the join asks, so the two
/// cannot disagree about what is read. A decision that cannot be made -- a
/// key the file does not have, or a predicate whose column it does not have
/// -- falls back to the key alone: the plan is refused before the read
/// anyway, and refusing a plan is not the estimate's job.
fn build_read(source: &ParquetSource, build_key: &str, build_filter: &[Predicate]) -> BuildRead {
    let Some(key) = source.column_index(build_key) else {
        return BuildRead { columns: Vec::new(), declined: false };
    };
    let Ok(filters) = crate::plan::resolve_filters(source, build_filter) else {
        return BuildRead { columns: vec![key], declined: false };
    };
    match source.decide_read(&[key], &filters) {
        Ok(decision) => {
            BuildRead { columns: decision.read, declined: !decision.declined.is_empty() }
        }
        Err(_) => BuildRead { columns: vec![key], declined: false },
    }
}

/// The bytes one collected key takes.
///
/// An **integer** key is collected into a vector of `i64`s: eight bytes a key.
/// Every other key is collected into a vector of the join's own key type,
/// which holds a string header beside the value: thirty-two bytes a key.
fn key_width(source: &ParquetSource, build_key: &str) -> usize {
    let data_type = source
        .columns()
        .iter()
        .find(|column| column.name == build_key)
        .map(|column| column.data_type);
    match data_type {
        Some(DataType::Float64 | DataType::Utf8) => 32,
        Some(DataType::Int64) | None => 8,
    }
}

/// The bytes the table the join builds over its build side takes.
///
/// The estimate **mirrors the join's own decision**, because the join makes
/// that decision from the same numbers: a **direct-addressed** table over the
/// build key's own range when the key is an integer and the range fits, and a
/// **hash table** over the rows that survive the filter otherwise.
///
/// There are two cases in which the join abandons the direct table it built
/// and hashes instead: a key outside the range the file's statistics claim
/// (which means the file's statistics are wrong), and a key more than 255
/// build rows carry (which means the key has very few distinct values, and
/// the hash table over those is smaller than the direct table, not larger).
/// Neither is charged for here, and both are named in the join's own code.
fn build_table_bytes(source: &ParquetSource, build_key: &str, survivors: usize) -> usize {
    match direct_table_bytes(source, build_key) {
        Some(bytes) => bytes.saturating_add(overflow_map_bytes(survivors)),
        None => survivors.saturating_mul(HASH_ENTRY_BYTES),
    }
}

/// The bytes the direct table's overflow map takes.
///
/// The map holds the keys more than 255 build rows carry, and every entry
/// accounts for at least 256 build rows, so there are at most one in every
/// 256 of the rows that survive.
fn overflow_map_bytes(survivors: usize) -> usize {
    survivors.div_ceil(256).saturating_mul(HASH_ENTRY_BYTES)
}

/// The bytes a direct-addressed table over the build key takes.
///
/// The table is addressed by the key itself and holds one byte a key, so its
/// size is the key's range. The range comes from the key column's own
/// statistics. A key that is not an integer, or whose range does not fit
/// [`DIRECT_TABLE_LIMIT`], is not built as a direct-addressed table at all,
/// and the table is then the hash table's.
fn direct_table_bytes(source: &ParquetSource, build_key: &str) -> Option<usize> {
    let (min, max) = source.column_range(build_key)?;
    let (ScalarValue::Int64(min), ScalarValue::Int64(max)) = (min, max) else {
        return None;
    };
    if max < min {
        return None;
    }
    let span = i128::from(max) - i128::from(min) + 1;
    let Ok(span) = u64::try_from(span) else {
        return None;
    };
    if !direct_table_fits(span) {
        return None;
    }
    Some(usize::try_from(span).unwrap_or(usize::MAX))
}

/// Whether a key range fits a direct-addressed table.
///
/// The range is the number of keys the table would address, and the table
/// holds one byte a key.
#[must_use]
pub fn direct_table_fits(range: u64) -> bool {
    range <= u64::try_from(DIRECT_TABLE_LIMIT).unwrap_or(u64::MAX)
}

/// The rows that can survive the build filter.
///
/// The bound is made over the file's own **row-group statistics**, so it is
/// known before a single row is read, and it is a true bound rather than a
/// guess: a row group whose own range cannot satisfy a predicate holds no row
/// that can satisfy it, and its rows are not counted. The columns of the
/// filter are combined by taking the tightest of them.
///
/// A filter column with no statistics, or a predicate the statistics cannot
/// be compared against, gives no bound, and the rows of the whole file are
/// used for that column.
fn surviving_rows(source: &ParquetSource, build_filter: &[Predicate], rows: usize) -> usize {
    let mut bound = rows;
    for column in distinct_columns(build_filter) {
        let Some(column_bound) = column_bound(source, column, build_filter) else {
            continue;
        };
        bound = bound.min(column_bound);
    }
    bound
}

/// The columns the filter names, in the order it names them.
fn distinct_columns(filter: &[Predicate]) -> Vec<&str> {
    let mut columns: Vec<&str> = Vec::new();
    for predicate in filter {
        if !columns.contains(&predicate.column.as_str()) {
            columns.push(predicate.column.as_str());
        }
    }
    columns
}

/// The rows a filter column's statistics allow through.
///
/// A row group is counted only when **every** predicate on the column could
/// be satisfied by some value inside the row group's own range. A row group
/// with no statistics for the column cannot be bounded, so all of its rows
/// are counted.
fn column_bound(source: &ParquetSource, column: &str, filter: &[Predicate]) -> Option<usize> {
    let groups = source.row_group_ranges(column);
    if groups.is_empty() {
        return None;
    }
    let predicates: Vec<&Predicate> =
        filter.iter().filter(|predicate| predicate.column == column).collect();
    if predicates.is_empty() {
        return None;
    }
    let mut bound = 0_usize;
    for group in groups {
        let counted = match &group.range {
            None => true,
            Some((min, max)) => {
                predicates.iter().all(|predicate| group_may_satisfy(predicate, min, max))
            }
        };
        if counted {
            bound = bound.saturating_add(group.rows);
        }
    }
    Some(bound)
}

/// Whether some value in a row group's range could satisfy a predicate.
///
/// The test is on the row group's own **minimum and maximum**, so it is a
/// bound and not a prediction: a row group whose values are all below the
/// predicate's bound cannot hold a row that satisfies it.
fn group_may_satisfy(predicate: &Predicate, min: &ScalarValue, max: &ScalarValue) -> bool {
    let Some(value) = coordinate(&predicate.value) else {
        // The predicate's value has no place on the column's line, so the
        // statistics cannot bound it. Every row group is counted.
        return true;
    };
    let (Some(min), Some(max)) = (coordinate(min), coordinate(max)) else {
        return true;
    };
    if min > max {
        return true;
    }
    match predicate.op {
        ComparisonOp::GtEq => max >= value,
        ComparisonOp::Gt => max > value,
        ComparisonOp::LtEq => min <= value,
        ComparisonOp::Lt => min < value,
        ComparisonOp::Eq => min <= value && max >= value,
        ComparisonOp::Ne => true,
    }
}

/// A value's place on its column's line.
///
/// The engine's values are integers or dates, so a coordinate is a number of
/// units or a number of days. A value that has no coordinate -- a float, or
/// a string that is not a date -- has none, and the column it belongs to
/// cannot be bounded by its statistics.
fn coordinate(value: &ScalarValue) -> Option<i128> {
    match value {
        ScalarValue::Int64(value) => Some(i128::from(*value)),
        ScalarValue::Float64(_) => None,
        ScalarValue::Utf8(text) => iso_days(text).map(i128::from),
    }
}

/// The day number of an ISO-8601 date, if the text is one.
///
/// The engine models strings, not dates, so the dates in a TPC-H-shaped file
/// are text. A text date has a place on a line -- the days between it and
/// the epoch -- and that is what lets the engine bound a range over it.
fn iso_days(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year: i64 = text.get(0..4)?.parse().ok()?;
    let month: i64 = text.get(5..7)?.parse().ok()?;
    let day: i64 = text.get(8..10)?.parse().ok()?;
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    Some(days_from_civil(year, month, day))
}

/// How many days a month holds.
///
/// A date the month cannot hold has no place on the line: the calendar says
/// there is no such day, and a bound computed from one would be a bound
/// against a day that does not exist.
fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Whether a year has a 29th of February.
fn is_leap_year(year: i64) -> bool {
    year.rem_euclid(4) == 0 && (year.rem_euclid(100) != 0 || year.rem_euclid(400) == 0)
}

/// The days between the epoch and a date in the proleptic Gregorian calendar.
///
/// The days are counted from 1970-01-01, which is the epoch the engine's
/// dates are measured against.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    // The year the day belongs to: January and February belong to the year
    // before, when the era is counted from March.
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_of_year = (month + 9) % 12;
    let day_of_year = (153 * month_of_year + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Estimates the working set of a plan's **join**, in bytes.
///
/// The join's build side is hashed, and the hash table holds one entry per
/// distinct key, so the build side's row count bounds it. A plan with no
/// join has no build side, and costs nothing here.
///
/// `threads` is the read's thread count: the build side's read holds one copy
/// of what it reads when it is single-threaded, and two when it is parallel.
#[must_use]
pub fn estimate_plan_join(
    join: Option<&ResolvedJoin>,
    source: Option<&ParquetSource>,
    threads: usize,
) -> usize {
    let (Some(join), Some(source)) = (join, source) else {
        return 0;
    };
    estimate_build_side(source, &join.spec.build_key, &join.spec.build_filter, threads)
}

/// Estimates the working set of a plan's **sort**, in bytes.
///
/// With a limit the sort keeps a bounded heap of `limit` rows; without one it
/// sorts every row, and the rows it sorts are its working set. A plan with no
/// sort has no working set of its own, and costs nothing here.
#[must_use]
pub fn estimate_plan_sort(sort: Option<&ResolvedSort>, rows: usize) -> usize {
    match sort {
        Some(sort) => estimate_sort(rows, sort.spec.limit),
        None => 0,
    }
}

/// Estimates the working set of a **sort**, in bytes.
///
/// With a limit the sort keeps a bounded heap of `limit` rows, so the heap
/// is its working set however many rows it is handed. Without one it sorts
/// every row, and the rows it sorts are its working set.
#[must_use]
pub fn estimate_sort(rows: usize, limit: Option<usize>) -> usize {
    let rows = match limit {
        Some(limit) => limit.min(rows),
        None => rows,
    };
    rows.saturating_mul(SORT_ENTRY_BYTES)
}

/// The estimated bytes per row the sort holds.
const SORT_ENTRY_BYTES: usize = 16;
