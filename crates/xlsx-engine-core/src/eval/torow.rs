//! Excel `TOROW(array, [ignore], [scan_by_col])`.
//!
//! Flattens `array` into a single row. Scan order is row-major (left-to-right,
//! then down) unless `scan_by_col` is TRUE, which walks top-to-bottom then
//! across. `ignore` is a whole number:
//!
//! - `0` (omitted): keep every value, including blanks and errors
//! - `1`: drop blanks (`ExcelValue::Empty` only — `""` is text, not blank)
//! - `2`: drop error values
//! - `3`: drop blanks and errors
//!
//! Anything else (non-finite, fractional, or outside `0..=3`) is `#VALUE!`
//! except a non-finite ignore, which is `#NUM!`.
//!
//! A result with no surviving cells is `#CALC!` (Excel cannot return an empty
//! array). A scalar error argument propagates as that error; an error *inside*
//! an array is a kept cell unless `ignore` is 2 or 3.
//!
//! ## Spill / model limits
//!
//! - The engine returns an [`ExcelValue::Array`] (always one row, including
//!   1×1). It does **not** write a spill range into the snippet workbook, so a
//!   blocked cell to the right of the host never yields `#SPILL!`.
//! - Excel's worksheet column cap (16,384 / `XFD`) is **not** enforced. A
//!   result wider than that is memory-bounded here; Excel itself would
//!   `#NUM!`. The ~1,048,576-row cap does not apply (the result is one row).
//! - Scalar operators (`TOROW(...)+1`) still [`scalarize`](super::coerce::scalarize)
//!   to the top-left element. Consume the row with `INDEX` / `SUM` / `COUNTA`
//!   / `TYPE` instead of relying on a written spill.
//!
//! [`apply`] walks the grid once and clones only kept cells, with a reserved
//! buffer. [`apply_naive`] materializes every cell first, then `retain`s —
//! same answers, more allocation. Used as the bench "before".

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

/// Excel `ignore` codes for `TOROW` / `TOCOL`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TorowIgnore {
    KeepAll = 0,
    Blanks = 1,
    Errors = 2,
    BlanksAndErrors = 3,
}

impl TorowIgnore {
    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::KeepAll),
            1 => Some(Self::Blanks),
            2 => Some(Self::Errors),
            3 => Some(Self::BlanksAndErrors),
            _ => None,
        }
    }

    fn keep(self, v: &ExcelValue) -> bool {
        match self {
            Self::KeepAll => true,
            Self::Blanks => !matches!(v, ExcelValue::Empty),
            Self::Errors => !matches!(v, ExcelValue::Error(_)),
            Self::BlanksAndErrors => !matches!(v, ExcelValue::Empty | ExcelValue::Error(_)),
        }
    }
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
            Ok(i) => i,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        TorowIgnore::KeepAll
    };
    let scan_by_col = if args.len() >= 3 {
        match logical_flag(ev, &args[2], ctx)? {
            Ok(b) => b,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        false
    };

    Ok(apply_owned(array, ignore, scan_by_col))
}

fn logical_flag(
    ev: &Evaluator,
    expr: &Expr,
    ctx: &mut Ctx<'_>,
) -> Result<Result<bool, ExcelError>, EvalError> {
    let v = ev.eval_scalar(expr, ctx)?;
    Ok(coerce::to_logical(&v))
}

/// Parse the `ignore` argument. Whole numbers `0..=3` only.
pub fn parse_ignore(v: &ExcelValue) -> Result<TorowIgnore, ExcelError> {
    let n = coerce::to_number(v)?;
    if !n.is_finite() {
        return Err(ExcelError::Num);
    }
    // Excel documents a non-whole ignore as `#VALUE!` (not INT-truncation).
    if n.fract() != 0.0 {
        return Err(ExcelError::Value);
    }
    if n < 0.0 || n > 3.0 {
        return Err(ExcelError::Value);
    }
    TorowIgnore::from_code(n as u8).ok_or(ExcelError::Value)
}

