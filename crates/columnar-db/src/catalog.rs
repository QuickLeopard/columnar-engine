//! The catalog: the tables a database holds, their schemas, and their paths.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::schema::Schema;

/// The format the catalog is written in.
pub const FORMAT: &str = "columnar-db/catalog/1";

/// The tables a database holds.
///
/// **The catalog is the database's truth about names**, the way the manifest
/// is a table's truth about files: small, JSON, and the one thing a reader
/// has to believe. It is written in **name order**, so a catalog is a file a
/// human can read and diff.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalog {
    tables: BTreeMap<String, TableEntry>,
}

impl Catalog {
    /// A catalog with no tables.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a table, and returns the entry it replaced.
    ///
    /// The path is **relative to the database's root**, so a database
    /// directory can be moved or renamed and its catalog still names the same
    /// tables.
    pub fn insert(
        &mut self,
        name: impl Into<String>,
        schema: Schema,
        path: PathBuf,
    ) -> Option<TableEntry> {
        self.tables.insert(name.into(), TableEntry { schema, path })
    }

    /// Removes a table, and returns its entry.
    pub fn remove(&mut self, name: &str) -> Option<TableEntry> {
        self.tables.remove(name)
    }

    /// The table with that name, if the catalog holds it.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&TableEntry> {
        self.tables.get(name)
    }

    /// The catalog's table names, in name order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.tables.keys().map(String::as_str)
    }

    /// How many tables the catalog holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tables.len()
    }

    /// Whether the catalog holds no tables.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }

    /// The catalog's JSON.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Json`] if the catalog cannot be written as JSON.
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(&CatalogFileRef { format: FORMAT, tables: &self.tables })
            .map_err(Error::Json)
    }

    /// A catalog from its JSON.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Catalog`] if the JSON is not a catalog this database
    /// can read, or if it is a catalog of another format.
    pub fn from_json(text: &str, path: &Path) -> Result<Self> {
        let file: CatalogFile = serde_json::from_str(text).map_err(|error| Error::Catalog {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })?;
        if file.format != FORMAT {
            return Err(Error::Catalog {
                path: path.to_path_buf(),
                reason: format!("the format is {}, and this database writes {FORMAT}", file.format),
            });
        }
        Ok(Self { tables: file.tables })
    }
}

/// A table's entry: its schema, and where its files are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableEntry {
    schema: Schema,
    path: PathBuf,
}

impl TableEntry {
    /// The table's schema.
    #[must_use]
    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    /// The table's path, relative to the database's root.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// The catalog as it is written.
#[derive(Debug, Serialize)]
struct CatalogFileRef<'a> {
    format: &'a str,
    tables: &'a BTreeMap<String, TableEntry>,
}

/// The catalog as it is read.
#[derive(Debug, Deserialize)]
struct CatalogFile {
    format: String,
    tables: BTreeMap<String, TableEntry>,
}
