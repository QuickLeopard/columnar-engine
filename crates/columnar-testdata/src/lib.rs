//! A deterministic, TPC-H-shaped dataset generator for the columnar engine.
//!
//! # What this crate produces
//!
//! Two Parquet files: a synthetic `lineitem` table -- sixteen columns in a
//! fixed order, with the distributions TPC-H's `lineitem` has -- and a
//! synthetic `orders` table beside it, with the nine columns TPC-H's
//! `orders` has. The second table is the build side of the join: it is a
//! quarter of the size of `lineitem`, and its key is drawn from the same
//! key space as `l_orderkey`, so the join has a real selectivity instead of
//! being empty or universal.
//!
//! **This is not the official TPC-H SF-1 file.** It is TPC-H-*shaped*:
//! the distributions are approximated, the row count is exact
//! (`6_000_000 * scale_factor` for `lineitem`, `1_500_000 * scale_factor`
//! for `orders`), and every value comes from a seeded PRNG rather than from
//! the TPC-H generator. A benchmark number produced against this dataset
//! is a number about this dataset. The manifest says so, in `provenance`,
//! and it should not be paraphrased as "TPC-H SF-1".
//!
//! # The schema is a contract
//!
//! The sixteen `lineitem` columns, their order and their types are shared
//! with `columnar-engine`, which is written against exactly this layout:
//!
//! | # | Column | Arrow type | Engine type | Parquet encoding |
//! |---|---|---|---|---|
//! | 1 | `l_orderkey` | Int64 | Int64 | plain |
//! | 2 | `l_partkey` | Int64 | Int64 | plain |
//! | 3 | `l_suppkey` | Int64 | Int64 | plain |
//! | 4 | `l_linenumber` | Int64 | Int64 | plain |
//! | 5 | `l_quantity` | Float64 | Float64 | plain |
//! | 6 | `l_extendedprice` | Float64 | Float64 | plain |
//! | 7 | `l_discount` | Float64 | Float64 | plain |
//! | 8 | `l_tax` | Float64 | Float64 | plain |
//! | 9 | `l_returnflag` | Utf8 | Utf8 | dictionary |
//! | 10 | `l_linestatus` | Utf8 | Utf8 | dictionary |
//! | 11 | `l_shipdate` | Utf8 | Utf8 | dictionary |
//! | 12 | `l_commitdate` | Utf8 | Utf8 | dictionary |
//! | 13 | `l_receiptdate` | Utf8 | Utf8 | dictionary |
//! | 14 | `l_shipinstruct` | Utf8 | Utf8 | dictionary |
//! | 15 | `l_shipmode` | Utf8 | Utf8 | dictionary |
//! | 16 | `l_comment` | Utf8 | Utf8 | plain |
//!
//! The nine `orders` columns are a contract in the same way:
//!
//! | # | Column | Arrow type | Engine type | Parquet encoding |
//! |---|---|---|---|---|
//! | 1 | `o_orderkey` | Int64 | Int64 | plain |
//! | 2 | `o_custkey` | Int64 | Int64 | plain |
//! | 3 | `o_orderstatus` | Utf8 | Utf8 | dictionary |
//! | 4 | `o_totalprice` | Float64 | Float64 | plain |
//! | 5 | `o_orderdate` | Utf8 | Utf8 | dictionary |
//! | 6 | `o_orderpriority` | Utf8 | Utf8 | dictionary |
//! | 7 | `o_clerk` | Int64 | Int64 | plain |
//! | 8 | `o_shippriority` | Int64 | Int64 | plain |
//! | 9 | `o_comment` | Utf8 | Utf8 | plain |
//!
//! Two things about those tables are deliberate and worth stating:
//!
//! - **The dates are `Utf8`, not `Date32`.** They are formatted
//!   `YYYY-MM-DD`. The engine models strings, not dates; claiming a richer
//!   type system than is implemented would be a claim rather than evidence.
//! - **The low-cardinality string columns are dictionary-encoded on disk.**
//!   A columnar engine without dictionary-encoded strings is a numeric toy,
//!   and the Q6 example filters a dictionary-encoded string column, not a
//!   numeric one. The comment column is the exception: it is
//!   high-cardinality, so it is written plain.
//!
//! # Determinism
//!
//! The dataset is a pure function of `(scale_factor, seed, null_rate)`. The
//! PRNG is hand-written -- `splitmix64` to expand the seed, `xoshiro256**`
//! for the stream -- because `rand` is banned workspace-wide and because a
//! generator whose output depends on the version of a crate in the
//! dependency graph is not reproducible, only repeatable.
//!
//! Each table has its own stream, seeded from the same dataset seed. The
//! `orders` stream is seeded from a *derived* seed, so generating the
//! `orders` table does not move a single `lineitem` row.
//!
//! # Memory
//!
//! The dataset is never held in memory. Rows are generated in batches of
//! 65,536 and written in row groups of 1,000,000, so the working set is
//! bounded by the row group rather than by the scale factor: 6 million rows
//! and 60 million rows peak at about the same number of bytes. See
//! [`writer`] for the measured figures.
//!
//! # Example
//!
//! ```no_run
//! use columnar_testdata::{generate, DatasetSpec};
//!
//! let spec = DatasetSpec::default();
//! let manifest = generate(&spec, std::path::Path::new("lineitem.parquet"))?;
//! manifest.write_json()?;
//! println!("{} rows, {} bytes", manifest.rows, manifest.bytes);
//! # Ok::<(), columnar_testdata::DatasetError>(())
//! ```

