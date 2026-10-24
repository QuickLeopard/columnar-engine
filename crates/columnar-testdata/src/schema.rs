//! The dataset schema: the contract shared with the engine.
//!
//! The column list, the column order and the column types are a hard
//! contract. `columnar-engine` is written against exactly this layout, so
//! changing anything here is a breaking change for the whole workspace.
//!
//! The schema is TPC-H's `lineitem` table, flattened into sixteen columns.
//! Two deliberate simplifications are worth stating plainly:
//!
//! - **Dates are `Utf8`, not `Date32`.** They are formatted `YYYY-MM-DD`.
//!   The engine models strings, not dates; claiming a richer type system
//!   than is implemented would be a claim rather than evidence.
//! - **The dictionary-encoded string columns are dictionary-encoded on
//!   disk.** A columnar engine without dictionary-encoded strings is a
//!   numeric toy, and the Q6 example filters `l_shipdate`.

use arrow::datatypes::{DataType as ArrowDataType, Field, Fields, Schema, SchemaRef};
use columnar_core::DataType;
use std::sync::Arc;

/// Rows generated per unit of scale factor.
///
/// SF-1 is 6,000,000 rows; SF-10 is 60,000,000.
pub const ROWS_PER_SCALE_FACTOR: u64 = 6_000_000;

/// The `l_comment` column: the one high-cardinality string column.
pub const COMMENT_COLUMN: &str = "l_comment";

/// The number of rows in a dataset of the given scale factor.
///
/// The row count is exactly `6_000_000 * scale_factor`.
#[must_use]
pub fn rows_for_scale_factor(scale_factor: u32) -> u64 {
    ROWS_PER_SCALE_FACTOR.saturating_mul(u64::from(scale_factor))
}

/// One column of the dataset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnSpec {
    /// The column name, as it appears in the Parquet file.
    pub name: &'static str,
    /// The engine's physical type for the column.
    pub data_type: DataType,
    /// Whether the column is written dictionary-encoded.
    pub dictionary: bool,
}

/// The `lineitem` columns, in schema order.
///
/// This is the contract. The order here is the order in the Parquet file.
pub const COLUMNS: [ColumnSpec; 16] = [
    ColumnSpec { name: "l_orderkey", data_type: DataType::Int64, dictionary: false },
    ColumnSpec { name: "l_partkey", data_type: DataType::Int64, dictionary: false },
    ColumnSpec { name: "l_suppkey", data_type: DataType::Int64, dictionary: false },
    ColumnSpec { name: "l_linenumber", data_type: DataType::Int64, dictionary: false },
    ColumnSpec { name: "l_quantity", data_type: DataType::Float64, dictionary: false },
    ColumnSpec { name: "l_extendedprice", data_type: DataType::Float64, dictionary: false },
    ColumnSpec { name: "l_discount", data_type: DataType::Float64, dictionary: false },
    ColumnSpec { name: "l_tax", data_type: DataType::Float64, dictionary: false },
    ColumnSpec { name: "l_returnflag", data_type: DataType::Utf8, dictionary: true },
    ColumnSpec { name: "l_linestatus", data_type: DataType::Utf8, dictionary: true },
    ColumnSpec { name: "l_shipdate", data_type: DataType::Utf8, dictionary: true },
    ColumnSpec { name: "l_commitdate", data_type: DataType::Utf8, dictionary: true },
    ColumnSpec { name: "l_receiptdate", data_type: DataType::Utf8, dictionary: true },
    ColumnSpec { name: "l_shipinstruct", data_type: DataType::Utf8, dictionary: true },
    ColumnSpec { name: "l_shipmode", data_type: DataType::Utf8, dictionary: true },
    ColumnSpec { name: "l_comment", data_type: DataType::Utf8, dictionary: false },
];

/// The names of the dictionary-encoded columns, in schema order.
pub const DICTIONARY_COLUMNS: [&str; 7] = [
    "l_returnflag",
    "l_linestatus",
    "l_shipdate",
    "l_commitdate",
    "l_receiptdate",
    "l_shipinstruct",
    "l_shipmode",
];

/// The column names, in schema order.
#[must_use]
pub fn column_names() -> Vec<String> {
    COLUMNS.iter().map(|column| column.name.to_string()).collect()
}

/// The engine's physical type for a column, by name.
#[must_use]
pub fn engine_data_type(name: &str) -> Option<DataType> {
    COLUMNS.iter().find(|column| column.name == name).map(|column| column.data_type)
}

/// Whether a column is written dictionary-encoded.
#[must_use]
pub fn is_dictionary_column(name: &str) -> bool {
    COLUMNS.iter().find(|column| column.name == name).is_some_and(|column| column.dictionary)
}

/// The Arrow type of a column.
///
/// Dictionary columns become `Dictionary(Int32, Utf8)`; everything else
/// maps to its natural Arrow type. The dictionary key type is `Int32`
/// because that is what the Parquet dictionary encoding stores.
#[must_use]
pub fn arrow_data_type(column: &ColumnSpec) -> ArrowDataType {
    match (column.data_type, column.dictionary) {
        (DataType::Int64, _) => ArrowDataType::Int64,
        (DataType::Float64, _) => ArrowDataType::Float64,
        (DataType::Utf8, false) => ArrowDataType::Utf8,
        (DataType::Utf8, true) => {
            ArrowDataType::Dictionary(Box::new(ArrowDataType::Int32), Box::new(ArrowDataType::Utf8))
        }
    }
}

/// The Arrow schema of the dataset.
#[must_use]
pub fn arrow_schema() -> SchemaRef {
    let fields: Vec<Field> = COLUMNS
        .iter()
        .map(|column| {
            Field::new(
                column.name,
                arrow_data_type(column),
                // `l_discount` and `l_comment` are the only nullable columns;
                // the schema says so, so the reader does not have to guess.
                matches!(column.name, "l_discount" | "l_comment"),
            )
        })
        .collect();
    Arc::new(Schema::new(Fields::from(fields)))
}
