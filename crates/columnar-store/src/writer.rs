//! The writer: rows in, a Parquet file out.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::{EnabledStatistics, WriterProperties, WriterVersion};
use parquet::schema::types::ColumnPath;

use crate::error::{Error, Result};
use crate::manifest::FileEntry;

/// The rows in the row groups the store writes.
///
/// The row-group size is the one thing the caller chooses, because the number
/// of row groups is the number of workers the read can use.
pub const ROW_GROUP_ROWS: usize = 1_000_000;

/// The rows in a batch, and the grid the file's bytes depend on.
///
/// **The writer's output is a function of this.** The Parquet writer splits
/// every batch it is handed into mini-batches of 1,024 values, and the
/// dictionary's fallback to plain is decided at one of those boundaries, so
/// the same rows in a different batch size are a different file. A store that
/// promises reproducible files has to fix the grid, and this is the grid the
/// published file was written in.
pub const BATCH_ROWS: usize = 65_536;

/// The buffer the writer's output goes through.
const WRITE_BUFFER_BYTES: usize = 1024 * 1024;

/// The writer's properties: the dictionary on, a million rows a row group, SNAPPY.
///
/// The columns named in `plain` are written **without** a dictionary. That is
/// the right call for a high-cardinality column, whose dictionary page would be
/// large and useless -- and the Parquet writer's own fallback leaves exactly
/// that page in the file, for every reader to skip.
#[must_use]
pub fn writer_properties(plain: &[&str]) -> WriterProperties {
    let mut builder = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_dictionary_enabled(true)
        .set_max_row_group_row_count(Some(ROW_GROUP_ROWS))
        .set_statistics_enabled(EnabledStatistics::Chunk)
        .set_writer_version(WriterVersion::PARQUET_1_0)
        .set_bloom_filter_enabled(false);
    for column in plain {
        builder = builder.set_column_dictionary_enabled(ColumnPath::from(*column), false);
    }
    builder.build()
}

/// The writer: rows in, a Parquet file out.
pub struct Writer {
    /// The file being written.
    path: PathBuf,
    /// The Parquet writer.
    inner: ArrowWriter<BufWriter<File>>,
    /// The rows written so far.
    rows: u64,
}

impl std::fmt::Debug for Writer {
    /// The writer's path and its row count. The Parquet writer itself has no
    /// `Debug`, and it is the file that says what is going on.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Writer")
            .field("path", &self.path)
            .field("rows", &self.rows)
            .field("inner", &"the parquet writer")
            .finish()
    }
}

impl Writer {
    /// A writer for a new file in `dir`, with the store's own properties.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the file cannot be created, and [`Error::Parquet`]
    /// if the Parquet writer cannot be built.
    pub fn create(dir: &Path, name: &str, schema: SchemaRef) -> Result<Self> {
        Self::create_with(dir, name, schema, writer_properties(&[]))
    }

    /// A writer for a new file in `dir`, with the caller's own properties.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the file cannot be created, and [`Error::Parquet`]
    /// if the Parquet writer cannot be built.
    pub fn create_with(
        dir: &Path,
        name: &str,
        schema: SchemaRef,
        properties: WriterProperties,
    ) -> Result<Self> {
        let path = dir.join(name);
        let file = File::create(&path)?;
        let buffered = BufWriter::with_capacity(WRITE_BUFFER_BYTES, file);
        let inner = ArrowWriter::try_new(buffered, schema, Some(properties))
            .map_err(|error| Error::Parquet(error.to_string()))?;
        Ok(Self { path, inner, rows: 0 })
    }

    /// The file being written.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The rows written so far.
    #[must_use]
    pub fn rows(&self) -> u64 {
        self.rows
    }

    /// Writes a batch of rows.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Parquet`] if the Parquet writer refuses the batch.
    pub fn write(&mut self, batch: &RecordBatch) -> Result<()> {
        self.inner.write(batch).map_err(|error| Error::Parquet(error.to_string()))?;
        self.rows += u64::try_from(batch.num_rows()).unwrap_or(u64::MAX);
        Ok(())
    }

    /// Closes the file, and returns its entry.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Parquet`] if the Parquet writer cannot be closed, and
    /// [`Error::Io`] if the file's size cannot be read.
    pub fn close(self) -> Result<FileEntry> {
        self.inner.close().map_err(|error| Error::Parquet(error.to_string()))?;
        let bytes = std::fs::metadata(&self.path)?.len();
        let name = self
            .path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        Ok(FileEntry { name, rows: self.rows, bytes })
    }
}