/// Production kernel from an already-evaluated array value.
pub fn apply(array: &ExcelValue, ignore: TorowIgnore, scan_by_col: bool) -> ExcelValue {
    match array {
        ExcelValue::Error(e) => ExcelValue::Error(*e),
        ExcelValue::Array(rows) => match rectangular(rows) {
            Ok(()) => flatten_fast(rows, ignore, scan_by_col),
            Err(e) => ExcelValue::Error(e),
        },
        other => {
            let g = [vec![other.clone()]];
            flatten_fast(&g, ignore, scan_by_col)
        }
    }
}

/// Same answers as [`apply`], taking ownership so a keep-all row-major flatten
/// can move cells instead of cloning them.
pub fn apply_owned(array: ExcelValue, ignore: TorowIgnore, scan_by_col: bool) -> ExcelValue {
    match array {
        ExcelValue::Error(e) => ExcelValue::Error(e),
        ExcelValue::Array(rows) => match rectangular(&rows) {
            Ok(()) => flatten_owned(rows, ignore, scan_by_col),
            Err(e) => ExcelValue::Error(e),
        },
        other => flatten_owned(vec![vec![other]], ignore, scan_by_col),
    }
}

/// Allocation-heavy baseline: clone every cell, then `retain`.
///
/// Same answers as [`apply`]. Used as the bench "before".
pub fn apply_naive(array: &ExcelValue, ignore: TorowIgnore, scan_by_col: bool) -> ExcelValue {
    match array {
        ExcelValue::Error(e) => ExcelValue::Error(*e),
        ExcelValue::Array(rows) => match rectangular(rows) {
            Ok(()) => flatten_naive(rows, ignore, scan_by_col),
            Err(e) => ExcelValue::Error(e),
        },
        other => {
            let g = [vec![other.clone()]];
            flatten_naive(&g, ignore, scan_by_col)
        }
    }
}

fn rectangular(rows: &[Vec<ExcelValue>]) -> Result<(), ExcelError> {
    if rows.is_empty() {
        return Ok(());
    }
    let cols = rows[0].len();
    if rows.iter().any(|r| r.len() != cols) {
        return Err(ExcelError::Value);
    }
    Ok(())
}

fn finish_row(kept: Vec<ExcelValue>) -> ExcelValue {
    if kept.is_empty() {
        ExcelValue::Error(ExcelError::Calc)
    } else {
        ExcelValue::Array(vec![kept])
    }
}

fn flatten_fast(grid: &[Vec<ExcelValue>], ignore: TorowIgnore, scan_by_col: bool) -> ExcelValue {
    if grid.is_empty() || grid[0].is_empty() {
        return ExcelValue::Error(ExcelError::Calc);
    }
    let rows = grid.len();
    let cols = grid[0].len();
    let mut out = Vec::with_capacity(rows.saturating_mul(cols));
    if scan_by_col {
        for c in 0..cols {
            for row in grid {
                push_kept(&mut out, &row[c], ignore);
            }
        }
    } else {
        for row in grid {
            for cell in row {
                push_kept(&mut out, cell, ignore);
            }
        }
    }
    finish_row(out)
}

fn flatten_owned(rows: Vec<Vec<ExcelValue>>, ignore: TorowIgnore, scan_by_col: bool) -> ExcelValue {
    if rows.is_empty() || rows[0].is_empty() {
        return ExcelValue::Error(ExcelError::Calc);
    }
    if ignore == TorowIgnore::KeepAll && !scan_by_col {
        let cap = rows.iter().map(|r| r.len()).sum();
        let mut out = Vec::with_capacity(cap);
        for row in rows {
            out.extend(row);
        }
        return finish_row(out);
    }
    flatten_fast(&rows, ignore, scan_by_col)
}

fn flatten_naive(grid: &[Vec<ExcelValue>], ignore: TorowIgnore, scan_by_col: bool) -> ExcelValue {
    if grid.is_empty() || grid[0].is_empty() {
        return ExcelValue::Error(ExcelError::Calc);
    }
    let mut all = if scan_by_col {
        let transposed = transpose(grid);
        flatten_all(&transposed)
    } else {
        flatten_all(grid)
    };
    all.retain(|v| ignore.keep(v));
    finish_row(all)
}

