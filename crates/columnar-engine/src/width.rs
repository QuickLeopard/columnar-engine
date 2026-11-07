//! What a row costs.
//!
//! **One statement, read by the estimate and by the reader's batch rule.**
//!
//! The engine holds a row in one of three shapes, and each has a price:
//!
//! | the shape | the row holds | the bytes the holder takes |
//! |---|---|---|
//! | a fixed-width value — an `int64` or a `float64` | the value itself | **8** |
//! | a dictionary's index | an `i32` into the column's dictionary | **4** |
//! | a plain string | a `String` | **24** — the header |
//!
//! **And the value's own bytes ride on top.** A plain string's heap holds the
//! string; a dictionary holds its distinct values **once** for the whole
//! column, so it costs nothing a row; and a number's eight bytes **are** its
//! value, so nothing rides on top of it.
//!
//! ## Why the encoding is the question, and not the type
//!
//! A `utf8` column is **two** prices: **24** when the rows are plain strings
//! and **4** when the rows are a dictionary's indices. **The type does not say
//! which**, and the file's footer does not either — a chunk's `encodings` list
//! names `RLE_DICTIONARY` for a column that is not dictionary-encoded at all.
//!
//! **The schema's own Arrow type does.** A `Dictionary(_, Utf8)` field is a
//! column of indices, and a `Utf8` field is a column of headers. **And it is
//! the same signal the reader's own arrays answer to**: the engine's
//! `Utf8Dict` chunk is built from a `DictionaryArray` and its `Utf8` chunk
//! from a `StringArray`, so the schema says which one the read will make.
//!
//! ## The data's own bytes, and where they come from
//!
//! The value's own bytes are **the data's own size**, and they are not known
//! before the read. **Two paths, two answers**:
//!
//! - The **eager** path has the chunk, so it reads the bytes off it: the
//!   strings' own lengths, or the dictionary's.
//! - The **lazy** path makes its estimate before a row is read, so it takes
//!   the file's own accounting of the column — the chunk's
//!   `total_uncompressed_size` over its `num_values` — **for the columns whose
//!   values the engine copies**, which are the strings. A **number's** eight
//!   bytes already are its value, so nothing is added for it.
//!
//! **The file's own accounting is not the statement.** It is the file's
//! **pages**, and the engine holds **decoded** values: the published file
//! stores `l_discount` in **0.50** bytes a row and the engine holds it in
//! **eight**. It is used here for the one thing it is good at — the size of a
//! column's own data — and the difference is published beside it.
//!
//! ## The statement is measured against
//!
//! A statement that is not measured against is a guess with a formula. The
//! measurement is in [§3ag](../../PERF.md#3ag-the-width-statement-paid): the
//! allocator's bytes a row, the statement's bytes a row, and the difference.

use arrow::datatypes::DataType as ArrowType;
use columnar_core::{DataType, Encoding};

/// The bytes an `int64` or a `float64` takes.
pub const FIXED64_BYTES: usize = 8;

/// The bytes a dictionary's index takes.
pub const INDEX_BYTES: usize = 4;

/// The bytes a `String`'s header takes.
pub const STRING_HEADER_BYTES: usize = 24;

/// How a column's values are stored.
///
/// This is the engine's own name for the three shapes a column's rows take,
/// and it is what the width statement is read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Storage {
    /// A fixed-width value — an `int64` or a `float64`.
    ///
    /// **Eight bytes a row**, and the value is all of it.
    Fixed64,
    /// A dictionary's **index**.
    ///
    /// **Four bytes a row**, and the dictionary's own values are shared by the
    /// whole column.
    Index,
    /// A plain string's **`String` header**.
    ///
    /// **Twenty-four bytes a row**, and the string's own bytes ride on top.
    StringHeader,
}

impl Storage {
    /// The bytes the row's **holder** takes, in the engine's own storage.
    #[must_use]
    pub const fn holder_bytes(self) -> usize {
        match self {
            Self::Fixed64 => FIXED64_BYTES,
            Self::Index => INDEX_BYTES,
            Self::StringHeader => STRING_HEADER_BYTES,
        }
    }

