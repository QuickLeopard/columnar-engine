//! The prelude: the surface, in one import.
//!
//! ```
//! use columnar_engine::prelude::*;
//! ```
//!
//! The prelude carries **exactly** the surface — the same list the crate root
//! re-exports, and the list in
//! [`spec 034`](../../../specs/034-the-engine-as-a-library/spec.md)'s *The surface,
//! exactly*. A name that is not in it is reachable, because this repository's
//! measurement harness lives in the same crate, and it is **not** part of the
//! surface, because it is not promised.

pub use crate::facade::*;