fn flatten_all(grid: &[Vec<ExcelValue>]) -> Vec<ExcelValue> {
    let mut out = Vec::new();
    for row in grid {
        for cell in row {
            out.push(cell.clone());
        }
    }
    out
}

fn transpose(rows: &[Vec<ExcelValue>]) -> Vec<Vec<ExcelValue>> {
    let ncols = rows.first().map(|r| r.len()).unwrap_or(0);
    let mut out = vec![Vec::with_capacity(rows.len()); ncols];
    for row in rows {
        for (c, cell) in row.iter().enumerate() {
            if c < ncols {
                out[c].push(cell.clone());
            }
        }
    }
    out
}

fn push_kept(out: &mut Vec<ExcelValue>, cell: &ExcelValue, ignore: TorowIgnore) {
    if ignore.keep(cell) {
        out.push(cell.clone());
    }
}

/// Scalar non-array values are a 1×1 grid (Excel wraps them).
fn as_matrix(v: &ExcelValue) -> Result<Vec<Vec<ExcelValue>>, ExcelError> {
    match v {
        ExcelValue::Error(e) => Err(*e),
        ExcelValue::Array(rows) => {
            rectangular(rows)?;
            Ok(rows.clone())
        }
        other => Ok(vec![vec![other.clone()]]),
    }
}

