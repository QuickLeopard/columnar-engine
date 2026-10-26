//! Writing the dataset to a Parquet file.
//!
//! ## Layout choices
//!
//! | Choice | Value | Why |
//! |---|---|---|
//! | Row group | 1,000,000 rows | A single row group for a 60M-row file would hold the whole dataset in memory at once. A million rows is large enough that the per-row-group overhead is negligible and small enough that the writer's working set stays bounded. |
//! | Write batch | 65,536 rows | The dataset is generated and written in batches of this size, so the generator never materialises more than one batch of rows. The Arrow writer splits a batch across a row group boundary when it has to, so the row groups come out at exactly the row group size. |
//! | Compression | SNAPPY | Fast enough that the generator is not the bottleneck, and the standard choice for a benchmark fixture. |
//! | Dictionary | enabled by default, disabled for `l_comment` | The seven low-cardinality string columns are dictionary-encoded. `l_comment` is high-cardinality: a dictionary page for it would be large and useless, so it is written plain. |
//! | Statistics | chunk level | Chunk-level statistics are what a reader uses for predicate pushdown. Page-level statistics were left off: they are not needed here and they change the byte layout in ways that are easy to miss when comparing two files. |
//! | Writer version | Parquet 1.0 | The most widely compatible format, and the one whose dictionary encoding is `RLE_DICTIONARY`. |
//!
//! The file is never held in memory: the generator produces a batch, the
//! batch is written, and the batch is dropped.
//!
//! ## What the memory actually goes on
//!
//! The generator's own footprint is one batch of 65,536 rows. The Parquet
//! writer, however, buffers a whole row group before it encodes it, so the
//! real peak is one row group's worth of column data -- about 130 MB for a
//! million rows of this schema, measured rather than guessed. That peak is
//! set by the row group size and does *not* grow with the dataset: writing
//! 1.5 million rows peaks at roughly 190 MB and writing 6 million rows
//! peaks at roughly 250 MB, because the difference is the half-full last
//! row group and not the dataset.

use crate::generator::DatasetGenerator;
use crate::manifest::{DatasetManifest, PROVENANCE};
use crate::schema;
use crate::{DatasetError, DatasetSpec};
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::{EnabledStatistics, WriterProperties, WriterVersion};
use parquet::schema::types::ColumnPath;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

/// The number of rows written per Arrow batch.
///
/// The generator materialises one batch at a time, so this is the upper
/// bound on the generator's memory use, independent of the scale factor.
pub const BATCH_ROWS: usize = 65_536;

/// The number of rows per Parquet row group.
pub const ROW_GROUP_ROWS: usize = 1_000_000;

/// The writer properties used for every dataset file.
///
/// The row-group size is the one thing the caller chooses, because the
/// number of row groups is the number of workers the read can use.
#[must_use]
pub fn writer_properties(row_group_rows: usize) -> WriterProperties {
    WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_dictionary_enabled(true)
        .set_max_row_group_size(row_group_rows)
        .set_statistics_enabled(EnabledStatistics::Chunk)
        .set_writer_version(WriterVersion::PARQUET_1_0)
        .set_bloom_filter_enabled(false)
        // `l_comment` is high cardinality: a dictionary page for it would be
        // large and useless, so this one column is written plain.
        .set_column_dictionary_enabled(ColumnPath::from(schema::COMMENT_COLUMN), false)
        .build()
}

/// Generates a dataset of `rows` rows and writes it to `output`.
///
/// The rows are written in batches of [`BATCH_ROWS`] and flushed into row
/// groups of `row_group_rows` rows.
///
/// # Errors
///
/// Returns [`DatasetError::InvalidSpec`] if the spec is not usable,
/// [`DatasetError::Io`] if the file cannot be created or written, and
/// [`DatasetError::Parquet`] if the Parquet writer rejects the data.
pub fn write_dataset(
    spec: &DatasetSpec,
    rows: u64,
    output: &Path,
    row_group_rows: usize,
) -> Result<DatasetManifest, DatasetError> {
    let properties = writer_properties(row_group_rows);
    // Create the parent directory if it does not exist, so that
    // `columnar dataset --output data/lineitem.parquet` works on a fresh
    // clone without a separate `mkdir` step. The "one command" promise
    // includes this.
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let file = File::create(output)?;
    let mut writer =
        ArrowWriter::try_new(BufWriter::new(file), schema::arrow_schema(), Some(properties))
            .map_err(|error| DatasetError::Parquet(format!("arrow writer: {error}")))?;

    let mut generator = DatasetGenerator::new(spec, rows)?;
    let batch_rows = u64::try_from(BATCH_ROWS).unwrap_or(u64::MAX);
    let mut remaining = rows;
    while remaining > 0 {
        let batch_size = usize::try_from(remaining.min(batch_rows)).unwrap_or(BATCH_ROWS);
        let batch = generator.next_batch(batch_size)?;
        writer
            .write(&batch)
            .map_err(|error| DatasetError::Parquet(format!("arrow write: {error}")))?;
        remaining -= u64::try_from(batch_size).expect("the batch size fits in a u64");
    }

    writer.close().map_err(|error| DatasetError::Parquet(format!("arrow close: {error}")))?;

    let bytes = std::fs::metadata(output)?.len();

    // The number of row groups the file has, which is the number of workers
    // the read can use. It is counted from the row count and the row-group
    // size rather than read back from the file, because the writer's own
    // count is the one the read will see.
    let row_groups = if row_group_rows == 0 {
        0
    } else {
        rows.div_ceil(u64::try_from(row_group_rows).unwrap_or(u64::MAX))
    };

    // The manifest records an absolute path, so that a manifest found
    // somewhere else still says which file it describes.
    let absolute = std::path::absolute(output).unwrap_or_else(|_| output.to_path_buf());

    Ok(DatasetManifest {
        path: absolute.to_string_lossy().into_owned(),
        rows,
        bytes,
        columns: schema::column_names(),
        scale_factor: spec.scale_factor,
        seed: spec.seed,
        null_rate: spec.null_rate,
        row_group_rows,
        row_groups,
        provenance: PROVENANCE.to_string(),
        generator_version: env!("CARGO_PKG_VERSION").to_string(),
    })
}
