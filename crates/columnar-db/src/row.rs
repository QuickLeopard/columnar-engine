//! A row: the values a table's columns hold.

use columnar_engine::{ColumnChunk, Dataset, Encoding, ScalarValue};

use crate::error::{Error, Result};
use crate::schema::Schema;

/// A row: one value per column, in the columns' order.
///
/// **A NULL is the absence of a value, and not a value of its own.** The
/// engine records a NULL by the absence of the validity bit, and a row here
/// says the same thing with an `Option` — so there is no sentinel and no
/// value that means "nothing".
///
/// **And the values are the engine's `ScalarValue`**, because the database's
/// vocabulary is the engine's. A second vocabulary would be a conversion at
/// every boundary and a place for the two to disagree.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    values: Vec<Option<ScalarValue>>,
}

impl Row {
    /// A row with those values, where `None` is a NULL.
    #[must_use]
    pub fn new(values: Vec<Option<ScalarValue>>) -> Self {
        Self { values }
    }

    /// A row with those values, and no NULL in it.
    #[must_use]
    pub fn filled(values: Vec<ScalarValue>) -> Self {
        Self { values: values.into_iter().map(Some).collect() }
    }

    /// How many values the row holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Whether the row holds no values.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// The value at that position, if the row has one.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&Option<ScalarValue>> {
        self.values.get(index)
    }

    /// The row's values.
    #[must_use]
    pub fn values(&self) -> &[Option<ScalarValue>] {
        &self.values
    }
}

/// The rows a plan produced, and the columns they hold.
///
/// **The columns are named beside the rows**, so a plan that projected a
/// subset of a table says which subset it got — and the rows' values are in
/// those columns' order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Rows {
    columns: Vec<String>,
    rows: Vec<Row>,
}

impl Rows {
    /// The rows a dataset holds, in the dataset's own columns.
    ///
    /// **Every encoding the engine has is read here**, the dictionaries
    /// included: a row is a row whichever way its column was stored.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Corrupt`] when the dataset does not hold a column it
    /// names, or when a column's rows are not readable.
    pub(crate) fn of(dataset: &Dataset) -> Result<Self> {
        let columns = dataset.column_names();
        let mut per_column = Vec::with_capacity(columns.len());
        for name in &columns {
            let chunk = dataset.chunk(name).ok_or_else(|| Error::Corrupt {
                column: name.clone(),
                reason: "the dataset does not hold the column it names".to_owned(),
            })?;
            per_column.push(values_of(chunk)?);
        }
        let mut rows = Vec::with_capacity(dataset.len());
        for row in 0..dataset.len() {
            let values =
                per_column.iter().map(|column| column.get(row).cloned().flatten()).collect();
            rows.push(Row::new(values));
        }
        Ok(Self { columns, rows })
    }

    /// The rows of a table that has no rows: the table's columns, and nothing
    /// in them.
    pub(crate) fn empty(schema: &Schema) -> Self {
        Self { columns: schema.names().map(str::to_owned).collect(), rows: Vec::new() }
    }

    /// The columns the rows hold.
    #[must_use]
    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    /// The rows.
    #[must_use]
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The row at that position.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&Row> {
        self.rows.get(index)
    }

    /// How many rows there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether there are no rows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// The values of a column chunk, in the database's own shape.
fn values_of(chunk: &ColumnChunk) -> Result<Vec<Option<ScalarValue>>> {
    let column = &chunk.name;
    let values: Vec<Option<ScalarValue>> = match &chunk.encoding {
        Encoding::Int64 { values } => {
            values.iter().map(|value| Some(ScalarValue::Int64(*value))).collect()
        }
        Encoding::Float64 { values } => {
            values.iter().map(|value| Some(ScalarValue::Float64(*value))).collect()
        }
        Encoding::Utf8 { values } => {
            values.iter().map(|value| Some(ScalarValue::Utf8(value.clone()))).collect()
        }
        Encoding::Int64Dict { dict, indices } => {
            let mut out = Vec::with_capacity(indices.len());
            for index in indices {
                let value = dict_entry(column, dict, *index)?;
                out.push(Some(ScalarValue::Int64(value)));
            }
            out
        }
        Encoding::Float64Dict { dict, indices } => {
            let mut out = Vec::with_capacity(indices.len());
            for index in indices {
                let value = dict_entry(column, dict, *index)?;
                out.push(Some(ScalarValue::Float64(value)));
            }
            out
        }
        Encoding::Utf8Dict { dict, indices } => {
            let mut out = Vec::with_capacity(indices.len());
            for index in indices {
                let value = dict_entry(column, dict, *index)?;
                out.push(Some(ScalarValue::Utf8(value)));
            }
            out
        }
        Encoding::BoolRle { runs } => {
            let mut out = Vec::new();
            for &(value, count) in runs {
                let number = i64::from(value);
                out.extend(std::iter::repeat_n(Some(ScalarValue::Int64(number)), count as usize));
            }
            out
        }
    };
    // **A NULL is the absence of the value**, so the mask decides.
    Ok(values
        .into_iter()
        .enumerate()
        .map(|(row, value)| if chunk.validity.is_valid(row) { value } else { None })
        .collect())
}

/// The dictionary entry an index names.
fn dict_entry<T>(column: &str, dict: &[T], index: i32) -> Result<T>
where
    T: Clone,
{
    let position = usize::try_from(index).map_err(|_| Error::Corrupt {
        column: column.to_owned(),
        reason: format!("the dictionary index {index} is negative"),
    })?;
    dict.get(position).cloned().ok_or_else(|| Error::Corrupt {
        column: column.to_owned(),
        reason: format!("the dictionary index {index} is out of range"),
    })
}
