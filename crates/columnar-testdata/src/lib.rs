//! A deterministic, TPC-H-shaped dataset generator for the columnar engine.
//!
//! # What this crate produces
//!
//! A Parquet file holding a synthetic `lineitem` table: sixteen columns in a
//! fixed order, with the distributions TPC-H's `lineitem` has -- keys,
//! quantities, prices, discounts, taxes, categorical strings and free-text
//! comments, spread over the 1992-1998 shipping window.
//!
//! **This is not the official TPC-H SF-1 file.** It is TPC-H-*shaped*:
//! the distributions are approximated, the row count is exact
//! (`6_000_000 * scale_factor`), and every value comes from a seeded PRNG
//! rather than from the TPC-H generator. A benchmark number produced against
//! this dataset is a number about this dataset. The manifest says so, in
//! `provenance`, and it should not be paraphrased as "TPC-H SF-1".
//!
//! # The schema is a contract
//!
//! The sixteen columns, their order and their types are shared with
//! `columnar-engine`, which is written against exactly this layout:
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
//! Two things about that table are deliberate and worth stating:
//!
//! - **The dates are `Utf8`, not `Date32`.** They are formatted
//!   `YYYY-MM-DD`. The engine models strings, not dates; claiming a richer
//!   type system than is implemented would be a claim rather than evidence.
//! - **Seven string columns are dictionary-encoded on disk.** A columnar
//!   engine without dictionary-encoded strings is a numeric toy, and the Q6
//!   example filters a dictionary-encoded string column, not a numeric one.
//!   `l_comment` is the exception: it is high-cardinality, so it is written
//!   plain.
//!
//! # Determinism
//!
//! The dataset is a pure function of `(scale_factor, seed, null_rate)`. The
//! PRNG is hand-written -- `splitmix64` to expand the seed, `xoshiro256**`
//! for the stream -- because `rand` is banned workspace-wide and because a
//! generator whose output depends on the version of a crate in the
//! dependency graph is not reproducible, only repeatable.
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

pub use manifest::DatasetManifest;

use std::path::Path;

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
    let rows = schema::rows_for_scale_factor(spec.scale_factor);
    writer::write_dataset(spec, rows, output)
}
