//! The rows that change: a row's identity, its tombstone, and the live rows.
//!
//! **Every row the database writes carries two hidden columns**: a `row_id`,
//! which is the version's own name, and a `deleted` flag, which says whether
//! the row is a tombstone. **The application names neither.** A schema may not
//! use either name, and a plan that names one is refused -- so the row's
//! identity is the database's bookkeeping and the application's rows are the
//! values and not the bookkeeping.
//!
//! **And a row that changes does not change in place.** The files are
//! append-only, so an update cannot rewrite the version it replaces: the file
//! that holds the old version stays as it is, and a reader that pinned it
//! still sees it. **So the replacement is a tombstone** -- a row that names
//! the version it retires -- and the new version is a new row with a new id.
//! **The tombstone is in the same file as the new version**, so the two land
//! together: one file, one manifest, one rename, and a reader sees the old
//! version and the new one or neither.
//!
//! **And the live rows are the ones the tombstones do not name.** The
//! tombstone's `row_id` is the retired version's id, so the retired ids are
//! exactly the `row_id`s of the rows whose `deleted` flag is set. A read
//! filters on `deleted = 0` -- which drops the tombstones themselves -- and on
//! `row_id != <retired id>` for each retired id -- which drops the versions
//! the tombstones name. **The second filter is the tombstone tax**: one
//! predicate per retired version, and the compaction is what pays it back.
//!
//! **And the ids are a function of what is on disk.** The id a new version
//! gets is the number of versions the table has ever had, and that number is
//! the manifest's rows less the retired ones -- so it is read from the
//! durable state and never written down. **So there is no counter to lose**:
//! a crash between two writes cannot make two rows share an id.

use std::collections::BTreeSet;
use std::path::PathBuf;

use columnar_engine::{
    ComparisonOp, Dataset, ExecOptions, FileSetSource, Predicate, QueryPlan, ScalarValue,
};

use crate::error::{Error, Result};
use crate::row::columns_of;

/// The hidden column that holds a version's own id.
pub(crate) const ROW_ID: &str = "row_id";

/// The hidden column that says whether a row is a tombstone.
pub(crate) const DELETED: &str = "deleted";

/// The value of the `deleted` column for a live row.
pub(crate) const LIVE: i64 = 0;

/// The value of the `deleted` column for a tombstone.
pub(crate) const TOMBSTONE: i64 = 1;

/// Whether a column name is one of the hidden ones.
#[must_use]
pub(crate) fn is_hidden(name: &str) -> bool {
    name == ROW_ID || name == DELETED
}

/// The filter that leaves only the live rows.
///
/// **Two kinds of predicate, and both are needed.** The `deleted = 0` drops
/// the tombstones themselves -- the rows the database wrote to say that a
/// version is gone. The `row_id != <retired id>` drops the versions the
/// tombstones name -- the rows that are still in an old file, unchanged,
/// because a file is never rewritten.
pub(crate) fn live_filter(dead: &BTreeSet<i64>) -> Vec<Predicate> {
    let mut filter = vec![Predicate::new(DELETED, ComparisonOp::Eq, ScalarValue::Int64(LIVE))];
    for id in dead {
        filter.push(Predicate::new(ROW_ID, ComparisonOp::Ne, ScalarValue::Int64(*id)));
    }
    filter
}

/// A row as it is written: the application's values, the version's id, and
/// whether the row is a tombstone.
pub(crate) struct Stored {
    /// The application's values, one per column.
    pub(crate) values: Vec<Option<ScalarValue>>,
    /// The version's own id -- or, for a tombstone, the id it retires.
    pub(crate) row_id: i64,
    /// Whether the row is a tombstone.
    pub(crate) deleted: bool,
}

/// A row waiting to be written.
#[derive(Debug)]
pub(crate) enum Pending {
    /// A version of a row: the application's values. **The id is assigned when
    /// the row is written**, because the id depends on what is already on disk.
    Version {
        /// The application's values, one per column.
        values: Vec<Option<ScalarValue>>,
    },
    /// A tombstone: the id of the version it retires.
    Tombstone {
        /// The id of the version this tombstone retires.
        row_id: i64,
    },
}

/// The live rows of a table: the application's values, and the version ids.
pub(crate) struct Live {
    /// The application's values, one vector per column.
    pub(crate) values: Vec<Vec<Option<ScalarValue>>>,
    /// The version id of each row, in the rows' order.
    pub(crate) ids: Vec<i64>,
}

/// The retired versions of a table: the ids its tombstones name.
///
/// **The query is the tombstones' own rows**: the rows whose `deleted` flag is
/// set, and their `row_id`. One scan of the table's files, and the answer is
/// the whole of what a read has to exclude.
pub(crate) fn retired(files: &[PathBuf]) -> Result<BTreeSet<i64>> {
    let plan = QueryPlan::new(
        vec![Predicate::new(DELETED, ComparisonOp::Eq, ScalarValue::Int64(TOMBSTONE))],
        vec![ROW_ID.to_owned()],
        None,
    );
    let dataset = read(files, &plan)?;
    let (_, columns) = columns_of(&dataset)?;
    let mut ids = BTreeSet::new();
    if let Some(column) = columns.first() {
        for value in column {
            if let Some(ScalarValue::Int64(id)) = value {
                ids.insert(*id);
            }
        }
    }
    Ok(ids)
}

/// The live rows of a table.
///
/// **The read is the live filter**, so the rows that come back are the rows an
/// application would see, and the ids that come with them are the versions'
/// own.
pub(crate) fn live(files: &[PathBuf], dead: &BTreeSet<i64>, app_columns: usize) -> Result<Live> {
    let plan = QueryPlan::new(live_filter(dead), Vec::new(), None);
    let dataset = read(files, &plan)?;
    let (names, columns) = columns_of(&dataset)?;
    let Some(index) = names.iter().position(|name| name == ROW_ID) else {
        return Err(Error::Corrupt {
            column: ROW_ID.to_owned(),
            reason: "the table's rows do not hold the row id".to_owned(),
        });
    };
    let mut ids = Vec::with_capacity(dataset.len());
    if let Some(column) = columns.get(index) {
        for value in column {
            if let Some(ScalarValue::Int64(id)) = value {
                ids.push(*id);
            }
        }
    }
    let values = columns.into_iter().take(app_columns).collect();
    Ok(Live { values, ids })
}

/// The versions of a table that match a predicate.
///
/// **The predicate is the application's**, and it is checked against the live
/// rows only: a tombstone is not a row an application can match, and a
/// retired version is not a row an application can see.
pub(crate) fn matching(
    files: &[PathBuf],
    dead: &BTreeSet<i64>,
    predicate: Predicate,
) -> Result<Vec<i64>> {
    let mut filter = vec![predicate];
    filter.extend(live_filter(dead));
    let plan = QueryPlan::new(filter, vec![ROW_ID.to_owned()], None);
    let dataset = read(files, &plan)?;
    let (_, columns) = columns_of(&dataset)?;
    let mut ids = Vec::new();
    if let Some(column) = columns.first() {
        for value in column {
            if let Some(ScalarValue::Int64(id)) = value {
                ids.push(*id);
            }
        }
    }
    Ok(ids)
}

/// Reads a table's files through a plan.
fn read(files: &[PathBuf], plan: &QueryPlan) -> Result<Dataset> {
    if files.is_empty() {
        return Ok(Dataset::from_chunks(Vec::new()));
    }
    let source = FileSetSource::open(files)?;
    Ok(source.execute_rows(plan, &ExecOptions::default())?)
}
