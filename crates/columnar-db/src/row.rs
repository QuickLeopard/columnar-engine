//! A row: the values a table's columns hold.

use columnar_engine::ScalarValue;

/// A row: one value per column, in the schema's order.
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
