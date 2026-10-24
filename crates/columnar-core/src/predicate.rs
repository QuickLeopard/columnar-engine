//! Predicates.
//!
//! The engine's query surface is a fixed, typed set of comparison predicates
//! over a single column and a single threshold. There is no SQL parser.
//!
//! This is a deliberate scope decision, recorded in `specs/001-columnar-core`
//! and in the portfolio blueprint: a parser is a month of work that produces
//! no evidence, while a typed predicate set is a week and produces a
//! benchmark that a reviewer can re-run. The query surface is expressed in
//! Rust by the caller, and the plan output prints it so a reader can see
//! exactly what was executed.

/// A comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComparisonOp {
    /// Strictly less than.
    Lt,
    /// Less than or equal.
    LtEq,
    /// Strictly greater than.
    Gt,
    /// Greater than or equal.
    GtEq,
    /// Equal.
    Eq,
    /// Not equal.
    Ne,
}

impl ComparisonOp {
    /// The operator as it appears in the plan output.
    #[must_use]
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Lt => "<",
            Self::LtEq => "<=",
            Self::Gt => ">",
            Self::GtEq => ">=",
            Self::Eq => "=",
            Self::Ne => "!=",
        }
    }

    /// The operator's code in the C ABI shared with the kernel.
    ///
    /// The mapping is part of the ABI contract: the C++ kernel switches on
    /// these integers, and changing one without changing the other silently
    /// changes what the filter does. The values are asserted against the
    /// C++ header by a test.
    #[must_use]
    pub const fn abi_code(self) -> i32 {
        match self {
            Self::Lt => 0,
            Self::LtEq => 1,
            Self::Gt => 2,
            Self::GtEq => 3,
            Self::Eq => 4,
            Self::Ne => 5,
        }
    }

    /// Every operator, in ABI-code order. Used to fuzz the mapping.
    #[must_use]
    pub const fn all() -> [Self; 6] {
        [Self::Lt, Self::LtEq, Self::Gt, Self::GtEq, Self::Eq, Self::Ne]
    }
}

/// A filter over one column.
#[derive(Debug, Clone, PartialEq)]
pub struct Predicate {
    /// The name of the column to filter.
    pub column: String,
    /// The comparison to apply.
    pub op: ComparisonOp,
    /// The value to compare against.
    pub value: crate::types::ScalarValue,
}

impl Predicate {
    /// Builds a predicate.
    #[must_use]
    pub fn new(
        column: impl Into<String>,
        op: ComparisonOp,
        value: impl Into<crate::types::ScalarValue>,
    ) -> Self {
        Self { column: column.into(), op, value: value.into() }
    }
}
