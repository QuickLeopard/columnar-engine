//! The schema: a table's columns, their types, and their nullability.

use serde::de::Error as _;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use columnar_engine::DataType;

use crate::error::{Error, Result};

/// The engine's stable name for a 64-bit integer.
const INT64: &str = "int64";
/// The engine's stable name for a 64-bit double.
const FLOAT64: &str = "float64";
/// The engine's stable name for a UTF-8 string.
const UTF8: &str = "utf8";

/// A table's schema: its columns, in order.
///
/// **The vocabulary is the engine's**, and it is three types wide: `Int64`,
/// `Float64` and `Utf8`. The database does not add a type of its own, because
/// a second vocabulary is a conversion at every boundary and a place for the
/// two to disagree. **A date is a `Utf8`, a decimal is a `Float64`, and a
/// boolean is an `Int64`** — the three names are the whole of it, and a
/// database that needs more has to widen the engine's vocabulary first, where
/// the cost is measured.
///
/// # The nullability is the schema's, and the file's is the store's
///
/// A column's `nullable` is the **schema's promise**: the column may hold a
/// `NULL`. The **file's** own optionality is the store's rule, and it is
/// decided by the data rather than by the schema — a column that holds no
/// `NULL` in a batch is written REQUIRED, and one that holds one is written
/// OPTIONAL. **So a table's declared nullability and its files' optionality
/// are two different things**, and they are two different things on purpose:
/// the schema is what the application promised, and the file is what the
/// rows in it happened to be.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schema {
    columns: Vec<Column>,
}

impl Schema {
    /// A schema for the given columns.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Schema`] when the schema has no columns, when a
    /// column's name is empty, or when two columns share a name.
    pub fn new(columns: Vec<Column>) -> Result<Self> {
        if columns.is_empty() {
            return Err(Error::Schema { reason: "a table has no columns".to_owned() });
        }
        let mut names: Vec<&str> = Vec::with_capacity(columns.len());
        for column in &columns {
            if column.name.is_empty() {
                return Err(Error::Schema { reason: "a column has no name".to_owned() });
            }
            if names.contains(&column.name.as_str()) {
                return Err(Error::Schema {
                    reason: format!("two columns are named {}", column.name),
                });
            }
            names.push(column.name.as_str());
        }
        Ok(Self { columns })
    }

    /// The schema's columns, in order.
    #[must_use]
    pub fn columns(&self) -> &[Column] {
        &self.columns
    }

    /// The schema's column names, in order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.columns.iter().map(Column::name)
    }

    /// The column with that name, if the schema has one.
    #[must_use]
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|column| column.name == name)
    }

    /// How many columns the schema has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.columns.len()
    }

    /// Whether the schema has no columns. A schema always has at least one,
    /// so this is always false — and it is here because the length is.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }
}

/// A column: its name, its type, and whether it may hold a `NULL`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    name: String,
    data_type: DataType,
    nullable: bool,
}

impl Column {
    /// A column with that name, that type, and that nullability.
    #[must_use]
    pub fn new(name: impl Into<String>, data_type: DataType, nullable: bool) -> Self {
        Self { name: name.into(), data_type, nullable }
    }

    /// The column's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The column's type.
    #[must_use]
    pub fn data_type(&self) -> DataType {
        self.data_type
    }

    /// Whether the column may hold a `NULL`.
    #[must_use]
    pub fn is_nullable(&self) -> bool {
        self.nullable
    }
}

/// The column as it is written: the type is the engine's stable name.
///
/// **The name is the engine's own** — `int64`, `float64`, `utf8` — and not the
/// enum's variant name, because the engine's `name` is the spelling it
/// promises to keep stable in the plan output and the dataset manifest. A
/// catalog that spelled the type differently from the engine would be a
/// second vocabulary, which is the thing this crate refuses to have.
impl Serialize for Column {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("Column", 3)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("type", self.data_type.name())?;
        state.serialize_field("nullable", &self.nullable)?;
        state.end()
    }
}

/// The column as it is read: the type is the engine's stable name, and a name
/// the engine does not have is a catalog this database cannot read.
impl<'de> Deserialize<'de> for Column {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawColumn::deserialize(deserializer)?;
        let data_type = data_type_from_name(&raw.data_type).ok_or_else(|| {
            D::Error::custom(format!("the type {} is not one the engine has", raw.data_type))
        })?;
        Ok(Self { name: raw.name, data_type, nullable: raw.nullable })
    }
}

/// The column as it sits in the JSON.
#[derive(Debug, Deserialize)]
struct RawColumn {
    name: String,
    #[serde(rename = "type")]
    data_type: String,
    nullable: bool,
}

/// The engine's type by its stable name.
fn data_type_from_name(name: &str) -> Option<DataType> {
    match name {
        INT64 => Some(DataType::Int64),
        FLOAT64 => Some(DataType::Float64),
        UTF8 => Some(DataType::Utf8),
        _ => None,
    }
}
