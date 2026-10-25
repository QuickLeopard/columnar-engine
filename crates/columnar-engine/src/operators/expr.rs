//! The expression operator: an arithmetic expression, evaluated into a column.
//!
//! ## Why an expression becomes a column
//!
//! TPC-H's Q6 sums `l_extendedprice * (1 - l_discount)`. The engine's
//! aggregate sums a column. The gap between those two sentences is closed
//! here, and it is closed the way that leaves the aggregate alone: the
//! expression is **evaluated into a column** over the rows that survived the
//! filter, and that column is what the aggregate sums.
//!
//! A computed column is a column, so the aggregate's type rules, its NULL
//! handling, its saturation and its accumulator all keep working exactly as
//! they did, and the differential tests that cover them cover this path too.
//!
//! ## The rules, in one place
//!
//! - **NULL in, NULL out.** A row whose expression reads a NULL is a NULL
//!   row, and the sum skips it — SQL's rule, and the rule the engine's column
//!   sum already follows.
//! - **Arithmetic is IEEE-754.** `NaN` propagates, and so does `inf`. This is
//!   deliberately *unlike* the comparison rules, where a `NaN` never matches:
//!   a comparison is a question and a `NaN` has no answer, while arithmetic
//!   is a computation and `NaN` is a value. The two rules live in different
//!   places and are not allowed to leak into each other.
//! - **Integer arithmetic saturates**, matching the engine's integer
//!   accumulator. A wrapping sum would be a different answer on a different
//!   machine.
//! - **Division needs a float operand.** The plan rejects integer division
//!   before it reaches here, because the engines disagree about it.
//!
//! ## What this is not
//!
//! There is no vectorised path here. The expression is evaluated over the
//! rows that survived the filter, which is a fraction of the file, and the
//! measurement in `PERF.md` says exactly what the arithmetic costs. If it
//! turns out to be the next bottleneck, the next increment vectorises it —
//! and that is a decision a measurement makes, rather than a guess.

use columnar_core::{ArithOp, BatchColumn, BatchData, DataType, NullBitmap, ScalarValue};

use crate::plan::ResolvedExpr;
use crate::EngineError;

/// Evaluates an expression over the selected rows, into a column.
///
/// `column` fetches a column by its index in the dataset. The expression's
/// columns are the ones the plan materialised, so they are gathered by the
/// projection stage before this runs.
///
/// # Errors
///
/// Returns [`EngineError::TypeMismatch`] if the expression's type is not
/// numeric, which the plan's resolution already rejects — so this is a
/// belt-and-braces check on a hand-built [`ResolvedExpr`].
///
/// # Panics
///
/// Panics if `column` does not know one of the expression's columns.
pub fn evaluate<'a>(
    expr: &ResolvedExpr,
    column: &dyn Fn(usize) -> &'a BatchColumn,
    rows: usize,
) -> Result<BatchColumn, EngineError> {
    match expr.data_type() {
        DataType::Int64 => {
            let (values, validity) = eval_int(expr, column, rows);
            Ok(BatchColumn::with_validity(
                EXPRESSION_NAME.to_owned(),
                validity,
                BatchData::Int64(values),
            ))
        }
        DataType::Float64 => {
            let (values, validity) = eval_float(expr, column, rows);
            Ok(BatchColumn::with_validity(
                EXPRESSION_NAME.to_owned(),
                validity,
                BatchData::Float64(values),
            ))
        }
        DataType::Utf8 => Err(EngineError::TypeMismatch(
            "an arithmetic expression computes a number, and this one is a string".to_owned(),
        )),
    }
}

/// The name the computed column carries.
///
/// It never appears in the plan output's materialised-column list, because it
/// is not a column of the dataset: it is a column *of the query*.
const EXPRESSION_NAME: &str = "(expression)";

