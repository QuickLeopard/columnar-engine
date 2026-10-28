//! The two key sources, held to the same columns.
//!
//! The join's key lookup has **two** implementations: `KeyColumn`, over the
//! engine's own column, and `ArrowKeys`, over the reader's Arrow array. They
//! sit behind one interface, `KeySource`, and the interface says the rules --
//! but a shared interface is a **promise**, and a promise is not evidence.
//!
//! The engine's own claim is that the two paths cannot answer differently: the
//! cold path probes the join **inside** the reader, over Arrow's arrays, and
//! the resident path probes it over the engine's own column. The only test
//! that compared them used **integer, non-NULL** keys, which is the one case
//! where the two implementations have nothing to disagree about.
//!
//! This is the evidence. The same columns, read through both, value for value:
//! NULLs on either side, plain strings, dictionary-encoded strings, the
//! plain-integer fast path, and a row past the end.

use std::sync::Arc;

use arrow::array::{ArrayRef, DictionaryArray, Int32Array, Int64Array, StringArray};
use arrow::datatypes::Int32Type;
use columnar_core::{ColumnChunk, Encoding, NullBitmap};
use columnar_engine::operators::key::{ArrowKeys, KeyColumn, KeySource};

/// Reads every row of a column through both sources, and holds them to each other.
fn agree(name: &str, chunk: &ColumnChunk, array: &ArrayRef) {
    let column = KeyColumn::from_chunk(chunk);
    let keys = ArrowKeys::new(array.as_ref()).expect("the probe can read the column");

    assert_eq!(column.len(), keys.len(), "{name}: the two sources disagree about the length");
    assert_eq!(
        column.has_nulls(),
        keys.has_nulls(),
        "{name}: the two sources disagree about whether the column holds NULLs"
    );
    assert_eq!(
        column.integer_values(),
        keys.integer_values(),
        "{name}: the two sources disagree about the plain integers"
    );

    for row in 0..column.len() {
        assert_eq!(
            column.value_at(row),
            keys.value_at(row),
            "{name}: the two sources disagree about row {row}"
        );
    }

    // A row **past the end** has no value, on both paths. The engine's own
    // rows are in range by construction, so this is the case that cannot
    // arise -- and it is the case where the two used to differ: one returned
    // `None` and the other panicked.
    let past = column.len();
    assert_eq!(column.value_at(past), None, "{name}: a row past the end holds a value");
    assert_eq!(keys.value_at(past), None, "{name}: a row past the end holds a value");
}

/// Plain integers, with no NULLs.
#[test]
fn plain_integers_agree() {
    let values = vec![10_i64, 20, 30, 40];
    let chunk = ColumnChunk::new(
        "k",
        Encoding::Int64 { values: values.clone() },
        NullBitmap::with_nulls(values.len(), &[]),
    );
    let array: ArrayRef = Arc::new(Int64Array::from(values));
    agree("plain integers", &chunk, &array);
}

/// Plain integers, with a NULL in the middle.
#[test]
fn plain_integers_with_nulls_agree() {
    let values = vec![10_i64, 20, 30, 40];
    let chunk = ColumnChunk::new(
        "k",
        Encoding::Int64 { values: values.clone() },
        NullBitmap::with_nulls(values.len(), &[1]),
    );
    let array: ArrayRef = Arc::new(Int64Array::from(vec![Some(10), None, Some(30), Some(40)]));
    agree("plain integers with NULLs", &chunk, &array);
}

/// Plain strings, with and without NULLs.
#[test]
fn plain_strings_agree() {
    let values = vec!["alpha".to_owned(), "beta".to_owned(), "gamma".to_owned()];
    let chunk = ColumnChunk::new(
        "k",
        Encoding::Utf8 { values: values.clone() },
        NullBitmap::with_nulls(values.len(), &[]),
    );
    let array: ArrayRef =
        Arc::new(StringArray::from(values.iter().map(String::as_str).collect::<Vec<_>>()));
    agree("plain strings", &chunk, &array);

    let chunk = ColumnChunk::new(
        "k",
        Encoding::Utf8 { values: values.clone() },
        NullBitmap::with_nulls(values.len(), &[2]),
    );
    let array: ArrayRef = Arc::new(StringArray::from(vec![Some("alpha"), Some("beta"), None]));
    agree("plain strings with NULLs", &chunk, &array);
}

/// Dictionary-encoded strings, with and without NULLs.
///
/// The dictionary is where the two implementations differ most: the engine
/// holds its own `Vec<String>` and its own indices, and the reader holds
/// Arrow's. A row that is NULL must have no value on either side, and a row
/// that points into the dictionary must read the **same** string.
#[test]
fn dictionary_encoded_strings_agree() {
    let dict = vec!["alpha".to_owned(), "beta".to_owned(), "gamma".to_owned()];
    let indices = vec![0_i32, 2, 1, 0];

    let chunk = ColumnChunk::new(
        "k",
        Encoding::Utf8Dict { dict: dict.clone(), indices: indices.clone() },
        NullBitmap::with_nulls(indices.len(), &[]),
    );
    let array: ArrayRef = Arc::new(DictionaryArray::<Int32Type>::new(
        Int32Array::from(indices.clone()),
        Arc::new(StringArray::from(vec!["alpha", "beta", "gamma"])),
    ));
    agree("dictionary-encoded strings", &chunk, &array);

    let chunk = ColumnChunk::new(
        "k",
        Encoding::Utf8Dict { dict: dict.clone(), indices: indices.clone() },
        NullBitmap::with_nulls(indices.len(), &[1]),
    );
    let array: ArrayRef = Arc::new(DictionaryArray::<Int32Type>::new(
        Int32Array::from(vec![Some(0), None, Some(1), Some(0)]),
        Arc::new(StringArray::from(vec!["alpha", "beta", "gamma"])),
    ));
    agree("dictionary-encoded strings with NULLs", &chunk, &array);
}

/// Plain doubles, with a NULL.
#[test]
fn plain_doubles_with_nulls_agree() {
    let values = vec![1.5_f64, 2.5, 3.5];
    let chunk = ColumnChunk::new(
        "k",
        Encoding::Float64 { values: values.clone() },
        NullBitmap::with_nulls(values.len(), &[2]),
    );
    let array: ArrayRef =
        Arc::new(arrow::array::Float64Array::from(vec![Some(1.5), Some(2.5), None]));
    agree("plain doubles with NULLs", &chunk, &array);
}

/// An empty column.
#[test]
fn an_empty_column_agrees() {
    let chunk = ColumnChunk::new(
        "k",
        Encoding::Int64 { values: Vec::new() },
        NullBitmap::with_nulls(0, &[]),
    );
    let array: ArrayRef = Arc::new(Int64Array::from(Vec::<i64>::new()));
    agree("an empty column", &chunk, &array);
}
