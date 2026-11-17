//! The database's errors.

use std::io;
use std::path::PathBuf;

/// What a database returns.
pub type Result<T> = std::result::Result<T, Error>;

/// What can go wrong in a database.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The file system refused.
    #[error("the file system refused: {0}")]
    Io(#[from] io::Error),

    /// The catalog on disk is not one this database can read.
    #[error("the catalog at {path} is not one this database can read: {reason}")]
    Catalog {
        /// The catalog's path.
        path: PathBuf,
        /// Why it could not be read.
        reason: String,
    },

    /// The catalog could not be written as JSON.
    #[error("the catalog could not be written as JSON: {0}")]
    Json(#[from] serde_json::Error),

    /// The store refused.
    #[error("the store refused: {0}")]
    Store(#[from] columnar_store::Error),

    /// The table's name is not one a database can hold.
    #[error("the table's name is not one a database can hold: {name}")]
    Name {
        /// The name that was refused.
        name: String,
    },

    /// The database already holds a table with that name.
    #[error("the database already holds a table named {name}")]
    Exists {
        /// The name that was refused.
        name: String,
    },

    /// The schema is not one a table can hold.
    #[error("the schema is not one a table can hold: {reason}")]
    Schema {
        /// Why the schema was refused.
        reason: String,
    },

    /// The row does not have one value per column.
    #[error("the row has {got} values, and the table has {expected} columns")]
    RowLength {
        /// How many columns the table has.
        expected: usize,
        /// How many values the row has.
        got: usize,
    },

    /// The value's type is not the column's type.
    #[error("the value for the column {column} is a {got}, and the column is an {expected}")]
    ValueType {
        /// The column's name.
        column: String,
        /// The column's type.
        expected: String,
        /// The value's type.
        got: String,
    },

    /// A column that may not hold a NULL holds one.
    #[error("the column {column} may not hold a NULL")]
    Null {
        /// The column's name.
        column: String,
    },

    /// The database does not hold a table with that name.
    #[error("the database does not hold a table named {name}")]
    UnknownTable {
        /// The name that was asked for.
        name: String,
    },

    /// The engine refused.
    #[error("the engine refused: {0}")]
    Engine(#[from] columnar_engine::EngineError),

    /// The table has no rows, so there is nothing to aggregate.
    #[error("the table {name} has no rows")]
    EmptyTable {
        /// The table's name.
        name: String,
    },

    /// The file's rows are not readable.
    #[error("the column {column} is not readable: {reason}")]
    Corrupt {
        /// The column's name.
        column: String,
        /// Why the rows are not readable.
        reason: String,
    },
}
