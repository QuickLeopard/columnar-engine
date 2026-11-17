//! The database: a directory, a catalog, and the tables the catalog names.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use columnar_store::{writer_properties, Store};

use crate::catalog::Catalog;
use crate::error::{Error, Result};
use crate::row::Row;
use crate::schema::Schema;
use crate::table::{Table, TableMut};

/// The catalog's name, inside the database's directory.
pub const CATALOG_FILE: &str = "catalog.json";

/// The catalog's name before the rename.
pub const CATALOG_TMP: &str = "catalog.json.tmp";

/// The directory the tables live in, inside the database's directory.
pub const TABLES_DIR: &str = "tables";

/// A database: a directory, a catalog, and the tables the catalog names.
///
/// **A database is a directory.** Its catalog is `catalog.json`, its tables
/// live under `tables/`, and the tables are the store's tables — the store
/// writes the files and the engine reads them, and the database is the glue
/// that says which files make up which table.
///
/// **The catalog is replaced by a rename**, the same primitive the store's
/// commit uses: it is written to `catalog.json.tmp`, made durable, renamed
/// over `catalog.json`, and the directory is made durable last. **So a reader
/// sees the old catalog or the new one, and never a half-written one** — and
/// the database adds no durability mechanism of its own to the store's.
///
/// **And the rows waiting to be written are the database's.** They are in the
/// buffer until a commit, they survive the handle that offered them, and a
/// commit is the store's five steps: the file made durable, the manifest
/// written, made durable and renamed over the old one, the directory made
/// durable. **A commit is a rename, and the rename is the answer.**
#[derive(Debug)]
pub struct Database {
    root: PathBuf,
    catalog: Catalog,
    store: Arc<Store>,
    /// The rows waiting to be written, per table.
    pub(crate) pending: BTreeMap<String, Vec<Row>>,
}

