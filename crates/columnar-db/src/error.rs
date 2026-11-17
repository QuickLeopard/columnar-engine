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
}
