//! The dataset schema: the contract shared with the engine.
//!
//! The column list, the column order and the column types are a hard
//! contract. `columnar-engine` is written against exactly this layout, so
//! changing anything here is a breaking change for the whole workspace.
//!
//! The dataset holds **two** tables: TPC-H's `lineitem`, flattened into
//! sixteen columns, and TPC-H's `orders`, flattened into nine. The second
//! table is the build side of the join: it is a quarter of the size of
//! `lineitem`, which is what makes it the side worth materialising.
//!
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

/// Rows in the `orders` table per unit of scale factor.
///
/// TPC-H's `orders` is a quarter of its `lineitem`: 1,500,000 rows at SF-1
/// against `lineitem`'s 6,000,000. The ratio is the same at every scale
/// factor, and it is the ratio the join's match rate is built on.
pub const ORDERS_ROWS_PER_SCALE_FACTOR: u64 = 1_500_000;

/// The `o_comment` column: the one high-cardinality string column of the
/// `orders` table, and the only nullable one.
pub const ORDERS_COMMENT_COLUMN: &str = "o_comment";

/// The number of rows in the `orders` table of the given scale factor.
///
/// The row count is exactly `1_500_000 * scale_factor`.
#[must_use]
pub fn orders_rows_for_scale_factor(scale_factor: u32) -> u64 {
    ORDERS_ROWS_PER_SCALE_FACTOR.saturating_mul(u64::from(scale_factor))
}

/// The `orders` columns, in schema order.
///
/// This is the contract for the second table. The order here is the order in
/// the Parquet file, and `o_orderkey` is the join key: it is drawn from the
/// same key space as `l_orderkey`, so the join has a real selectivity.
pub const ORDERS_COLUMNS: [ColumnSpec; 9] = [
    ColumnSpec { name: "o_orderkey", data_type: DataType::Int64, dictionary: false },
    ColumnSpec { name: "o_custkey", data_type: DataType::Int64, dictionary: false },
    ColumnSpec { name: "o_orderstatus", data_type: DataType::Utf8, dictionary: true },
    ColumnSpec { name: "o_totalprice", data_type: DataType::Float64, dictionary: false },
    ColumnSpec { name: "o_orderdate", data_type: DataType::Utf8, dictionary: true },
    ColumnSpec { name: "o_orderpriority", data_type: DataType::Utf8, dictionary: true },
    ColumnSpec { name: "o_clerk", data_type: DataType::Int64, dictionary: false },
    ColumnSpec { name: "o_shippriority", data_type: DataType::Int64, dictionary: false },
    ColumnSpec { name: "o_comment", data_type: DataType::Utf8, dictionary: false },
];

/// The names of the dictionary-encoded `orders` columns, in schema order.
///
/// The same rule as `lineitem`: a column of few distinct values is
/// dictionary-encoded, and the one high-cardinality column is not. The date
/// column counts as few distinct values -- the `orders` dates cover the
/// same 2,550-day window as the `lineitem` ship dates -- so it is
/// dictionary-encoded here for the same reason it is there.
pub const ORDERS_DICTIONARY_COLUMNS: [&str; 3] =
    ["o_orderstatus", "o_orderdate", "o_orderpriority"];

/// The `orders` column names, in schema order.
#[must_use]
pub fn orders_column_names() -> Vec<String> {
    ORDERS_COLUMNS.iter().map(|column| column.name.to_string()).collect()
}

/// The engine's physical type for an `orders` column, by name.
#[must_use]
pub fn orders_engine_data_type(name: &str) -> Option<DataType> {
    ORDERS_COLUMNS.iter().find(|column| column.name == name).map(|column| column.data_type)
}

/// Whether an `orders` column is written dictionary-encoded.
#[must_use]
pub fn orders_is_dictionary_column(name: &str) -> bool {
    ORDERS_COLUMNS.iter().find(|column| column.name == name).is_some_and(|column| column.dictionary)
}

/// The Arrow schema of the `orders` table.
#[must_use]
pub fn orders_arrow_schema() -> SchemaRef {
    let fields: Vec<Field> = ORDERS_COLUMNS
        .iter()
        .map(|column| {
            Field::new(
                column.name,
                arrow_data_type(column),
                // `o_comment` is the only nullable column of this table.
                column.name == ORDERS_COMMENT_COLUMN,
            )
        })
        .collect();
    Arc::new(Schema::new(Fields::from(fields)))
}
