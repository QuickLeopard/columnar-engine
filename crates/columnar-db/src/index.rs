//! The index: the key, and the files that hold it.
//!
//! **And the index is the snapshot's, and not the database's.** A snapshot
//! pins the file list it saw, and a write that lands afterwards is a write
//! the snapshot does not see — **so an index the writer maintained would name
//! files the snapshot does not have**, and a lookup would answer with rows
//! the snapshot must not see. **So the index is derived from the snapshot's
//! own files**, and it is built when the snapshot is first asked for one.
//!
//! **And the write path pays nothing for it.** The index is derived, and the
//! price of deriving it is paid by the reader — once, at the first lookup —
//! and not by the writer.
//!
//! **And the map is the shape the figure decided**: a map from the key to the
//! files that hold it, many-to-one, over the grid
//! ([ADR-0068](../../docs/adr/0068-the-indexs-shape-and-its-price.md)). **And
//! a key's files are the files that hold a row with that key**, so the answer
//! is the same rows the scan would answer, and only the files read differ.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use columnar_engine::{ExecOptions, FileSetSource, QueryPlan, ScalarValue};

use crate::error::{Error, Result};
use crate::row::values_of;

/// The index: the key, and the files that hold it.
#[derive(Debug, Default)]
pub(crate) struct Index {
    /// The key to the files that hold it.
    entries: HashMap<Key, Vec<usize>>,
}

impl Index {
    /// The index of a table's files, over its key column.
    ///
    /// **And every file's key column is read**, because the index is derived
    /// and the files are the truth. **And the files are read in order**, so
    /// the file ids are the table's own order.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] and [`Error::Engine`] when a file cannot be
    /// read, and [`Error::Corrupt`] when a file's rows are not readable.
    pub(crate) fn build(files: &[PathBuf], column: &str) -> Result<Self> {
        let mut entries: HashMap<Key, Vec<usize>> = HashMap::new();
        for (index, file) in files.iter().enumerate() {
            for key in keys_of(file, column)? {
                entries.entry(key).or_default().push(index);
            }
        }
        Ok(Self { entries })
    }

    /// The files that hold a key.
    ///
    /// **And the answer is empty when no file holds it**, which is the answer
    /// the scan gives too: no row holds the key, so no row is returned.
    pub(crate) fn files_of(&self, key: &Key) -> &[usize] {
        self.entries.get(key).map_or(&[], Vec::as_slice)
    }
}

/// The keys of one file, in the file's order.
///
/// **And only the key column is read**: the index needs the keys and nothing
/// else, and a file's other columns are the lookup's business and not the
/// index's.
fn keys_of(file: &Path, column: &str) -> Result<Vec<Key>> {
    let source = FileSetSource::open(&[file.to_path_buf()])?;
    let plan = QueryPlan::new(Vec::new(), vec![column.to_owned()], None);
    let dataset = source.execute_rows(&plan, &ExecOptions::default())?;
    let chunk = dataset.chunk(column).ok_or_else(|| Error::Corrupt {
        column: column.to_owned(),
        reason: "the file does not hold the column it is indexed on".to_owned(),
    })?;
    let values = values_of(chunk)?;
    let mut keys = Vec::with_capacity(values.len());
    for value in values {
        let Some(value) = value else {
            continue;
        };
        keys.push(Key::of(&value, column)?);
    }
    Ok(keys)
}

/// The index's own key: a value of the engine's vocabulary.
///
/// **And a float is keyed by its bits**, because a float is not `Eq` and a map
/// needs one: the bits are the value, and the same bits are the same key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Key {
    /// An integer key.
    Int64(i64),
    /// A floating-point key, by its bits.
    Float64(u64),
    /// A text key.
    Utf8(String),
}

impl Key {
    /// The key a value is.
    ///
    /// **And a value the engine's vocabulary does not have is refused**: the
    /// index's key is the engine's own vocabulary and not a second one.
    pub(crate) fn of(value: &ScalarValue, column: &str) -> Result<Self> {
        match value {
            ScalarValue::Int64(number) => Ok(Self::Int64(*number)),
            ScalarValue::Float64(number) => Ok(Self::Float64(number.to_bits())),
            ScalarValue::Utf8(text) => Ok(Self::Utf8(text.clone())),
            other => Err(Error::Corrupt {
                column: column.to_owned(),
                reason: format!(
                    "the value is a {}, and the index has no key for it",
                    other.data_type().name()
                ),
            }),
        }
    }
}
