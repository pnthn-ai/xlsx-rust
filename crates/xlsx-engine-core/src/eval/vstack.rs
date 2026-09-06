//! Excel `VSTACK(array1, [array2], ...)`.
//!
//! Stacks arrays **vertically** (row-wise). Result height is the sum of the
//! argument heights; result width is the **max** argument width.
//!
//! Documented Excel quirks this module implements:
//!
//! - Narrower arrays are padded on the right with `#N/A` (not blank, not
//!   `#VALUE!`). A blank cell in a source array stays [`ExcelValue::Empty`].
//! - A scalar is a 1×1 array (`VSTACK(1, 2)` is a 2×1 column).
//! - A **computed** scalar error (`#DIV/0!` literal, `1/0`, `FILTER` that
//!   returned `#CALC!`, `NA()`) surfaces as the whole result. First such
//!   error wins, left-to-right. A **cell / range / array-literal** error is
//!   data: it becomes a 1×1 error cell and is stacked (Microsoft’s F2
//!   `#VALUE!` example). Errors already inside an array stay in their cells.
//! - A 0-row array is ignored (contributes no rows). If every argument is
//!   0-row, the stacked result would be empty → `#CALC!` (Excel cannot
//!   return an empty array). A blank range is **not** empty: it contributes
//!   `Empty` cells.
//! - Jagged source arrays (rows of unequal length) are `#VALUE!`.
//! - No-arg `VSTACK()` is `#VALUE!` (handled by the caller).
//!
//! ## Spill / pad / width — model limits (honest)
//!
//! - The engine returns an [`ExcelValue::Array`]; it does **not** write a
//!   spill range into the workbook snippet. Occupied neighbors never yield
//!   `#SPILL!`.
//! - Pad `#N/A` lives **inside** that array. `IFNA` / `IFERROR` only replace
//!   a **scalar** error result (for example a propagated `#CALC!`). They do
//!   **not** walk the array and rewrite pad cells the way Excel's dynamic
//!   array `IFNA(VSTACK(...), "")` does. Use `INDEX` to pick a pad cell, or
//!   `SUM` (which surfaces `#N/A`) / `COUNTA` (which counts it).
//! - Excel's worksheet array-size cap (~1,048,576 rows) is not enforced;
//!   allocation is bounded only by memory.
//! - Omitted middle arguments (`VSTACK(A1,,B1)`) are not modeled — the
//!   parser requires an expression after each comma.
//!
//! [`stack`] measures once, preallocates the exact grid, and clones each
//! cell once. [`stack_owned`] is the evaluator path: it **moves** already-
//! owned rows when width matches. [`stack_naive`] rebuilds the whole result
//! on every argument (immutable append). Same answers; more allocation.
//! Used as the bench "before".