#![deny(missing_docs)]
#![deny(unsafe_code)]

pub mod generator;
pub mod manifest;
pub mod prng;
pub mod schema;
pub mod writer;

pub use manifest::{DatasetManifest, KeySpace, TableRecord};

use std::path::{Path, PathBuf};

/// Why a dataset could not be produced.
#[derive(Debug, thiserror::Error)]
pub enum DatasetError {
    /// The dataset could not be read from or written to disk.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// The Parquet or Arrow writer rejected the data.
    #[error("parquet error: {0}")]
    Parquet(String),
    /// The dataset specification is not usable.
    #[error("invalid dataset spec: {0}")]
    InvalidSpec(String),
}

/// How a dataset should be produced.
#[derive(Debug, Clone, PartialEq)]
pub struct DatasetSpec {
    /// TPC-H scale factor. SF-1 is the default and yields `6_000_000` lineitem rows.
    pub scale_factor: u32,
    /// Seed for the PRNG. The dataset is a pure function of (`scale_factor`, `seed`).
    pub seed: u64,
    /// Fraction of rows whose `l_discount` and `l_comment` are NULL, in [0.0, 1.0].
    /// Default 0.0, so the headline numbers match a plain (non-null) dataset.
    pub null_rate: f64,
}

impl Default for DatasetSpec {
    fn default() -> Self {
        Self { scale_factor: 1, seed: 0x5EED, null_rate: 0.0 }
    }
}

/// Generates the dataset and writes it to `output`.
///
/// The dataset holds `6_000_000 * scale_factor` rows. Returns the manifest
/// describing what was written.
///
/// # Errors
///
/// Returns [`DatasetError::InvalidSpec`] if the spec is not usable,
/// [`DatasetError::Io`] if the file cannot be created or written, and
/// [`DatasetError::Parquet`] if the Parquet writer rejects the data.
pub fn generate(spec: &DatasetSpec, output: &Path) -> Result<DatasetManifest, DatasetError> {
    generate_with_row_groups(spec, output, writer::ROW_GROUP_ROWS)
}

/// Generates the dataset with a chosen row-group size, and writes it to `output`.
///
/// The row-group size is the knob the parallelism cap hangs on: the read
/// splits the file's row groups across the workers, so a file with more, and
/// smaller, row groups can use more workers.
///
/// # Errors
///
/// Returns [`DatasetError::InvalidSpec`] if the spec is not usable,
/// [`DatasetError::Io`] if the file cannot be created or written, and
/// [`DatasetError::Parquet`] if the Parquet writer rejects the data.
pub fn generate_with_row_groups(
    spec: &DatasetSpec,
    output: &Path,
    row_group_rows: usize,
) -> Result<DatasetManifest, DatasetError> {
    let rows = schema::rows_for_scale_factor(spec.scale_factor);
    writer::write_dataset(spec, rows, output, row_group_rows)
}

/// Generates the `orders` table and writes it to `output`.
///
/// The table holds `1_500_000 * scale_factor` rows. The join key,
/// `o_orderkey`, is distinct and is drawn from the same key space as
/// `l_orderkey`: `[1, 6_000_000 * scale_factor]`. Returns the manifest
/// describing what was written.
///
/// # Errors
///
/// Returns [`DatasetError::InvalidSpec`] if the spec is not usable,
/// [`DatasetError::Io`] if the file cannot be created or written, and
/// [`DatasetError::Parquet`] if the Parquet writer rejects the data.
pub fn generate_orders(spec: &DatasetSpec, output: &Path) -> Result<DatasetManifest, DatasetError> {
    generate_orders_with_row_groups(spec, output, writer::ROW_GROUP_ROWS)
}

