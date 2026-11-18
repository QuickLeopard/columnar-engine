//! The database: a directory, a catalog, and the tables the catalog names.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use columnar_engine::{ColumnChunk, Predicate};
use columnar_store::{writer_properties, FileEntry, Manifest, Store, Writer};

use crate::catalog::Catalog;
use crate::error::{Error, Result};
use crate::readers::Readers;
use crate::row::Row;
use crate::schema::Schema;
use crate::snapshot::{PinnedTable, Snapshot};
use crate::table::{Table, TableMut};
use crate::versions::{Live, Pending, Stored, ROW_ID};

/// The catalog's name, inside the database's directory.
pub const CATALOG_FILE: &str = "catalog.json";

/// The catalog's name before the rename.
pub const CATALOG_TMP: &str = "catalog.json.tmp";

/// The directory the tables live in, inside the database's directory.
pub const TABLES_DIR: &str = "tables";

/// What a compaction did.
///
/// **The rows are the live rows the new file holds**, and the two counts are
/// the files that went and the files that stayed. **A file that stayed is a
/// file a reader still holds**, and it is removed at the next compaction that
/// no reader stands in the way of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Compaction {
    /// The live rows the new file holds.
    pub rows: u64,
    /// The files the compaction removed.
    pub reclaimed: usize,
    /// The files the compaction left, because a reader still holds them.
    pub pending: usize,
}

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
///
/// **And a row has an identity the application never sees.** Every row
/// carries a hidden `row_id` and a hidden `deleted` flag, and a row that
/// changes leaves a tombstone behind it rather than being rewritten. **The
/// ids are read from what is on disk and never written down**, so there is no
/// counter to lose.
#[derive(Debug)]
pub struct Database {
    root: PathBuf,
    catalog: Catalog,
    store: Arc<Store>,
    /// The rows waiting to be written, per table.
    pub(crate) pending: BTreeMap<String, Vec<Pending>>,
    /// The retired versions of a table, per table.
    dead: BTreeMap<String, BTreeSet<i64>>,
    /// The readers that are alive.
    readers: Arc<Readers>,
    /// The files that no manifest names, and that a reader may still hold.
    orphans: Vec<Vec<PathBuf>>,
}

