//! The prelude: the surface, in one import.
//!
//! ```
//! use columnar_db::prelude::*;
//! ```

pub use crate::catalog::Catalog;
pub use crate::database::Database;
pub use crate::row::Row;
pub use crate::schema::{Column, Schema};
pub use crate::table::{Table, TableMut};

/// The engine's own type, re-exported so that a column can be declared
/// without a second dependency on the engine.
pub use columnar_engine::DataType;