/// Generates the `orders` table with a chosen row-group size, and writes it to `output`.
///
/// The row-group size is the knob the parallelism cap hangs on, exactly as
/// it is for the `lineitem` file.
///
/// # Errors
///
/// Returns [`DatasetError::InvalidSpec`] if the spec is not usable,
/// [`DatasetError::Io`] if the file cannot be created or written, and
/// [`DatasetError::Parquet`] if the Parquet writer rejects the data.
pub fn generate_orders_with_row_groups(
    spec: &DatasetSpec,
    output: &Path,
    row_group_rows: usize,
) -> Result<DatasetManifest, DatasetError> {
    let orders_rows = schema::orders_rows_for_scale_factor(spec.scale_factor);
    let key_space = schema::rows_for_scale_factor(spec.scale_factor);
    writer::write_orders_dataset(spec, orders_rows, key_space, output, row_group_rows)
}

/// Generates the whole dataset: `lineitem` and `orders`, side by side.
///
/// The `orders` file is written beside the `lineitem` file -- see
/// [`orders_path_for`] -- and the two files hold the two tables of one
/// dataset: the same seed, the same scale factor, the same null rate.
///
/// The manifest that comes back records **both** tables, the key space
/// their join is drawn over, and the match rate that join is expected to
/// have. The `orders` file gets a manifest of its own, written beside it,
/// because it is a file of its own; the returned manifest is the dataset's,
/// and the caller writes it, as [`generate`] does.
///
/// # Errors
///
/// Returns [`DatasetError::InvalidSpec`] if the spec is not usable,
/// [`DatasetError::Io`] if a file cannot be created or written, and
/// [`DatasetError::Parquet`] if the Parquet writer rejects the data.
pub fn generate_dataset(
    spec: &DatasetSpec,
    lineitem: &Path,
) -> Result<DatasetManifest, DatasetError> {
    generate_dataset_with_row_groups(spec, lineitem, writer::ROW_GROUP_ROWS)
}

/// Generates the whole dataset with a chosen row-group size.
///
/// The row-group size applies to both files: they are one dataset, and a
/// reader that splits the file's row groups across its workers should find
/// the same knob on both sides.
///
/// # Errors
///
/// Returns [`DatasetError::InvalidSpec`] if the spec is not usable,
/// [`DatasetError::Io`] if a file cannot be created or written, and
/// [`DatasetError::Parquet`] if the Parquet writer rejects the data.
pub fn generate_dataset_with_row_groups(
    spec: &DatasetSpec,
    lineitem: &Path,
    row_group_rows: usize,
) -> Result<DatasetManifest, DatasetError> {
    let lineitem_rows = schema::rows_for_scale_factor(spec.scale_factor);
    let orders_rows = schema::orders_rows_for_scale_factor(spec.scale_factor);

    let lineitem_manifest = writer::write_dataset(spec, lineitem_rows, lineitem, row_group_rows)?;
    let orders_path = orders_path_for(lineitem);
    let mut orders_manifest = writer::write_orders_dataset(
        spec,
        orders_rows,
        lineitem_rows,
        &orders_path,
        row_group_rows,
    )?;

    let tables = vec![TableRecord::of(&lineitem_manifest), TableRecord::of(&orders_manifest)];
    let key_space = KeySpace::join(lineitem_rows, orders_rows);

    // Both manifests describe the dataset, so both name both tables.
    orders_manifest.tables.clone_from(&tables);
    orders_manifest.key_space = Some(key_space.clone());
    orders_manifest.write_json()?;

    let mut manifest = lineitem_manifest;
    manifest.tables = tables;
    manifest.key_space = Some(key_space);
    Ok(manifest)
}

/// The path of the `orders` file beside a `lineitem` file.
///
/// `data/lineitem.parquet` sits beside `data/orders.parquet`, and
/// `data/lineitem-rg24.parquet` beside `data/orders-rg24.parquet`: the
/// name after `lineitem` is kept, so a file that says what is different
/// about it still says it.
#[must_use]
pub fn orders_path_for(lineitem: &Path) -> PathBuf {
    let stem =
        lineitem.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
    let orders_stem = match stem.strip_prefix("lineitem") {
        Some(rest) => format!("orders{rest}"),
        None => "orders".to_string(),
    };
    lineitem.with_file_name(format!("{orders_stem}.parquet"))
}