/// High-level entry used by seed-compliant and benches that pass values.
///
/// `ignore` / `scan_by_col` omitted (`None`) are Excel defaults (`0` / FALSE).
pub fn excel_torow(
    array: &ExcelValue,
    ignore: Option<&ExcelValue>,
    scan_by_col: Option<&ExcelValue>,
) -> ExcelValue {
    if let ExcelValue::Error(e) = array {
        return ExcelValue::Error(*e);
    }
    let ign = match ignore {
        Some(v) => match parse_ignore(v) {
            Ok(i) => i,
            Err(e) => return ExcelValue::Error(e),
        },
        None => TorowIgnore::KeepAll,
    };
    let by_col = match scan_by_col {
        Some(v) => match coerce::to_logical(v) {
            Ok(b) => b,
            Err(e) => return ExcelValue::Error(e),
        },
        None => false,
    };
    match as_matrix(array) {
        Ok(grid) => flatten_fast(&grid, ign, by_col),
        Err(e) => ExcelValue::Error(e),
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
    fn row(vals: &[ExcelValue]) -> ExcelValue {
        ExcelValue::Array(vec![vals.to_vec()])
    }
    fn grid(rows: Vec<Vec<ExcelValue>>) -> ExcelValue {
        ExcelValue::Array(rows)
    }

    fn both_eq(array: &ExcelValue, ignore: TorowIgnore, scan_by_col: bool) {
        assert_eq!(
            apply(array, ignore, scan_by_col),
            apply_naive(array, ignore, scan_by_col),
            "fast != naive ignore={ignore:?} scan_by_col={scan_by_col}"
        );
    }

    #[test]
    fn matrix_row_major() {
        let a = grid(vec![
            vec![n(1.0), n(2.0), n(3.0)],
            vec![n(4.0), n(5.0), n(6.0)],
        ]);
        assert_eq!(
            apply(&a, TorowIgnore::KeepAll, false),
            row(&[n(1.0), n(2.0), n(3.0), n(4.0), n(5.0), n(6.0)])
        );
        both_eq(&a, TorowIgnore::KeepAll, false);
    }

    #[test]
    fn matrix_col_major() {
        let a = grid(vec![
            vec![n(1.0), n(2.0), n(3.0)],
            vec![n(4.0), n(5.0), n(6.0)],
        ]);
        assert_eq!(
            apply(&a, TorowIgnore::KeepAll, true),
            row(&[n(1.0), n(4.0), n(2.0), n(5.0), n(3.0), n(6.0)])
        );
        both_eq(&a, TorowIgnore::KeepAll, true);
    }

    #[test]
    fn column_becomes_row() {
        let a = grid(vec![vec![n(1.0)], vec![n(2.0)], vec![n(3.0)]]);
        assert_eq!(
            apply(&a, TorowIgnore::KeepAll, false),
            row(&[n(1.0), n(2.0), n(3.0)])
        );
        both_eq(&a, TorowIgnore::KeepAll, false);
    }

    #[test]
    fn already_a_row() {
        let a = row(&[n(1.0), n(2.0), n(3.0)]);
        assert_eq!(apply(&a, TorowIgnore::KeepAll, false), a);
        both_eq(&a, TorowIgnore::KeepAll, false);
    }

    #[test]
    fn scalar_wraps_as_1x1() {
        assert_eq!(apply(&n(5.0), TorowIgnore::KeepAll, false), row(&[n(5.0)]));
        assert_eq!(apply(&t("hi"), TorowIgnore::Blanks, true), row(&[t("hi")]));
        both_eq(&n(5.0), TorowIgnore::KeepAll, false);
        both_eq(&ExcelValue::Empty, TorowIgnore::KeepAll, false);
        assert_eq!(
            apply(&ExcelValue::Empty, TorowIgnore::Blanks, false),
            ExcelValue::Error(ExcelError::Calc)
        );
    }

    #[test]
    fn ignore_blanks_keeps_empty_string() {
        let a = grid(vec![vec![n(1.0), ExcelValue::Empty, t("")]]);
        assert_eq!(apply(&a, TorowIgnore::Blanks, false), row(&[n(1.0), t("")]));
        both_eq(&a, TorowIgnore::Blanks, false);
    }

    #[test]
    fn ignore_errors_keeps_blanks() {
        let a = grid(vec![vec![
            n(1.0),
            ExcelValue::Error(ExcelError::Div0),
            ExcelValue::Empty,
        ]]);
        assert_eq!(
            apply(&a, TorowIgnore::Errors, false),
            row(&[n(1.0), ExcelValue::Empty])
        );
        both_eq(&a, TorowIgnore::Errors, false);
    }

    #[test]
    fn ignore_both_all_gone_is_calc() {
        let a = grid(vec![vec![
            ExcelValue::Empty,
            ExcelValue::Error(ExcelError::Na),
        ]]);
        assert_eq!(
            apply(&a, TorowIgnore::BlanksAndErrors, false),
            ExcelValue::Error(ExcelError::Calc)
        );
        both_eq(&a, TorowIgnore::BlanksAndErrors, false);
    }

    #[test]
    fn keep_errors_in_result() {
        let a = grid(vec![vec![
            n(1.0),
            ExcelValue::Error(ExcelError::Div0),
            n(3.0),
        ]]);
        assert_eq!(
            apply(&a, TorowIgnore::KeepAll, false),
            row(&[n(1.0), ExcelValue::Error(ExcelError::Div0), n(3.0)])
        );
        both_eq(&a, TorowIgnore::KeepAll, false);
    }

    #[test]
    fn zero_and_false_are_not_blank() {
        let a = grid(vec![vec![
            n(0.0),
            ExcelValue::Bool(false),
            ExcelValue::Empty,
        ]]);
        assert_eq!(
            apply(&a, TorowIgnore::Blanks, false),
            row(&[n(0.0), ExcelValue::Bool(false)])
        );
        both_eq(&a, TorowIgnore::Blanks, false);
    }

    #[test]
    fn empty_grid_is_calc() {
        let a = ExcelValue::Array(vec![]);
        assert_eq!(
            apply(&a, TorowIgnore::KeepAll, false),
            ExcelValue::Error(ExcelError::Calc)
        );
        both_eq(&a, TorowIgnore::KeepAll, false);
    }

    #[test]
    fn jagged_is_value() {
        let a = grid(vec![vec![n(1.0), n(2.0)], vec![n(3.0)]]);
        assert_eq!(
            apply(&a, TorowIgnore::KeepAll, false),
            ExcelValue::Error(ExcelError::Value)
        );
        both_eq(&a, TorowIgnore::KeepAll, false);
    }

    #[test]
    fn parse_ignore_codes() {
        assert_eq!(parse_ignore(&n(0.0)).unwrap(), TorowIgnore::KeepAll);
        assert_eq!(parse_ignore(&n(1.0)).unwrap(), TorowIgnore::Blanks);
        assert_eq!(parse_ignore(&n(2.0)).unwrap(), TorowIgnore::Errors);
        assert_eq!(parse_ignore(&n(3.0)).unwrap(), TorowIgnore::BlanksAndErrors);
        assert_eq!(
            parse_ignore(&ExcelValue::Bool(true)).unwrap(),
            TorowIgnore::Blanks
        );
        assert_eq!(
            parse_ignore(&ExcelValue::Bool(false)).unwrap(),
            TorowIgnore::KeepAll
        );
        assert_eq!(
            parse_ignore(&ExcelValue::Empty).unwrap(),
            TorowIgnore::KeepAll
        );
        assert_eq!(parse_ignore(&n(1.5)), Err(ExcelError::Value));
        assert_eq!(parse_ignore(&n(4.0)), Err(ExcelError::Value));
        assert_eq!(parse_ignore(&n(-1.0)), Err(ExcelError::Value));
        assert_eq!(parse_ignore(&t("x")), Err(ExcelError::Value));
        assert_eq!(
            parse_ignore(&ExcelValue::Number(f64::INFINITY)),
            Err(ExcelError::Num)
        );
    }

    #[test]
    fn excel_torow_defaults() {
        let a = grid(vec![vec![n(1.0), n(2.0)], vec![n(3.0), n(4.0)]]);
        assert_eq!(
            excel_torow(&a, None, None),
            row(&[n(1.0), n(2.0), n(3.0), n(4.0)])
        );
        assert_eq!(
            excel_torow(&a, Some(&n(0.0)), Some(&ExcelValue::Bool(true))),
            row(&[n(1.0), n(3.0), n(2.0), n(4.0)])
        );
    }

    #[test]
    fn scalar_error_propagates() {
        let e = ExcelValue::Error(ExcelError::Div0);
        assert_eq!(excel_torow(&e, None, None), e);
    }

    #[test]
    fn apply_owned_moves_row_major() {
        let a = grid(vec![vec![n(1.0), n(2.0)], vec![n(3.0), n(4.0)]]);
        assert_eq!(
            apply_owned(a.clone(), TorowIgnore::KeepAll, false),
            apply(&a, TorowIgnore::KeepAll, false)
        );
        assert_eq!(
            apply_owned(a.clone(), TorowIgnore::KeepAll, true),
            apply(&a, TorowIgnore::KeepAll, true)
        );
    }

    #[test]
    fn large_grid_matches_naive() {
        let n_rows = 64usize;
        let n_cols = 32usize;
        let mut rows = Vec::with_capacity(n_rows);
        for r in 0..n_rows {
            let mut row = Vec::with_capacity(n_cols);
            for c in 0..n_cols {
                let i = r * n_cols + c;
                row.push(match i % 5 {
                    0 => ExcelValue::Empty,
                    1 => ExcelValue::Error(ExcelError::Na),
                    2 => t("x"),
                    3 => ExcelValue::Bool(i % 2 == 0),
                    _ => n(i as f64),
                });
            }
            rows.push(row);
        }
        let array = ExcelValue::Array(rows);
        for ign in [
            TorowIgnore::KeepAll,
            TorowIgnore::Blanks,
            TorowIgnore::Errors,
            TorowIgnore::BlanksAndErrors,
        ] {
            for by_col in [false, true] {
                both_eq(&array, ign, by_col);
            }
        }
    }

    #[test]
    fn ignore_blanks_scan_by_col() {
        let a = grid(vec![
            vec![n(1.0), ExcelValue::Empty, n(3.0)],
            vec![ExcelValue::Empty, n(5.0), n(6.0)],
        ]);
        assert_eq!(
            apply(&a, TorowIgnore::Blanks, true),
            row(&[n(1.0), n(5.0), n(3.0), n(6.0)])
        );
        both_eq(&a, TorowIgnore::Blanks, true);
    }
}