impl Database {
    /// Opens a database, creating the directory if it is not there.
    ///
    /// **A directory with no catalog is a database with no tables**, and that
    /// is not an error: it is a database that has never been written to.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the directory cannot be created or read, and
    /// [`Error::Catalog`] if the catalog on disk is not one this database can
    /// read.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        let catalog = read_catalog(&root)?;
        let store = Store::open(root.join(TABLES_DIR))?;
        Ok(Self { root, catalog, store: Arc::new(store), pending: BTreeMap::new() })
    }

    /// The database's root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The database's catalog.
    #[must_use]
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// The store the database's tables live in.
    ///
    /// **The tables are the store's tables**, and the database writes them
    /// through the store's commit — the five steps, ending in a rename. The
    /// store is here because an embedder that wants to write rows directly
    /// should be able to, and because the write path is the store's, not the
    /// database's.
    #[must_use]
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Creates a table, and writes the catalog.
    ///
    /// **The table exists as soon as the catalog says so.** Its directory is
    /// made when the first rows are written, because the store makes it then
    /// and a table with no rows has no files to name.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Name`] if the name is not one a database can hold,
    /// [`Error::Exists`] if the database already holds that table, and
    /// [`Error::Io`] if the catalog cannot be written.
    pub fn create_table(&mut self, name: &str, schema: Schema) -> Result<TableMut<'_>> {
        check_name(name)?;
        if self.catalog.get(name).is_some() {
            return Err(Error::Exists { name: name.to_owned() });
        }
        let relative = Path::new(TABLES_DIR).join(name);
        let mut catalog = self.catalog.clone();
        catalog.insert(name, schema.clone(), relative.clone());
        write_catalog(&self.root, &catalog)?;
        self.catalog = catalog;
        let path = self.root.join(relative);
        Ok(TableMut::new(self, name.to_owned(), schema, path))
    }

    /// The table with that name, if the database holds it.
    #[must_use]
    pub fn table(&self, name: &str) -> Option<Table> {
        self.catalog
            .get(name)
            .map(|entry| Table::new(name, entry.schema().clone(), self.root.join(entry.path())))
    }

    /// A handle to the table with that name, for writing.
    ///
    /// **The handle borrows the database, and that is the whole of the
    /// one-writer rule.** The compiler will not let two handles write at once,
    /// and the store's commit is atomic on top of that.
    #[must_use]
    pub fn table_mut(&mut self, name: &str) -> Option<TableMut<'_>> {
        let entry = self.catalog.get(name)?;
        let schema = entry.schema().clone();
        let path = self.root.join(entry.path());
        Some(TableMut::new(self, name.to_owned(), schema, path))
    }

    /// The database's table names, in name order.
    #[must_use]
    pub fn tables(&self) -> Vec<String> {
        self.catalog.names().map(str::to_owned).collect()
    }

    /// Writes the rows waiting for a table, and commits them.
    ///
    /// **A commit with nothing waiting is not an error**: there is nothing to
    /// write, so there is nothing to fail.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnknownTable`] if the database does not hold the
    /// table, and [`Error::Store`] when the write fails.
    pub fn commit(&mut self, name: &str) -> Result<()> {
        if self.catalog.get(name).is_none() {
            return Err(Error::UnknownTable { name: name.to_owned() });
        }
        self.flush(name)
    }

    /// How many rows are waiting to be written for a table.
    #[must_use]
    pub fn pending_rows(&self, name: &str) -> usize {
        self.pending.get(name).map_or(0, Vec::len)
    }

    /// Writes the rows waiting for a table, and commits them.
    ///
    /// **The rows stay in the buffer if the write fails**, because a row that
    /// was offered and not written is still a row the caller believes it has.
    pub(crate) fn flush(&mut self, name: &str) -> Result<()> {
        let Some(rows) = self.pending.get(name) else {
            return Ok(());
        };
        if rows.is_empty() {
            return Ok(());
        }
        let Some(entry) = self.catalog.get(name) else {
            return Err(Error::UnknownTable { name: name.to_owned() });
        };
        let schema = entry.schema().clone();
        let chunks = crate::chunks::chunks_of(&schema, rows)?;
        self.store.append_rows(name, &chunks, writer_properties(&[]))?;
        self.pending.remove(name);
        Ok(())
    }
}

/// Reads the catalog, and a directory with no catalog is an empty catalog.
fn read_catalog(root: &Path) -> Result<Catalog> {
    let path = root.join(CATALOG_FILE);
    if !path.is_file() {
        return Ok(Catalog::new());
    }
    let text = fs::read_to_string(&path)?;
    Catalog::from_json(&text, &path)
}

/// Writes the catalog, and replaces the old one by a rename.
///
/// **The same primitive the store's commit uses, written out**: the store's
/// commit is the *table's* manifest, and the catalog is a different file with
/// a different type, and the two steps are the same three lines rather than
/// one shared piece of machinery.
fn write_catalog(root: &Path, catalog: &Catalog) -> Result<()> {
    let tmp = root.join(CATALOG_TMP);
    let mut handle = File::create(&tmp)?;
    handle.write_all(catalog.to_json()?.as_bytes())?;
    handle.flush()?;
    drop(handle);
    fsync_file(&tmp)?;

    fs::rename(&tmp, root.join(CATALOG_FILE))?;
    fsync_dir(root)?;
    Ok(())
}

/// Makes a file durable.
fn fsync_file(path: &Path) -> Result<()> {
    let file = File::open(path)?;
    file.sync_all()?;
    Ok(())
}

/// Makes a directory's entries durable.
fn fsync_dir(path: &Path) -> Result<()> {
    let dir = File::open(path)?;
    dir.sync_all()?;
    Ok(())
}

/// Checks the name of a table.
///
/// **The rule is the store's own**, because the store is what holds the
/// table's files: a name is a directory name, so it may not be empty, may not
/// be `.` or `..`, and may not carry a separator.
fn check_name(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err(Error::Name { name: name.to_owned() });
    }
    Ok(())
}
