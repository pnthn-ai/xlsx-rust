//! Excel `TOCOL(array, [ignore], [scan_by_col])`.
//!
//! Flattens `array` into a single column. Documented Excel behavior:
//!
//! - `ignore` 0 (default): keep every value, including blanks and errors
//! - `ignore` 1: drop blanks (`Empty` only — stored `""` is text and is kept)
//! - `ignore` 2: drop error values
//! - `ignore` 3: drop blanks and errors
//! - other `ignore` (after numeric coerce + trunc toward zero) → `#VALUE!`
//! - `scan_by_col` FALSE / omitted: row-major (left-to-right, then down)
//! - `scan_by_col` TRUE: column-major (top-to-bottom, then right)
//! - no survivors after ignore → `#CALC!` (Excel cannot return an empty array)
//! - more than [`TOCOL_MAX_ROWS`] kept values → `#NUM!`
//! - nested [`ExcelValue::Array`] cells are unnested with the same
//!   `ignore` / `scan_by_col` (Excel 365)
//!
//! Errors inside the source are **data** unless `ignore` is 2 or 3. A scalar
//! error `array` / `ignore` / `scan_by_col` argument still surfaces, left to
//! right.
//!
//! ## Spill / model limits
//!
//! - The engine returns an [`ExcelValue::Array`] (n×1, including 1×1). It
//!   does **not** write a spill range into the workbook snippet, so a blocked
//!   cell below the host never yields `#SPILL!`.
//! - Scalar operators (`TOCOL(...)+1`) take the top-left element
//!   (`scalarize`), not a host-aware intersection of a written spill. Use
//!   `INDEX` / `SUM` / `COUNTA` to consume the column without a grid write.
//! - Excel's worksheet column cap is enforced: a result longer than
//!   1,048,576 rows is `#NUM!`. That is the only size gate; there is no
//!   spill-grid occupancy check.

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

/// Excel worksheet row / dynamic-array height cap. `TOCOL` of a result that
/// would exceed this is `#NUM!`.
pub const TOCOL_MAX_ROWS: usize = 1_048_576;

const EMPTY: ExcelValue = ExcelValue::Empty;

/// Production kernel: walk in scan order, clone only kept cells, no transpose.
pub fn tocol_apply(array: &ExcelValue, ignore: u8, scan_by_col: bool) -> ExcelValue {
    tocol_kernel(array, ignore, scan_by_col, TOCOL_MAX_ROWS, Strategy::Fast)
}

/// Allocation-heavy baseline: clone-all, optional transpose, retain, wrap.
///
/// Same answers as [`tocol_apply`]. Used as the bench "before".
pub fn tocol_apply_naive(array: &ExcelValue, ignore: u8, scan_by_col: bool) -> ExcelValue {
    tocol_kernel(array, ignore, scan_by_col, TOCOL_MAX_ROWS, Strategy::Naive)
}

/// Test hook: same as [`tocol_apply`] with a smaller row cap.
pub fn tocol_apply_limited(
    array: &ExcelValue,
    ignore: u8,
    scan_by_col: bool,
    max_rows: usize,
) -> ExcelValue {
    tocol_kernel(array, ignore, scan_by_col, max_rows, Strategy::Fast)
}

#[derive(Clone, Copy)]
enum Strategy {
    Fast,
    Naive,
}

