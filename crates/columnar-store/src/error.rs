//! The store's errors.

use std::io;
use std::path::PathBuf;

/// What a store returns.
pub type Result<T> = std::result::Result<T, Error>;

/// What can go wrong in a store.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The file system refused.
    #[error("the file system refused: {0}")]
    Io(#[from] io::Error),

    /// The manifest on disk is not one this store can read.
    #[error("the manifest at {path} is not one this store can read: {reason}")]
    Manifest {
        /// The manifest's path.
        path: PathBuf,
        /// Why it could not be read.
        reason: String,
    },

    /// The rows arrived in batches that are not on the writer's grid.
    #[error("the rows arrived in batches of {got} rows, and the writer's grid is {expected}")]
    Grid {
        /// The grid the writer is on.
        expected: usize,
        /// The batch that broke it.
        got: usize,
    },

    /// The table's name is not one a store can hold.
    #[error("the table's name is not one a store can hold: {name}")]
    Name {
        /// The name that was refused.
        name: String,
    },

    /// There were no rows to write.
    #[error("there were no rows to write")]
    NoRows,

    /// The Parquet writer refused the rows.
    #[error("the parquet writer refused the rows: {0}")]
    Parquet(String),

    /// The manifest could not be written as JSON.
    #[error("the manifest could not be written as JSON: {0}")]
    Json(#[from] serde_json::Error),
}
