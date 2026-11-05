//! The store: a directory of tables, and the catalog that names them.

use std::fs;
use std::path::{Path, PathBuf};

use arrow::record_batch::RecordBatch;
use parquet::file::properties::WriterProperties;

use crate::commit::{commit, MANIFEST_FILE};
use crate::error::{Error, Result};
use crate::manifest::{FileEntry, Manifest};
use crate::writer::{Writer, BATCH_ROWS};

/// A store: a directory of tables, and the catalog that names them.
///
/// **A table is a set of files**, and the manifest in the table's directory is
/// what names them. A commit appends a file and never modifies an old one, so
/// the store grows by adding to the set and never rewrites it.
#[derive(Debug, Clone)]
pub struct Store {
    /// The store's root.
    root: PathBuf,
}

impl Store {
    /// Opens a store, creating the directory if it is not there.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the directory cannot be created.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    /// The store's root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The tables the store holds, in name order.
    ///
    /// A directory with a manifest in it is a table. A directory without one is
    /// not, and neither is a file.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the directory cannot be read.
    pub fn tables(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if path.join(MANIFEST_FILE).is_file() {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        names.sort();
        Ok(names)
    }

    /// A table's manifest, if the store holds the table.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Manifest`] if the manifest on disk is not one this store
    /// can read, and [`Error::Io`] if it cannot be read.
    pub fn manifest(&self, name: &str) -> Result<Option<Manifest>> {
        let path = self.table_dir(name).join(MANIFEST_FILE);
        if !path.is_file() {
            return Ok(None);
        }
        let text = fs::read_to_string(&path)?;
        Ok(Some(Manifest::from_json(&text, &path)?))
    }

    /// A table, if the store holds it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Manifest`] if the manifest on disk is not one this store
    /// can read, and [`Error::Io`] if it cannot be read.
    pub fn table(&self, name: &str) -> Result<Option<Table>> {
        Ok(self.manifest(name)?.map(|manifest| Table { dir: self.table_dir(name), manifest }))
    }

    /// A table's directory.
    #[must_use]
    pub fn table_dir(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// A table's file.
    #[must_use]
    pub fn file_path(&self, name: &str, file: &str) -> PathBuf {
        self.table_dir(name).join(file)
    }

    /// Appends rows to a table, and commits them.
    ///
    /// The rows arrive in batches, and **every batch but the last has to be on
    /// the writer's grid**: the file's bytes depend on it, and a file that
    /// comes out differently is not a bug in the store but a caller that
    /// batched differently. The store says so rather than writing a file whose
    /// bytes nobody can predict.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Name`] if the table's name is not one a store can hold,
    /// [`Error::Grid`] if the rows are not on the writer's grid,
    /// [`Error::NoRows`] if there are no rows, and [`Error::Io`] if the file
    /// system refuses.
    pub fn append(&self, name: &str, batches: &[RecordBatch]) -> Result<Manifest> {
        self.append_with(name, batches, crate::writer::writer_properties(&[]))
    }

    /// Appends rows to a table with the caller's own writer properties, and
    /// commits them.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Name`] if the table's name is not one a store can hold,
    /// [`Error::Grid`] if the rows are not on the writer's grid,
    /// [`Error::NoRows`] if there are no rows, and [`Error::Io`] if the file
    /// system refuses.
    pub fn append_with(
        &self,
        name: &str,
        batches: &[RecordBatch],
        properties: WriterProperties,
    ) -> Result<Manifest> {
        check_name(name)?;
        check_grid(batches)?;
        let schema = batches.first().map(RecordBatch::schema).ok_or(Error::NoRows)?;

        let dir = self.table_dir(name);
        fs::create_dir_all(&dir)?;
        let mut manifest = self.manifest(name)?.unwrap_or_else(|| Manifest::new(name));

        let file_name = format!("{:06}.parquet", manifest.files.len() + 1);
        let mut writer = Writer::create_with(&dir, &file_name, schema, properties)?;
        for batch in batches {
            writer.write(batch)?;
        }
        let entry = writer.close()?;
        manifest.add(entry);

        commit(&dir, &manifest)?;
        Ok(manifest)
    }
}

/// A table: its manifest, and the directory it lives in.
#[derive(Debug, Clone)]
pub struct Table {
    /// The table's directory.
    dir: PathBuf,
    /// The table's manifest.
    manifest: Manifest,
}

impl Table {
    /// The table's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.manifest.table
    }

    /// The table's manifest.
    #[must_use]
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The table's files, in the order they were committed.
    #[must_use]
    pub fn files(&self) -> &[FileEntry] {
        &self.manifest.files
    }

    /// The table's files' **paths**, in the order they were committed.
    ///
    /// **This is the glue.** The engine reads a table that is **many files**
    /// through one source over a set of files, and that source takes the list
    /// of paths -- so this list is what the embedder hands it:
    ///
    /// ```text
    /// let paths = table.paths();
    /// let set = FileSetSource::open(&paths)?;
    /// ```
    ///
    /// The order is the table's order, and it is the order the rows come back
    /// in.
    #[must_use]
    pub fn paths(&self) -> Vec<PathBuf> {
        self.manifest.files.iter().map(|entry| self.dir.join(&entry.name)).collect()
    }

    /// The rows in the table.
    #[must_use]
    pub fn rows(&self) -> u64 {
        self.manifest.rows
    }

    /// The bytes the table takes on disk.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.manifest.bytes
    }

    /// The table's directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// A file's path.
    #[must_use]
    pub fn file_path(&self, file: &str) -> PathBuf {
        self.dir.join(file)
    }
}

/// Checks that a table's name is one a store can hold.
fn check_name(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err(Error::Name { name: name.to_owned() });
    }
    Ok(())
}

/// Checks that the rows arrive on the writer's grid.
///
/// **Every batch but the last has to be exactly the grid's rows**, and the last
/// may be shorter -- it is the end of the stream. The reason is in the writer:
/// the Parquet writer splits every batch it is handed into mini-batches of
/// 1,024 values, and the dictionary's fallback to plain is decided at one of
/// those boundaries, so the file's bytes are a function of the grid.
fn check_grid(batches: &[RecordBatch]) -> Result<()> {
    let last = batches.len().saturating_sub(1);
    for (index, batch) in batches.iter().enumerate() {
        let rows = batch.num_rows();
        let on_grid = rows == BATCH_ROWS || (index == last && rows < BATCH_ROWS);
        if !on_grid {
            return Err(Error::Grid { expected: BATCH_ROWS, got: rows });
        }
    }
    Ok(())
}