pub(crate) fn eval(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.is_empty() || args.len() > 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }

    let array = ev.eval_expr(&args[0], ctx)?;
    if let ExcelValue::Error(e) = array {
        return Ok(ExcelValue::Error(e));
    }

    let ignore = if args.len() >= 2 {
        match parse_ignore(&ev.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        0
    };

    let scan_by_col = if args.len() >= 3 {
        match coerce::to_logical(&ev.eval_scalar(&args[2], ctx)?) {
            Ok(b) => b,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        false
    };

    Ok(tocol_apply(&array, ignore, scan_by_col))
}

/// Coerce `ignore` like other integer Excel args: number/bool/empty/numeric
/// text, truncate toward zero, then require 0..=3.
pub fn parse_ignore(v: &ExcelValue) -> Result<u8, ExcelError> {
    let n = coerce::to_number(v)?;
    if !n.is_finite() {
        return Err(ExcelError::Value);
    }
    let t = n.trunc();
    if !(0.0..=3.0).contains(&t) {
        return Err(ExcelError::Value);
    }
    Ok(t as u8)
}

fn tocol_kernel(
    array: &ExcelValue,
    ignore: u8,
    scan_by_col: bool,
    max_rows: usize,
    strategy: Strategy,
) -> ExcelValue {
    match strategy {
        Strategy::Fast => match flatten_fast(array, ignore, scan_by_col, max_rows) {
            Ok(rows) => finish_column(rows),
            Err(e) => ExcelValue::Error(e),
        },
        Strategy::Naive => match flatten_naive(array, ignore, scan_by_col, max_rows) {
            Ok(rows) => finish_column(rows),
            Err(e) => ExcelValue::Error(e),
        },
    }
}

fn finish_column(rows: Vec<Vec<ExcelValue>>) -> ExcelValue {
    if rows.is_empty() {
        ExcelValue::Error(ExcelError::Calc)
    } else {
        ExcelValue::Array(rows)
    }
}

fn flatten_fast(
    array: &ExcelValue,
    ignore: u8,
    scan_by_col: bool,
    max_rows: usize,
) -> Result<Vec<Vec<ExcelValue>>, ExcelError> {
    let grid = Matrix::from_value(array)?;
    let (rows, cols) = grid.dims();
    let mut out = Vec::new();
    if ignore == 0 {
        out.reserve(rows.saturating_mul(cols).min(max_rows));
    }
    if scan_by_col {
        for c in 0..cols {
            for r in 0..rows {
                push_cell(grid.get(r, c), ignore, scan_by_col, max_rows, &mut out)?;
            }
        }
    } else {
        for r in 0..rows {
            for c in 0..cols {
                push_cell(grid.get(r, c), ignore, scan_by_col, max_rows, &mut out)?;
            }
        }
    }
    Ok(out)
}

/// Clone the whole matrix, transpose when scanning by column, flatten, then
/// `retain` ignored values. Same answers as [`flatten_fast`].
fn flatten_naive(
    array: &ExcelValue,
    ignore: u8,
    scan_by_col: bool,
    max_rows: usize,
) -> Result<Vec<Vec<ExcelValue>>, ExcelError> {
    let grid = Matrix::from_value(array)?;
    let (rows, cols) = grid.dims();
    let mut owned = Vec::with_capacity(rows);
    for r in 0..rows {
        let mut row = Vec::with_capacity(cols);
        for c in 0..cols {
            row.push(grid.get(r, c).clone());
        }
        owned.push(row);
    }
    if scan_by_col {
        owned = transpose(owned);
    }
    let mut flat = Vec::with_capacity(rows.saturating_mul(cols));
    for row in owned {
        for cell in row {
            expand_naive(cell, ignore, scan_by_col, max_rows, &mut flat)?;
        }
    }
    flat.retain(|v| keep_leaf(v, ignore));
    if flat.len() > max_rows {
        return Err(ExcelError::Num);
    }
    Ok(flat.into_iter().map(|v| vec![v]).collect())
}

fn expand_naive(
    cell: ExcelValue,
    ignore: u8,
    scan_by_col: bool,
    max_rows: usize,
    out: &mut Vec<ExcelValue>,
) -> Result<(), ExcelError> {
    if matches!(cell, ExcelValue::Array(_)) {
        match tocol_kernel(&cell, ignore, scan_by_col, max_rows, Strategy::Naive) {
            ExcelValue::Error(ExcelError::Calc) => Ok(()),
            ExcelValue::Error(e) => Err(e),
            ExcelValue::Array(rows) => {
                for row in rows {
                    if let Some(v) = row.into_iter().next() {
                        out.push(v);
                    }
                }
                Ok(())
            }
            other => {
                out.push(other);
                Ok(())
            }
        }
    } else {
        out.push(cell);
        Ok(())
    }
}

fn push_cell(
    cell: &ExcelValue,
    ignore: u8,
    scan_by_col: bool,
    max_rows: usize,
    out: &mut Vec<Vec<ExcelValue>>,
) -> Result<(), ExcelError> {
    match cell {
        ExcelValue::Array(_) => {
            match tocol_kernel(cell, ignore, scan_by_col, max_rows, Strategy::Fast) {
                ExcelValue::Error(ExcelError::Calc) => Ok(()),
                ExcelValue::Error(e) => Err(e),
                ExcelValue::Array(rows) => {
                    if out.len().saturating_add(rows.len()) > max_rows {
                        return Err(ExcelError::Num);
                    }
                    out.extend(rows);
                    Ok(())
                }
                other => {
                    if out.len() >= max_rows {
                        return Err(ExcelError::Num);
                    }
                    out.push(vec![other]);
                    Ok(())
                }
            }
        }
        other if keep_leaf(other, ignore) => {
            if out.len() >= max_rows {
                return Err(ExcelError::Num);
            }
            out.push(vec![other.clone()]);
            Ok(())
        }
        _ => Ok(()),
    }
}

fn keep_leaf(v: &ExcelValue, ignore: u8) -> bool {
    match v {
        ExcelValue::Empty => ignore & 1 == 0,
        ExcelValue::Error(_) => ignore & 2 == 0,
        ExcelValue::Array(_) => true,
        _ => true,
    }
}

fn transpose(rows: Vec<Vec<ExcelValue>>) -> Vec<Vec<ExcelValue>> {
    if rows.is_empty() {
        return rows;
    }
    let cols = rows[0].len();
    let mut out = vec![Vec::with_capacity(rows.len()); cols];
    for row in rows {
        for (c, v) in row.into_iter().enumerate() {
            if c < out.len() {
                out[c].push(v);
            }
        }
    }
    out
}

enum Matrix<'a> {
    Array(&'a [Vec<ExcelValue>]),
    Scalar(&'a ExcelValue),
}

impl<'a> Matrix<'a> {
    fn from_value(v: &'a ExcelValue) -> Result<Self, ExcelError> {
        match v {
            ExcelValue::Array(rows) => {
                if !rows.is_empty() {
                    let cols = rows[0].len();
                    if rows.iter().any(|r| r.len() != cols) {
                        return Err(ExcelError::Value);
                    }
                }
                Ok(Self::Array(rows))
            }
            other => Ok(Self::Scalar(other)),
        }
    }

    fn dims(&self) -> (usize, usize) {
        match self {
            Self::Array(rows) => {
                let r = rows.len();
                let c = rows.first().map(|row| row.len()).unwrap_or(0);
                (r, c)
            }
            Self::Scalar(_) => (1, 1),
        }
    }

    fn get(&self, r: usize, c: usize) -> &ExcelValue {
        match self {
            Self::Array(rows) => rows.get(r).and_then(|row| row.get(c)).unwrap_or(&EMPTY),
            Self::Scalar(v) if r == 0 && c == 0 => v,
            Self::Scalar(_) => &EMPTY,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(x: f64) -> ExcelValue {
        ExcelValue::Number(x)
    }

    fn t(s: &str) -> ExcelValue {
        ExcelValue::Text(s.into())
    }

    fn col(vals: &[ExcelValue]) -> ExcelValue {
        ExcelValue::Array(vals.iter().cloned().map(|v| vec![v]).collect())
    }

    fn matrix(rows: Vec<Vec<ExcelValue>>) -> ExcelValue {
        ExcelValue::Array(rows)
    }

    #[test]
    fn row_major_default() {
        let a = matrix(vec![vec![n(1.0), n(2.0)], vec![n(3.0), n(4.0)]]);
        assert_eq!(
            tocol_apply(&a, 0, false),
            col(&[n(1.0), n(2.0), n(3.0), n(4.0)])
        );
        assert_eq!(
            tocol_apply(&a, 0, true),
            col(&[n(1.0), n(3.0), n(2.0), n(4.0)])
        );
    }

    #[test]
    fn ignore_blanks_keeps_empty_string() {
        let a = matrix(vec![vec![n(1.0), ExcelValue::Empty], vec![t(""), n(2.0)]]);
        assert_eq!(tocol_apply(&a, 1, false), col(&[n(1.0), t(""), n(2.0)]));
    }

    #[test]
    fn ignore_errors_and_both() {
        let a = matrix(vec![
            vec![n(1.0), ExcelValue::Error(ExcelError::Na)],
            vec![ExcelValue::Empty, n(2.0)],
        ]);
        assert_eq!(
            tocol_apply(&a, 2, false),
            col(&[n(1.0), ExcelValue::Empty, n(2.0)])
        );
        assert_eq!(tocol_apply(&a, 3, false), col(&[n(1.0), n(2.0)]));
    }

    #[test]
    fn all_ignored_is_calc() {
        let a = matrix(vec![vec![ExcelValue::Empty, ExcelValue::Empty]]);
        assert_eq!(
            tocol_apply(&a, 1, false),
            ExcelValue::Error(ExcelError::Calc)
        );
        let errs = matrix(vec![vec![
            ExcelValue::Error(ExcelError::Div0),
            ExcelValue::Error(ExcelError::Na),
        ]]);
        assert_eq!(
            tocol_apply(&errs, 2, false),
            ExcelValue::Error(ExcelError::Calc)
        );
    }

    #[test]
    fn blanks_kept_are_empty_not_zero() {
        let a = matrix(vec![vec![n(1.0), ExcelValue::Empty]]);
        assert_eq!(tocol_apply(&a, 0, false), col(&[n(1.0), ExcelValue::Empty]));
    }

    #[test]
    fn scalar_and_one_by_one() {
        assert_eq!(tocol_apply(&n(5.0), 0, false), col(&[n(5.0)]));
        assert_eq!(
            tocol_apply(&matrix(vec![vec![n(7.0)]]), 0, false),
            col(&[n(7.0)])
        );
        assert_eq!(
            tocol_apply(&ExcelValue::Empty, 1, false),
            ExcelValue::Error(ExcelError::Calc)
        );
    }

    #[test]
    fn nested_array_unnests() {
        let inner = matrix(vec![vec![n(1.0), n(2.0)]]);
        let a = matrix(vec![vec![inner, n(3.0)]]);
        assert_eq!(tocol_apply(&a, 0, false), col(&[n(1.0), n(2.0), n(3.0)]));
    }

    #[test]
    fn row_cap_is_num() {
        let a = matrix(vec![vec![n(1.0), n(2.0), n(3.0)]]);
        assert_eq!(
            tocol_apply_limited(&a, 0, false, 2),
            ExcelValue::Error(ExcelError::Num)
        );
        assert_eq!(
            tocol_apply_limited(&a, 0, false, 3),
            col(&[n(1.0), n(2.0), n(3.0)])
        );
    }

    #[test]
    fn parse_ignore_truncates() {
        assert_eq!(parse_ignore(&n(1.9)).unwrap(), 1);
        assert_eq!(parse_ignore(&ExcelValue::Bool(true)).unwrap(), 1);
        assert_eq!(parse_ignore(&ExcelValue::Empty).unwrap(), 0);
        assert_eq!(parse_ignore(&t("2")).unwrap(), 2);
        assert_eq!(parse_ignore(&n(4.0)), Err(ExcelError::Value));
        assert_eq!(parse_ignore(&n(-1.0)), Err(ExcelError::Value));
        assert_eq!(parse_ignore(&t("x")), Err(ExcelError::Value));
    }

    #[test]
    fn jagged_is_value() {
        let a = matrix(vec![vec![n(1.0), n(2.0)], vec![n(3.0)]]);
        assert_eq!(
            tocol_apply(&a, 0, false),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn fast_matches_naive() {
        let a = matrix(vec![
            vec![n(1.0), ExcelValue::Empty, ExcelValue::Error(ExcelError::Na)],
            vec![t(""), n(2.0), ExcelValue::Bool(true)],
        ]);
        for ignore in 0..=3 {
            for scan in [false, true] {
                assert_eq!(
                    tocol_apply(&a, ignore, scan),
                    tocol_apply_naive(&a, ignore, scan),
                    "ignore={ignore} scan_by_col={scan}"
                );
            }
        }
    }
}
