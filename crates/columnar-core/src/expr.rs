//! Arithmetic expressions over columns and constants.
//!
//! ## Why this exists
//!
//! TPC-H's Q6 is `sum(l_extendedprice * (1 - l_discount))`: an arithmetic
//! expression, summed. An engine whose aggregate can only sum a column cannot
//! run it, and a repository that measures "Q6's predicate with `count(*)`"
//! and calls it Q6 is measuring something else.
//!
//! So the plan's surface grows a second kind of expression, next to the
//! predicate: a tree of constants, column references and arithmetic
//! operators.
//!
//! ## What it is not
//!
//! This is **not** a general expression language. There are no comparisons in
//! an expression, no `CASE`, no function calls, no casts, and no subqueries. An
//! expression computes a number, and that is the whole of it. The narrowness
//! is the feature: the type rules below are four lines long because there are
//! four operators, and every one of them is checkable.
//!
//! ## The type rules
//!
//! An expression's type is `int64` only if **every** operand is `int64`, and
//! `float64` otherwise. There is no implicit cast in the other direction: an
//! `int64` in a `float64` expression is widened, and a `float64` in an
//! `int64` expression is not. The engine's predicate follows the same rule —
//! an `int64` column compared against a `float64` threshold is a plan error —
//! because an implicit cast is where a query planner starts lying about what
//! it did.
//!
//! Division is the one operator with a rule of its own, and it has it because
//! the engines disagree: `5 / 2` is `2` in `PostgreSQL` and `2.5` in `DuckDB`.
//! The engine refuses integer division rather than pick a side. See
//! [`ArithOp::Div`].

use crate::types::ScalarValue;

/// An arithmetic operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArithOp {
    /// Addition.
    Add,
    /// Subtraction.
    Sub,
    /// Multiplication.
    Mul,
    /// Division.
    ///
    /// Division requires at least one `float64` operand. The engines disagree
    /// about integer division — `5 / 2` is `2` in `PostgreSQL` and `2.5` in
    /// `DuckDB` — and an engine that silently picks one of them is an engine
    /// whose answers are not portable. The engine refuses it instead, and says
    /// why.
    Div,
}

impl ArithOp {
    /// The operator's symbol, as it appears in the plan output.
    #[must_use]
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
        }
    }
}

/// A scalar arithmetic expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// A literal value.
    Constant(ScalarValue),
    /// A reference to a column, by name.
    Column(String),
    /// An arithmetic operation over two expressions.
    Binary {
        /// The operator.
        op: ArithOp,
        /// The left operand.
        left: Box<Expr>,
        /// The right operand.
        right: Box<Expr>,
    },
    /// The negation of an expression.
    Negate(Box<Expr>),
}

impl Expr {
    /// A reference to a column.
    #[must_use]
    pub fn column(name: impl Into<String>) -> Self {
        Self::Column(name.into())
    }

    /// A literal value.
    #[must_use]
    pub fn constant(value: impl Into<ScalarValue>) -> Self {
        Self::Constant(value.into())
    }

    /// An arithmetic operation over two expressions.
    #[must_use]
    pub fn binary(op: ArithOp, left: Expr, right: Expr) -> Self {
        Self::Binary { op, left: Box::new(left), right: Box::new(right) }
    }

    /// The negation of an expression.
    #[must_use]
    pub fn negate(expr: Expr) -> Self {
        Self::Negate(Box::new(expr))
    }

    /// The columns this expression reads, in the order they are first read.
    ///
    /// A column that appears twice is listed once: the expression reads the
    /// column once and uses it twice.
    #[must_use]
    pub fn columns(&self) -> Vec<&str> {
        let mut columns: Vec<&str> = Vec::new();
        self.collect_columns(&mut columns);
        columns
    }

    /// Collects the columns this expression reads.
    fn collect_columns<'a>(&'a self, columns: &mut Vec<&'a str>) {
        match self {
            Self::Constant(_) => {}
            Self::Column(name) => {
                if !columns.contains(&name.as_str()) {
                    columns.push(name.as_str());
                }
            }
            Self::Binary { left, right, .. } => {
                left.collect_columns(columns);
                right.collect_columns(columns);
            }
            Self::Negate(inner) => inner.collect_columns(columns),
        }
    }

    /// The expression as it appears in the plan output.
    ///
    /// The rendering is SQL-shaped, because the plan output is read by a
    /// person checking that the engine ran the query they meant. A binary
    /// operation is parenthesised, so `a * (1 - b)` reads as it is written and
    /// not as `a * 1 - b`.
    #[must_use]
    pub fn to_sql(&self) -> String {
        match self {
            Self::Constant(value) => value.to_string(),
            Self::Column(name) => name.clone(),
            Self::Binary { op, left, right } => {
                format!("({} {} {})", left.to_sql(), op.symbol(), right.to_sql())
            }
            Self::Negate(inner) => format!("(-{})", inner.to_sql()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ArithOp, Expr};

    #[test]
    fn the_columns_an_expression_reads_are_listed_once_each() {
        let expr = Expr::binary(
            ArithOp::Mul,
            Expr::column("a"),
            Expr::binary(ArithOp::Sub, Expr::constant(1.0), Expr::column("b")),
        );
        assert_eq!(expr.columns(), vec!["a", "b"]);
    }

    #[test]
    fn a_column_read_twice_is_listed_once() {
        let expr = Expr::binary(ArithOp::Add, Expr::column("a"), Expr::column("a"));
        assert_eq!(expr.columns(), vec!["a"]);
    }

    #[test]
    fn the_sql_rendering_is_parenthesised() {
        let expr = Expr::binary(
            ArithOp::Mul,
            Expr::column("l_extendedprice"),
            Expr::binary(ArithOp::Sub, Expr::constant(1.0), Expr::column("l_discount")),
        );
        assert_eq!(
            expr.to_sql(),
            "(l_extendedprice * (1 - l_discount))",
            "the rendering must not read as a different expression"
        );
    }

    #[test]
    fn a_negation_renders_with_a_minus() {
        assert_eq!(Expr::negate(Expr::column("x")).to_sql(), "(-x)");
    }
}