    /// The storage of a column of the given engine type.
    ///
    /// This is the answer when nothing else is known: a `utf8` column is
    /// charged a plain string's header, which is the **dearer** of the two
    /// prices a `utf8` column can have.
    #[must_use]
    pub const fn of_type(data_type: DataType) -> Self {
        match data_type {
            DataType::Int64 | DataType::Float64 => Self::Fixed64,
            DataType::Utf8 => Self::StringHeader,
        }
    }

    /// What a column in this storage costs, with `value` bytes of the value's
    /// own data on top.
    #[must_use]
    pub const fn cost(self, value: usize) -> RowCost {
        RowCost { holder: self.holder_bytes(), value }
    }
}

/// What one row of a column costs, in the engine's own storage.
///
/// The two parts are the row's **holder** and the row's **value**, and they
/// are kept apart because they are known at different times: the holder is
/// known from the schema, and the value's own bytes are known from the chunk
/// on the eager path and from the file's own accounting on the lazy one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowCost {
    /// The bytes one row's **holder** takes — **8**, **4** or **24**.
    pub holder: usize,
    /// The bytes the column's own **value** takes, in the engine's own
    /// storage, over the whole column.
    pub value: usize,
}

impl RowCost {
    /// The bytes one row of a column of this shape costs, over `rows` rows.
    ///
    /// **The value's own bytes are the column's**, so they are spread over the
    /// rows -- and **rounded up**, because the estimate is meant to be on the
    /// safe side of the truth.
    #[must_use]
    pub fn per_row(self, rows: usize) -> usize {
        let value = if rows == 0 { 0 } else { self.value.div_ceil(rows) };
        self.holder.saturating_add(value)
    }
}

/// The storage of a column of the given Arrow type.
///
/// **The schema's own type is the signal**, and not the chunk's encodings: a
/// chunk's `encodings` list names `RLE_DICTIONARY` for a column that is not
/// dictionary-encoded at all.
#[must_use]
pub(crate) fn storage_of(arrow_type: &ArrowType) -> Storage {
    match arrow_type {
        ArrowType::Int64 | ArrowType::Float64 => Storage::Fixed64,
        ArrowType::Dictionary(_, value) => match value.as_ref() {
            ArrowType::Int64 | ArrowType::Float64 | ArrowType::Utf8 | ArrowType::LargeUtf8 => {
                Storage::Index
            }
            _ => Storage::StringHeader,
        },
        _ => Storage::StringHeader,
    }
}

/// The storage of a column chunk the engine has decoded.
///
/// **The chunk's own encoding**, which is the same answer the schema gives
/// for a column the reader made: a `DictionaryArray` becomes a
/// `Utf8Dict` chunk, and a `StringArray` becomes a `Utf8` one.
#[must_use]
pub(crate) fn storage_of_encoding(encoding: &Encoding) -> Storage {
    match encoding {
        Encoding::Int64 { .. } | Encoding::Float64 { .. } | Encoding::BoolRle { .. } => {
            Storage::Fixed64
        }
        Encoding::Int64Dict { .. } | Encoding::Float64Dict { .. } | Encoding::Utf8Dict { .. } => {
            Storage::Index
        }
        Encoding::Utf8 { .. } => Storage::StringHeader,
    }
}

/// The value's own bytes, of a column chunk the engine has decoded.
///
/// **The eager path's answer**: the chunk's own value bytes — a plain string's
/// strings, or a dictionary's values. **A number's eight bytes already are
/// its value**, so nothing is added for it.
#[must_use]
pub(crate) fn value_bytes_of_encoding(encoding: &Encoding) -> usize {
    match encoding {
        Encoding::Int64 { .. } | Encoding::Float64 { .. } | Encoding::BoolRle { .. } => 0,
        Encoding::Utf8 { values } => values.iter().map(String::len).sum(),
        Encoding::Int64Dict { dict, .. } => dict.len().saturating_mul(8),
        Encoding::Float64Dict { dict, .. } => dict.len().saturating_mul(8),
        Encoding::Utf8Dict { dict, .. } => dict.iter().map(String::len).sum(),
    }
}
