//! A table: a name, a schema, and the directory its files live in.
//!
//! **A table is a set of files, and the schema is what the files are
//! supposed to hold.** The two are not the same thing: the files are what
//! the rows turned out to be, and the schema is the promise the application
//! made when it created the table. A read resolves the plan against the
//! files' own schema, so a file that does not match the promise fails at the
//! read rather than at the write.

use std::path::{Path, PathBuf};

use crate::database::Database;
use crate::row::Row;
use crate::schema::Schema;

/// A table the database holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    name: String,
    schema: Schema,
    path: PathBuf,
}

impl Table {
    /// A table with that name, that schema, and that path.
    #[must_use]
    pub fn new(name: impl Into<String>, schema: Schema, path: impl Into<PathBuf>) -> Self {
        Self { name: name.into(), schema, path: path.into() }
    }

    /// The table's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The table's schema.
    #[must_use]
    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    /// The table's directory: the directory its files live in.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// A table being written.
///
/// **The handle borrows the database, and that is the whole of the one-writer
/// rule.** The rows waiting to be written are the **database's**, not the
/// handle's, so a handle that is dropped leaves its rows waiting -- and
/// [`Database::commit`] writes them. The borrow says that there is one
/// writer: the compiler will not let two handles write at once, and the
/// store's commit is atomic on top of that.
///
/// **And the rows are not durable until they are committed.** An insert puts
/// the rows in the buffer; a commit is the store's five steps -- the file
/// made durable, the manifest written, made durable and renamed over the old
/// one, the directory made durable -- and a commit is a rename.
#[derive(Debug)]
pub struct TableMut<'a> {
    database: &'a mut Database,
    name: String,
    schema: Schema,
    path: PathBuf,
}

impl<'a> TableMut<'a> {
    /// A handle to a table, for writing.
    pub(crate) fn new(
        database: &'a mut Database,
        name: String,
        schema: Schema,
        path: PathBuf,
    ) -> Self {
        Self { database, name, schema, path }
    }

    /// The table's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The table's schema.
    #[must_use]
    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    /// The table's directory: the directory its files live in.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Puts the rows in the buffer.
    ///
    /// **The rows are checked against the schema before they are buffered**:
    /// a row with the wrong number of values, a value of the wrong type, or a
    /// NULL in a column that may not hold one, is refused here and never
    /// reaches the buffer.
    ///
    /// **And the buffer is written when it reaches the grid.** The store's
    /// `append` writes one file per call, so a database that committed every
    /// insert would grow one file per insert, and every read would open every
    /// one of them. **So the rows are grouped**: the buffer is committed when
    /// it holds a grid's worth -- **65,536** rows -- and the file count then
    /// grows with the data rather than with the number of inserts.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::RowLength`] when a row does not have one value
    /// per column, [`crate::Error::ValueType`] when a value's type is not the
    /// column's, [`crate::Error::Null`] when a column that may not hold a
    /// NULL holds one, and [`crate::Error::Store`] when the write fails.
    pub fn insert(&mut self, rows: &[Row]) -> Result<(), crate::Error> {
        for row in rows {
            crate::chunks::checked_values(&self.schema, row)?;
        }
        let count = {
            let pending = self.database.pending.entry(self.name.clone()).or_default();
            pending.extend_from_slice(rows);
            pending.len()
        };
        if count >= columnar_store::BATCH_ROWS {
            self.database.flush(&self.name)?;
        }
        Ok(())
    }

    /// Writes the rows waiting for this table, and commits them.
    ///
    /// **A commit with nothing waiting is not an error**: there is nothing to
    /// write, so there is nothing to fail.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::Store`] when the write fails.
    pub fn commit(&mut self) -> Result<(), crate::Error> {
        self.database.flush(&self.name)
    }

    /// How many rows are waiting to be written.
    #[must_use]
    pub fn pending_rows(&self) -> usize {
        self.database.pending.get(&self.name).map_or(0, Vec::len)
    }
}
