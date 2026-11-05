//! The manifest: a table's files, and the table's totals.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The format this store's manifests are written in.
pub const FORMAT: &str = "columnar-store/1";

/// One file in a table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// The file's name, inside the table's directory.
    pub name: String,
    /// The rows in it.
    pub rows: u64,
    /// The bytes it takes on disk.
    pub bytes: u64,
}

/// A table's manifest: the files that make it up, and its totals.
///
/// **The manifest is the table's truth.** A file that the manifest does not
/// name is not part of the table, and a file the manifest names is part of it
/// whether or not a reader has opened it. The manifest is small, it is JSON,
/// and it is the one thing a reader has to believe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The format this manifest was written in.
    pub format: String,
    /// The table's name.
    pub table: String,
    /// The table's files, in the order they were committed.
    pub files: Vec<FileEntry>,
    /// The rows in the table.
    pub rows: u64,
    /// The bytes the table takes on disk.
    pub bytes: u64,
}

impl Manifest {
    /// A manifest for a table that has no files yet.
    #[must_use]
    pub fn new(table: impl Into<String>) -> Self {
        Self {
            format: FORMAT.to_owned(),
            table: table.into(),
            files: Vec::new(),
            rows: 0,
            bytes: 0,
        }
    }

    /// Adds a file to the manifest, and the totals with it.
    pub fn add(&mut self, entry: FileEntry) {
        self.rows += entry.rows;
        self.bytes += entry.bytes;
        self.files.push(entry);
    }

    /// The manifest's JSON.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Json`] if the manifest cannot be written as JSON.
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(Error::Json)
    }

    /// A manifest from its JSON.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Manifest`] if the JSON is not a manifest this store can
    /// read, or if it is a manifest of another format.
    pub fn from_json(text: &str, path: &Path) -> Result<Self> {
        let manifest: Self = serde_json::from_str(text).map_err(|error| Error::Manifest {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })?;
        if manifest.format != FORMAT {
            return Err(Error::Manifest {
                path: path.to_path_buf(),
                reason: format!(
                    "the format is {}, and this store writes {FORMAT}",
                    manifest.format
                ),
            });
        }
        Ok(manifest)
    }
}
