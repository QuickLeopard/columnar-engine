//! Core vocabulary of the columnar engine.
//!
//! This crate is the shared language of the workspace. It holds the types
//! that the storage layer produces, the operators consume, and the kernels
//! are measured against:
//!
//! - [`types`] -- physical types and scalar values.
//! - [`null`] -- validity masks, where a NULL is an absent bit rather than a
//!   sentinel value.
//! - [`selection`] -- selection vectors, in both index and bitmap form, with
//!   the runtime dispatch between them.
//! - [`chunk`] -- column chunks and their encodings.
//! - [`batch`] -- batches, the unit of work.
//! - [`predicate`] -- the comparison predicates the engine understands.
//!
//! It has no dependencies, on purpose. See `constitution.md`, Principle I:
//! the vocabulary must not be able to drift, and a vocabulary with
//! dependencies is a vocabulary with a version history.

#![deny(missing_docs)]
#![deny(unsafe_code)]
// Bit-mask manipulation is indexing by nature: a validity mask is read at
// `words[row / 64]` and written at `words[row / 64]`, and there is no
// non-indexing spelling of that. Every entry point in this crate checks its
// bounds against the mask's length before indexing, and the boundary checks
// are what the tests exercise. The lint stays enabled for the rest of the
// workspace, where indexing is more likely to be a genuine bug.
#![allow(clippy::indexing_slicing)]

pub mod batch;
pub mod chunk;
pub mod expr;
pub mod null;
pub mod predicate;
pub mod selection;
pub mod types;

pub use batch::{Batch, BatchColumn, BatchData, BATCH_SIZE};
pub use chunk::{ColumnChunk, Encoding};
pub use expr::{ArithOp, Expr};
pub use null::NullBitmap;
pub use predicate::{ComparisonOp, Predicate};
pub use selection::{SelectionIter, SelectionKind, SelectionVector, BITMAP_DISPATCH_DENSITY};
pub use types::{DataType, ScalarValue};

/// The engine's version string, stamped into every published benchmark
/// result so that a number can be traced to the code that produced it.
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");