/// Evaluates an integer expression.
///
/// Returns the values and the validity mask. A NULL row's value is whatever
/// the arithmetic left in it; the validity mask says it is NULL, and the
/// aggregate skips it.
fn eval_int<'a>(
    expr: &ResolvedExpr,
    column: &dyn Fn(usize) -> &'a BatchColumn,
    rows: usize,
) -> (Vec<i64>, NullBitmap) {
    match expr {
        ResolvedExpr::Constant(value) => match value {
            ScalarValue::Int64(value) => (vec![*value; rows], NullBitmap::all_valid(rows)),
            _ => (Vec::new(), NullBitmap::all_valid(rows)),
        },
        ResolvedExpr::Column { index, .. } => {
            let source = column(*index);
            match &source.data {
                BatchData::Int64(values) => (values.clone(), source.validity.clone()),
                // A float column inside an *integer* expression cannot happen:
                // the type rules make the whole expression a float expression.
                // A string column cannot happen either -- the plan rejects it.
                // Both are refused loudly rather than silently producing an
                // empty column, which would look like a valid answer.
                BatchData::Float64(_) | BatchData::Utf8(_) => {
                    panic!("a column's type does not match the expression's type")
                }
            }
        }
        ResolvedExpr::Binary { op, left, right } => {
            let (left_values, left_validity) = eval_int(left, column, rows);
            let (right_values, right_validity) = eval_int(right, column, rows);
            let validity = left_validity.and(&right_validity);
            let values = left_values
                .iter()
                .zip(right_values.iter())
                .map(|(left, right)| apply_int(*op, *left, *right))
                .collect();
            (values, validity)
        }
        ResolvedExpr::Negate(inner) => {
            let (values, validity) = eval_int(inner, column, rows);
            (values.iter().map(|value| value.saturating_neg()).collect(), validity)
        }
    }
}

/// Evaluates a floating-point expression.
///
/// Returns the values and the validity mask.
fn eval_float<'a>(
    expr: &ResolvedExpr,
    column: &dyn Fn(usize) -> &'a BatchColumn,
    rows: usize,
) -> (Vec<f64>, NullBitmap) {
    match expr {
        ResolvedExpr::Constant(value) => match value {
            ScalarValue::Float64(value) => (vec![*value; rows], NullBitmap::all_valid(rows)),
            _ => (Vec::new(), NullBitmap::all_valid(rows)),
        },
        ResolvedExpr::Column { index, .. } => {
            let source = column(*index);
            match &source.data {
                BatchData::Float64(values) => (values.clone(), source.validity.clone()),
                // An integer column inside a *float* expression is widened.
                // The type rules say an expression is an integer expression
                // only when every operand is, so this is the ordinary case of
                // `1 - l_discount` where the constant is an integer.
                BatchData::Int64(values) => {
                    (values.iter().map(|value| widen(*value)).collect(), source.validity.clone())
                }
                // A string column cannot happen: the plan rejects it.
                BatchData::Utf8(_) => {
                    panic!("a string column cannot be an operand of an arithmetic expression")
                }
            }
        }
        ResolvedExpr::Binary { op, left, right } => {
            let (left_values, left_validity) = eval_float(left, column, rows);
            let (right_values, right_validity) = eval_float(right, column, rows);
            let validity = left_validity.and(&right_validity);
            let values = left_values
                .iter()
                .zip(right_values.iter())
                .map(|(left, right)| apply_float(*op, *left, *right))
                .collect();
            (values, validity)
        }
        ResolvedExpr::Negate(inner) => {
            let (values, validity) = eval_float(inner, column, rows);
            (values.iter().map(|value| -value).collect(), validity)
        }
    }
}

/// Widens an integer to a double.
///
/// The widening is lossy above 2^53, and it has to be: the expression's type
/// is `float64` because one of its operands is, and a `float64` operand
/// makes the whole thing a `float64` expression. This is the same widening
/// every SQL engine does, and the alternative -- refusing the expression --
/// would make `1 - l_discount` impossible for no gain.
#[allow(clippy::cast_precision_loss)]
fn widen(value: i64) -> f64 {
    value as f64
}

/// Applies an operator to two integers.
///
/// Saturating, because a wrapping sum would be a different answer on a
/// different machine. Division never reaches here: the plan rejects integer
/// division, and an expression that divides is a floating-point expression.
fn apply_int(op: ArithOp, left: i64, right: i64) -> i64 {
    match op {
        ArithOp::Add => left.saturating_add(right),
        ArithOp::Sub => left.saturating_sub(right),
        ArithOp::Mul => left.saturating_mul(right),
        ArithOp::Div => left.saturating_div(right),
    }
}

/// Applies an operator to two doubles.
///
/// IEEE-754 throughout: `NaN` propagates, `inf` propagates, and a division
/// by zero is an infinity rather than an error.
fn apply_float(op: ArithOp, left: f64, right: f64) -> f64 {
    match op {
        ArithOp::Add => left + right,
        ArithOp::Sub => left - right,
        ArithOp::Mul => left * right,
        ArithOp::Div => left / right,
    }
}
