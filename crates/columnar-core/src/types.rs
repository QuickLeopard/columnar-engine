//! Scalar types understood by the engine.
//!
//! The engine is deliberately narrow: three physical types, no `NULL` as a
//! value. A NULL is an *absence* recorded in a validity mask, never a
//! sentinel inside a value array. That distinction is the difference between
//! a columnar engine and a vectorised CSV reader, and it is what makes the
//! comparison kernels branchless on the null path.
//!
//! Type strictness is a feature, not a limitation. A `Float64` column
//! compared against an `Int64` threshold is a *plan* error, not something to
//! paper over with an implicit cast: the implicit cast is where a query
//! planner starts lying about what it did.

/// The physical type of a column.
///
/// This is a *physical* type, not a logical one. There is no `Decimal` and no
/// `Date`: the TPC-H generator materialises decimals as `Float64` and dates as
/// `Int32` days, and says so in the dataset manifest. Claiming a richer type
/// system than the engine implements would be a claim, not evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataType {
    /// 64-bit signed integer.
    Int64,
    /// 64-bit IEEE-754 double.
    Float64,
    /// UTF-8 string. Stored dictionary-encoded; the dictionary holds the
    /// distinct values and the column holds 32-bit indices into it.
    Utf8,
}

impl DataType {
    /// A short, stable name used in the query plan output and the dataset
    /// manifest. Deliberately not `Display`-formatted from the enum variant:
    /// the plan output is read by humans and diffed in CI, so the spelling
    /// must not change when the enum is reordered.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Int64 => "int64",
            Self::Float64 => "float64",
            Self::Utf8 => "utf8",
        }
    }
}

/// A single scalar value.
///
/// Used for predicate thresholds and for aggregate outputs. It carries a
/// `Utf8` variant with an owned `String` rather than a `&str` because a
/// predicate threshold is stored in the plan and may outlive the batch it
/// was built against.
#[derive(Debug, Clone, PartialEq)]
pub enum ScalarValue {
    /// A 64-bit signed integer.
    Int64(i64),
    /// A 64-bit double.
    Float64(f64),
    /// A UTF-8 string.
    Utf8(String),
}

impl ScalarValue {
    /// The physical type of this value.
    #[must_use]
    pub const fn data_type(&self) -> DataType {
        match self {
            Self::Int64(_) => DataType::Int64,
            Self::Float64(_) => DataType::Float64,
            Self::Utf8(_) => DataType::Utf8,
        }
    }
}

impl From<i64> for ScalarValue {
    fn from(v: i64) -> Self {
        Self::Int64(v)
    }
}

impl From<f64> for ScalarValue {
    fn from(v: f64) -> Self {
        Self::Float64(v)
    }
}

impl From<&str> for ScalarValue {
    fn from(v: &str) -> Self {
        Self::Utf8(v.to_owned())
    }
}

impl std::fmt::Display for ScalarValue {
    /// Writes the value the way a plan output reads it.
    ///
    /// A string is quoted, so that a threshold of `'1994-01-01'` is not
    /// mistaken for a number, and a float is written with Rust's shortest
    /// round-tripping form, so the output says the number that was actually
    /// compared against.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Int64(v) => write!(f, "{v}"),
            Self::Float64(v) => write!(f, "{v}"),
            Self::Utf8(v) => write!(f, "'{v}'"),
        }
    }
}
