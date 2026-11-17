//! The database: a directory, a catalog, and the tables the catalog names.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use columnar_store::Store;

use crate::catalog::Catalog;
use crate::error::{Error, Result};
use crate::schema::Schema;
use crate::table::Table;

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
#[derive(Debug)]
pub struct Database {
    root: PathBuf,
    catalog: Catalog,
    store: Store,
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
        Ok(Self { root, catalog, store })
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
    pub fn create_table(&mut self, name: &str, schema: Schema) -> Result<Table> {
        check_name(name)?;
        if self.catalog.get(name).is_some() {
            return Err(Error::Exists { name: name.to_owned() });
        }
        let relative = Path::new(TABLES_DIR).join(name);
        let mut catalog = self.catalog.clone();
        catalog.insert(name, schema.clone(), relative.clone());
        write_catalog(&self.root, &catalog)?;
        self.catalog = catalog;
        Ok(Table::new(name, schema, self.root.join(relative)))
    }

    /// The table with that name, if the database holds it.
    #[must_use]
    pub fn table(&self, name: &str) -> Option<Table> {
        self.catalog
            .get(name)
            .map(|entry| Table::new(name, entry.schema().clone(), self.root.join(entry.path())))
    }

    /// The database's table names, in name order.
    #[must_use]
    pub fn tables(&self) -> Vec<String> {
        self.catalog.names().map(str::to_owned).collect()
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