impl Database {
    /// Opens a database, creating the directory if it is not there.
    ///
    /// **A directory with no catalog is a database with no tables**, and that
    /// is not an error: it is a database that has never been written to.
    ///
    /// **And the files a compaction left behind are collected.** They are the
    /// files no manifest names, so no reader will ever open them — but they
    /// are collected and not removed, because the removal is the same removal
    /// a compaction makes, and it waits for a reader that is not there.
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
        let store = Arc::new(Store::open(root.join(TABLES_DIR))?);
        let mut database = Self {
            root,
            catalog,
            store,
            pending: BTreeMap::new(),
            dead: BTreeMap::new(),
            readers: Arc::new(Readers::default()),
            orphans: Vec::new(),
        };
        database.collect_orphans()?;
        Ok(database)
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
    /// **And the schema may not use the database's own column names.** The
    /// hidden `row_id` and `deleted` columns are in every file of every table,
    /// so a schema that used either name would be a schema the files could not
    /// hold.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Name`] if the name is not one a database can hold,
    /// [`Error::Exists`] if the database already holds that table,
    /// [`Error::ReservedColumn`] if the schema uses one of the hidden column
    /// names, and [`Error::Io`] if the catalog cannot be written.
    pub fn create_table(&mut self, name: &str, schema: Schema) -> Result<TableMut<'_>> {
        check_name(name)?;
        if self.catalog.get(name).is_some() {
            return Err(Error::Exists { name: name.to_owned() });
        }
        for column in schema.columns() {
            if crate::versions::is_hidden(column.name()) {
                return Err(Error::ReservedColumn { column: column.name().to_owned() });
            }
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

    /// Takes a snapshot of the database.
    ///
    /// **The snapshot pins every table's manifest**, so the file list each
    /// manifest named is that table for the life of the snapshot. **A write
    /// that lands afterwards is a write the snapshot does not see**, and a
    /// reader sees the old file set or the new one, never a mixture.
    ///
    /// **And the snapshot owns what it needs**, so the reader does not hold
    /// the database: the writer can write while the reader reads, and the
    /// reader's view does not move.
    ///
    /// **And the snapshot holds the files it pinned**, so a compaction knows
    /// that they are not free to remove.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Store`] when a manifest cannot be read.
    pub fn snapshot(&self) -> Result<Snapshot> {
        let mut tables = BTreeMap::new();
        let mut catalog = columnar_engine::Catalog::new();
        let mut by_path = BTreeMap::new();
        let mut held = Vec::new();
        for name in self.catalog.names() {
            let entry = self.catalog.get(name).expect("the catalog holds the name");
            let schema = entry.schema().clone();
            let files = self.files_of(name)?;
            // **The engine's catalog resolves a name to one path**, and the
            // database resolves it to the whole table. The first file is the
            // name's anchor, and the pinned list is the table.
            if let Some(first) = files.first() {
                catalog.insert(name, first.clone());
                by_path.insert(first.clone(), name.to_owned());
            }
            held.extend(files.iter().cloned());
            tables.insert(name.to_owned(), PinnedTable { name: name.to_owned(), schema, files });
        }
        let id = self.readers.register(held);
        Ok(Snapshot::new(tables, catalog, by_path, self.readers.clone(), id))
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
        let Some(rows) = self.pending.remove(name) else {
            return Ok(());
        };
        if rows.is_empty() {
            return Ok(());
        }
        let outcome = self.write_pending(name, &rows);
        match outcome {
            Ok(()) => Ok(()),
            Err(error) => {
                self.pending.insert(name.to_owned(), rows);
                Err(error)
            }
        }
    }

    /// Writes the rows waiting for a table, and commits them.
    fn write_pending(&mut self, name: &str, rows: &[Pending]) -> Result<()> {
        let Some(entry) = self.catalog.get(name) else {
            return Err(Error::UnknownTable { name: name.to_owned() });
        };
        let schema = entry.schema().clone();
        let mut dead = self.dead_set(name)?;
        let base = self.next_id(name, &dead)?;
        let stored = stored_rows(rows, base);
        let chunks = crate::chunks::chunks_of(&schema, &stored)?;
        self.write_file(name, &chunks)?;
        for row in &stored {
            if row.deleted {
                dead.insert(row.row_id);
            }
        }
        self.dead.insert(name.to_owned(), dead);
        Ok(())
    }

    /// Updates the rows that match a predicate, and answers with how many.
    ///
    /// **The predicate is the application's**, and it is checked against the
    /// live rows: a tombstone is not a row an application can match, and a
    /// retired version is not a row an application can see.
    ///
    /// **And an update does not rewrite anything.** Each matching version
    /// leaves a tombstone behind it and is replaced by a new version with a
    /// new id, and the two land in the same file — so a reader sees the old
    /// version and the new one, or neither, and never half of an update.
    pub(crate) fn update(&mut self, name: &str, predicate: Predicate, row: Row) -> Result<usize> {
        let Some(entry) = self.catalog.get(name) else {
            return Err(Error::UnknownTable { name: name.to_owned() });
        };
        let schema = entry.schema().clone();
        let values = crate::chunks::checked_values(&schema, &row.into_values())?;
        // **The rows waiting are written first**, so the update sees them.
        self.flush(name)?;
        let files = self.files_of(name)?;
        let dead = self.dead_set(name)?;
        let ids = crate::versions::matching(&files, &dead, predicate)?;
        if ids.is_empty() {
            return Ok(0);
        }
        let mut pending = Vec::with_capacity(ids.len() * 2);
        for row_id in &ids {
            // **The tombstone and the new version go in together.**
            pending.push(Pending::Tombstone { row_id: *row_id });
            pending.push(Pending::Version { values: values.clone() });
        }
        self.pending.entry(name.to_owned()).or_default().extend(pending);
        self.flush(name)?;
        Ok(ids.len())
    }

    /// Deletes the rows that match a predicate, and answers with how many.
    ///
    /// **A delete is a tombstone**, and a tombstone is a row: the version stays
    /// in its file, unchanged, and a row that names it says that it is gone.
    pub(crate) fn delete(&mut self, name: &str, predicate: Predicate) -> Result<usize> {
        if self.catalog.get(name).is_none() {
            return Err(Error::UnknownTable { name: name.to_owned() });
        }
        self.flush(name)?;
        let files = self.files_of(name)?;
        let dead = self.dead_set(name)?;
        let ids = crate::versions::matching(&files, &dead, predicate)?;
        if ids.is_empty() {
            return Ok(0);
        }
        let pending: Vec<Pending> =
            ids.iter().map(|row_id| Pending::Tombstone { row_id: *row_id }).collect();
        self.pending.entry(name.to_owned()).or_default().extend(pending);
        self.flush(name)?;
        Ok(ids.len())
    }

    /// Compacts a table: the live rows are rewritten into one file.
    ///
    /// **The compaction is what pays the tombstone tax back.** The tax is one
    /// predicate per retired version, and the compaction is the rewriting that
    /// makes the retired versions stop existing — so the live rows are read,
    /// written into a new file, and the manifest is replaced by one that names
    /// it and nothing else.
    ///
    /// **And the versions are renumbered.** A compacted table has no retired
    /// versions, so its ids are `0, 1, 2, ...` in the order they are written,
    /// and the id the next version gets is the number of rows in the table.
    ///
    /// **And the old files go when no reader holds them.** A reader that
    /// pinned them still has to read them, so they stay until it is gone.
    pub(crate) fn compact(&mut self, name: &str) -> Result<Compaction> {
        let Some(entry) = self.catalog.get(name) else {
            return Err(Error::UnknownTable { name: name.to_owned() });
        };
        let schema = entry.schema().clone();
        // **The rows waiting are written first**: a compaction rewrites what is
        // on disk, and a row that is still in the buffer is not on disk.
        self.flush(name)?;
        let files = self.files_of(name)?;
        // **A table with no files has nothing to compact**, and a compaction
        // that wrote an empty file would be a file the store's writer cannot
        // write.
        if files.is_empty() {
            let (reclaimed, pending) = self.reclaim();
            return Ok(Compaction { rows: 0, reclaimed, pending });
        }
        let dead = self.dead_set(name)?;
        let names: Vec<String> = schema.names().map(str::to_owned).collect();
        let live = crate::versions::live(&files, &dead, schema.len(), &names)?;
        let rows = renumbered(&live)?;
        let chunks = crate::chunks::chunks_of(&schema, &rows)?;
        let dir = self.store.table_dir(name);
        fs::create_dir_all(&dir)?;
        let file = write_new_file(&dir, &chunks)?;
        let mut manifest = Manifest::new(name);
        manifest.add(file);
        columnar_store::commit(&dir, &manifest)?;
        // **The old files are the table's orphans now.**
        self.orphans.push(files);
        // **And the table has no retired versions left.**
        self.dead.insert(name.to_owned(), BTreeSet::new());
        let (reclaimed, pending) = self.reclaim();
        let rows = u64::try_from(live.ids.len()).unwrap_or(u64::MAX);
        Ok(Compaction { rows, reclaimed, pending })
    }

    /// The retired versions of a table.
    fn dead_set(&mut self, name: &str) -> Result<BTreeSet<i64>> {
        if let Some(dead) = self.dead.get(name) {
            return Ok(dead.clone());
        }
        let files = self.files_of(name)?;
        let dead = crate::versions::retired(&files)?;
        self.dead.insert(name.to_owned(), dead.clone());
        Ok(dead)
    }

    /// The id the next version gets.
    ///
    /// **The number of versions the table has ever had**, which is the
    /// manifest's rows less the retired ones. **And it is read, not
    /// written**: the counter is a function of what is on disk, so there is no
    /// counter to lose and no window in which two rows could share an id.
    fn next_id(&mut self, name: &str, dead: &BTreeSet<i64>) -> Result<i64> {
        let rows = self.store.manifest(name)?.map_or(0, |manifest| manifest.rows);
        let retired = u64::try_from(dead.len()).unwrap_or(u64::MAX);
        let versions = rows.saturating_sub(retired);
        let count = usize::try_from(versions).unwrap_or(usize::MAX);
        i64::try_from(versions).map_err(|_| Error::Rows { rows: count })
    }

    /// The files a table's manifest names.
    fn files_of(&self, name: &str) -> Result<Vec<PathBuf>> {
        Ok(self.store.table(name)?.map_or_else(Vec::new, |table| table.paths()))
    }

    /// Writes a file of rows and commits it, with the manifest naming it.
    ///
    /// **The database names the file, and not the store.** The store names a
    /// file after the number of files the manifest holds, and a compaction
    /// replaces the manifest with one file — so the store's numbering would
    /// start again and the next write would land on a name an old file still
    /// has. **So the name is the highest index in the table's directory plus
    /// one**, and the bytes are still the store's writer's bytes.
    fn write_file(&self, name: &str, chunks: &[ColumnChunk]) -> Result<()> {
        let dir = self.store.table_dir(name);
        fs::create_dir_all(&dir)?;
        let mut manifest = self.store.manifest(name)?.unwrap_or_else(|| Manifest::new(name));
        let entry = write_new_file(&dir, chunks)?;
        manifest.add(entry);
        columnar_store::commit(&dir, &manifest)?;
        Ok(())
    }

    /// Removes the files no reader holds, and reports what went and what
    /// stayed.
    fn reclaim(&mut self) -> (usize, usize) {
        let mut reclaimed = 0;
        let mut kept = Vec::new();
        for set in std::mem::take(&mut self.orphans) {
            if self.readers.blocks(&set) {
                kept.push(set);
                continue;
            }
            for file in set {
                if fs::remove_file(&file).is_ok() {
                    reclaimed += 1;
                }
            }
        }
        self.orphans = kept;
        let pending: usize = self.orphans.iter().map(Vec::len).sum();
        (reclaimed, pending)
    }

    /// Collects the files that no manifest names.
    fn collect_orphans(&mut self) -> Result<()> {
        let names: Vec<String> = self.catalog.names().map(str::to_owned).collect();
        for name in names {
            let files = self.files_of(&name)?;
            let orphans = orphans_in(&self.store.table_dir(&name), &files)?;
            if !orphans.is_empty() {
                self.orphans.push(orphans);
            }
        }
        Ok(())
    }
}

/// Writes a file of rows into a table's directory, and returns its entry.
///
/// **The row id is written without a dictionary.** Every id is different, so
/// a dictionary of them would be a page of entries that are looked up once
/// each -- and the row id is the one column every file of a table has to
/// agree about.
fn write_new_file(dir: &Path, chunks: &[ColumnChunk]) -> Result<FileEntry> {
    let batches = columnar_store::rows::batches(chunks)?;
    let name = next_file_name(dir)?;
    let schema = columnar_store::rows::schema(chunks);
    let mut writer = Writer::create_with(dir, &name, schema, writer_properties(&[ROW_ID]))?;
    for batch in &batches {
        writer.write(batch)?;
    }
    Ok(writer.close()?)
}

/// The rows as they are written: the versions get their ids, and the
/// tombstones keep the ones they name.
fn stored_rows(rows: &[Pending], base: i64) -> Vec<Stored> {
    let mut next = base;
    let mut stored = Vec::with_capacity(rows.len());
    for row in rows {
        match row {
            Pending::Version { values } => {
                stored.push(Stored { values: values.clone(), row_id: next, deleted: false });
                next = next.saturating_add(1);
            }
            Pending::Tombstone { row_id } => {
                stored.push(Stored { values: Vec::new(), row_id: *row_id, deleted: true });
            }
        }
    }
    stored
}

/// The live rows, renumbered: a compacted table's versions are `0, 1, 2, ...`
/// in the order they are written.
fn renumbered(live: &Live) -> Result<Vec<Stored>> {
    let rows = live.ids.len();
    let mut stored = Vec::with_capacity(rows);
    for row in 0..rows {
        let values = live.values.iter().map(|column| column.get(row).cloned().flatten()).collect();
        let row_id = i64::try_from(row).map_err(|_| Error::Rows { rows })?;
        stored.push(Stored { values, row_id, deleted: false });
    }
    Ok(stored)
}

/// The name of the next file in a table's directory.
///
/// **The highest index in the directory plus one**, and not the number of
/// files the manifest holds: a compaction leaves the old files on disk when a
/// reader holds them, and the next write must not land on one of them.
fn next_file_name(dir: &Path) -> Result<String> {
    let mut highest = 0_u64;
    if dir.is_dir() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(index) = index_of(&name) {
                highest = highest.max(index);
            }
        }
    }
    Ok(format!("{:06}.parquet", highest.saturating_add(1)))
}

/// The index in a file's name, if it has one.
fn index_of(name: &str) -> Option<u64> {
    let stem = name.split('.').next()?;
    stem.parse::<u64>().ok()
}

/// The files in a table's directory that the manifest does not name.
fn orphans_in(dir: &Path, files: &[PathBuf]) -> Result<Vec<PathBuf>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut orphans = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("parquet") {
            continue;
        }
        if !files.contains(&path) {
            orphans.push(path);
        }
    }
    Ok(orphans)
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