use super::{Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

const NA: ExcelValue = ExcelValue::Error(ExcelError::Na);
const EMPTY: ExcelValue = ExcelValue::Empty;

pub(crate) fn eval(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.is_empty() {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let mut values = Vec::with_capacity(args.len());
    for arg in args {
        values.push(eval_stack_arg(ev, arg, ctx)?);
    }
    Ok(stack_owned(values))
}

/// Cell / range / array-literal / name errors are 1×1 data. A computed
/// scalar error (`1/0`, `FILTER` → `#CALC!`, `#N/A` token) is left as
/// [`ExcelValue::Error`] so [`stack`] can surface it.
fn eval_stack_arg(ev: &Evaluator, expr: &Expr, ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    let v = ev.eval_expr(expr, ctx)?;
    match expr {
        Expr::Cell(_) | Expr::Range(_) | Expr::Array(_) | Expr::Name(_) => Ok(error_as_1x1(v)),
        _ => Ok(v),
    }
}

fn error_as_1x1(v: ExcelValue) -> ExcelValue {
    match v {
        ExcelValue::Error(e) => ExcelValue::Array(vec![vec![ExcelValue::Error(e)]]),
        other => other,
    }
}

/// Excel `VSTACK` from already-evaluated arguments.
pub fn stack(args: &[ExcelValue]) -> ExcelValue {
    vstack_apply(args, VstackStrategy::Fast)
}

/// Evaluator path: move owned `Array` rows when width already matches.
pub fn stack_owned(args: Vec<ExcelValue>) -> ExcelValue {
    if args.is_empty() {
        return ExcelValue::Error(ExcelError::Value);
    }
    let mut first_err = None;
    let mut total_rows = 0usize;
    let mut max_cols = 0usize;
    for arg in &args {
        match Matrix::from_value(arg) {
            Ok(m) => {
                let (r, c) = m.dims();
                total_rows += r;
                max_cols = max_cols.max(c);
            }
            Err(e) if first_err.is_none() => first_err = Some(e),
            Err(_) => {}
        }
    }
    if let Some(e) = first_err {
        return ExcelValue::Error(e);
    }
    if total_rows == 0 || max_cols == 0 {
        return ExcelValue::Error(ExcelError::Calc);
    }

    let mut out = Vec::with_capacity(total_rows);
    for arg in args {
        match arg {
            ExcelValue::Error(e) => return ExcelValue::Error(e),
            ExcelValue::Array(mut rows) => {
                if rows.is_empty() {
                    continue;
                }
                let cols = rows[0].len();
                if cols == 0 {
                    continue;
                }
                if cols == max_cols {
                    out.append(&mut rows);
                } else {
                    for mut row in rows {
                        row.reserve(max_cols - row.len());
                        while row.len() < max_cols {
                            row.push(NA);
                        }
                        out.push(row);
                    }
                }
            }
            other => {
                let mut row = Vec::with_capacity(max_cols);
                row.push(other);
                while row.len() < max_cols {
                    row.push(NA);
                }
                out.push(row);
            }
        }
    }
    ExcelValue::Array(out)
}

/// Immutable-append baseline: rebuild the whole result on every argument.
/// Same answers as [`stack`]. Bench "before".
pub fn stack_naive(args: &[ExcelValue]) -> ExcelValue {
    vstack_apply(args, VstackStrategy::Naive)
}

#[derive(Clone, Copy)]
enum VstackStrategy {
    Fast,
    Naive,
}

enum Matrix<'a> {
    Array(&'a [Vec<ExcelValue>]),
    Scalar(&'a ExcelValue),
    Empty,
}

impl<'a> Matrix<'a> {
    fn from_value(v: &'a ExcelValue) -> Result<Self, ExcelError> {
        match v {
            ExcelValue::Error(e) => Err(*e),
            ExcelValue::Array(rows) => {
                if rows.is_empty() {
                    return Ok(Self::Empty);
                }
                let cols = rows[0].len();
                if rows.iter().any(|r| r.len() != cols) {
                    return Err(ExcelError::Value);
                }
                if cols == 0 {
                    return Ok(Self::Empty);
                }
                Ok(Self::Array(rows))
            }
            other => Ok(Self::Scalar(other)),
        }
    }

    fn dims(&self) -> (usize, usize) {
        match self {
            Self::Array(rows) => (rows.len(), rows[0].len()),
            Self::Scalar(_) => (1, 1),
            Self::Empty => (0, 0),
        }
    }

    fn get(&self, r: usize, c: usize) -> &ExcelValue {
        match self {
            Self::Array(rows) => &rows[r][c],
            Self::Scalar(v) if r == 0 && c == 0 => v,
            Self::Scalar(_) | Self::Empty => &EMPTY,
        }
    }
}

fn vstack_apply(args: &[ExcelValue], strategy: VstackStrategy) -> ExcelValue {
    if args.is_empty() {
        return ExcelValue::Error(ExcelError::Value);
    }

    let mut mats = Vec::with_capacity(args.len());
    for arg in args {
        match Matrix::from_value(arg) {
            Ok(m) => mats.push(m),
            Err(e) => return ExcelValue::Error(e),
        }
    }

    match strategy {
        VstackStrategy::Fast => stack_fast(&mats),
        VstackStrategy::Naive => stack_naive_from(&mats),
    }
}

fn stack_fast(mats: &[Matrix<'_>]) -> ExcelValue {
    let mut total_rows = 0usize;
    let mut max_cols = 0usize;
    for m in mats {
        let (r, c) = m.dims();
        total_rows += r;
        max_cols = max_cols.max(c);
    }
    if total_rows == 0 || max_cols == 0 {
        return ExcelValue::Error(ExcelError::Calc);
    }

    let mut out = Vec::with_capacity(total_rows);
    for m in mats {
        let (rows, cols) = m.dims();
        for r in 0..rows {
            let mut row = Vec::with_capacity(max_cols);
            for c in 0..cols {
                row.push(m.get(r, c).clone());
            }
            for _ in cols..max_cols {
                row.push(NA);
            }
            out.push(row);
        }
    }
    ExcelValue::Array(out)
}

/// Rebuild the accumulated result on every argument (clone-all-so-far).
fn stack_naive_from(mats: &[Matrix<'_>]) -> ExcelValue {
    let mut out: Vec<Vec<ExcelValue>> = Vec::new();
    for m in mats {
        let (rows, cols) = m.dims();
        let width = out.first().map(|r| r.len()).unwrap_or(0).max(cols);
        let mut next = Vec::with_capacity(out.len() + rows);
        for row in &out {
            let mut r = row.clone();
            r.resize(width, NA);
            next.push(r);
        }
        for r in 0..rows {
            let mut row = Vec::with_capacity(width);
            for c in 0..cols {
                row.push(m.get(r, c).clone());
            }
            row.resize(width, NA);
            next.push(row);
        }
        out = next;
    }
    if out.is_empty() || out.first().map(|r| r.len()).unwrap_or(0) == 0 {
        return ExcelValue::Error(ExcelError::Calc);
    }
    ExcelValue::Array(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(x: f64) -> ExcelValue {
        ExcelValue::Number(x)
    }

    fn col(vals: &[f64]) -> ExcelValue {
        ExcelValue::Array(vals.iter().map(|x| vec![n(*x)]).collect())
    }

    fn row(vals: &[f64]) -> ExcelValue {
        ExcelValue::Array(vec![vals.iter().map(|x| n(*x)).collect()])
    }

    fn both_eq(args: &[ExcelValue]) {
        let owned = stack_owned(args.to_vec());
        assert_eq!(stack(args), stack_naive(args), "{args:?}");
        assert_eq!(stack(args), owned, "owned vs borrow {args:?}");
    }

    #[test]
    fn two_columns_equal_width() {
        let a = col(&[1.0, 2.0]);
        let b = col(&[3.0, 4.0]);
        assert_eq!(stack(&[a.clone(), b.clone()]), col(&[1.0, 2.0, 3.0, 4.0]));
        both_eq(&[a, b]);
    }

    #[test]
    fn pad_narrower_with_na() {
        let a = row(&[1.0, 2.0]);
        let b = ExcelValue::Number(3.0);
        let got = stack(&[a.clone(), b.clone()]);
        assert_eq!(
            got,
            ExcelValue::Array(vec![vec![n(1.0), n(2.0)], vec![n(3.0), NA]])
        );
        both_eq(&[a, b]);
    }

    #[test]
    fn wide_then_narrow_and_narrow_then_wide() {
        let wide = row(&[1.0, 2.0, 3.0]);
        let narrow = row(&[4.0]);
        let expected = ExcelValue::Array(vec![vec![n(1.0), n(2.0), n(3.0)], vec![n(4.0), NA, NA]]);
        assert_eq!(stack(&[wide.clone(), narrow.clone()]), expected);
        both_eq(&[wide.clone(), narrow.clone()]);

        let expected_rev =
            ExcelValue::Array(vec![vec![n(4.0), NA, NA], vec![n(1.0), n(2.0), n(3.0)]]);
        assert_eq!(stack(&[narrow.clone(), wide.clone()]), expected_rev);
        both_eq(&[narrow, wide]);
    }

    #[test]
    fn scalar_error_surfaces() {
        let err = ExcelValue::Error(ExcelError::Div0);
        assert_eq!(
            stack(&[err.clone(), n(1.0)]),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            stack(&[n(1.0), err.clone()]),
            ExcelValue::Error(ExcelError::Div0)
        );
        both_eq(&[err, n(1.0)]);
    }

    #[test]
    fn first_scalar_error_wins() {
        assert_eq!(
            stack(&[
                ExcelValue::Error(ExcelError::Na),
                ExcelValue::Error(ExcelError::Div0)
            ]),
            ExcelValue::Error(ExcelError::Na)
        );
    }

    #[test]
    fn error_inside_array_stays() {
        let a = ExcelValue::Array(vec![
            vec![n(1.0)],
            vec![ExcelValue::Error(ExcelError::Div0)],
        ]);
        let b = col(&[2.0]);
        let got = stack(&[a.clone(), b.clone()]);
        assert_eq!(
            got,
            ExcelValue::Array(vec![
                vec![n(1.0)],
                vec![ExcelValue::Error(ExcelError::Div0)],
                vec![n(2.0)]
            ])
        );
        both_eq(&[a, b]);
    }

    #[test]
    fn empty_array_ignored_then_calc() {
        let empty = ExcelValue::Array(vec![]);
        assert_eq!(
            stack(&[empty.clone(), n(5.0)]),
            ExcelValue::Array(vec![vec![n(5.0)]])
        );
        assert_eq!(stack(&[empty.clone()]), ExcelValue::Error(ExcelError::Calc));
        assert_eq!(
            stack(&[empty.clone(), ExcelValue::Array(vec![])]),
            ExcelValue::Error(ExcelError::Calc)
        );
        both_eq(&[empty, n(5.0)]);
    }

    #[test]
    fn blank_is_not_na_pad() {
        let a = ExcelValue::Array(vec![vec![n(1.0), ExcelValue::Empty]]);
        let b = row(&[2.0, 3.0]);
        let got = stack(&[a.clone(), b.clone()]);
        assert_eq!(
            got,
            ExcelValue::Array(vec![vec![n(1.0), ExcelValue::Empty], vec![n(2.0), n(3.0)]])
        );
        both_eq(&[a, b]);
    }

    #[test]
    fn jagged_is_value() {
        let jagged = ExcelValue::Array(vec![vec![n(1.0)], vec![n(2.0), n(3.0)]]);
        assert_eq!(stack(&[jagged]), ExcelValue::Error(ExcelError::Value));
    }

    #[test]
    fn single_scalar_is_1x1() {
        assert_eq!(stack(&[n(7.0)]), ExcelValue::Array(vec![vec![n(7.0)]]));
        both_eq(&[n(7.0)]);
    }

    #[test]
    fn no_args_is_value() {
        assert_eq!(stack(&[]), ExcelValue::Error(ExcelError::Value));
        assert_eq!(stack_naive(&[]), ExcelValue::Error(ExcelError::Value));
    }
}
