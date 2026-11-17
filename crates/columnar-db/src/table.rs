//! A table: a name, a schema, and the directory its files live in.

use std::path::{Path, PathBuf};

use crate::schema::Schema;

/// A table the database holds.
///
/// **A table is a set of files, and the schema is what the files are
/// supposed to hold.** The two are not the same thing: the files are what
/// the rows turned out to be, and the schema is the promise the application
/// made when it created the table. A read resolves the plan against the
/// files' own schema, so a file that does not match the promise fails at the
/// read rather than at the write.
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
