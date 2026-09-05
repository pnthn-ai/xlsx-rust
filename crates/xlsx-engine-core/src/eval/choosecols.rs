//! Excel `CHOOSECOLS(array, col_num1, [col_num2], ...)`.
//!
//! Returns the listed columns, in the listed order, as an
//! [`ExcelValue::Array`] (including 1×1). This engine does **not** write a
//! spill range, so occupied neighbors never produce `#SPILL!`.
//!
//! ## Documented Excel quirks
//!
//! - **Positive** `col_num` is 1-based from the left.
//! - **Negative** `col_num` counts from the right: `-1` is the last column,
//!   `-2` the second-last. Microsoft's wording: a `#VALUE!` if the absolute
//!   value is zero or exceeds the column count. So `-4` on a 3-column array
//!   is `#VALUE!`, not a wrap.
//! - **Zero** (`0`, `-0`, blank coerced to 0, `FALSE`) is `#VALUE!`.
//!   Out-of-range positive (`4` on 3 columns) is also `#VALUE!` — **not**
//!   INDEX's `#REF!`.
//! - Fractions **truncate toward zero** (CHOOSE / INDEX / LEFT family).
//!   `1.9` → column 1; `-1.9` → `-1` (last column); `0.9` / `-0.4` → `0`
//!   → `#VALUE!`. `3.9` on a 3-column array is column 3. CI has no live
//!   Excel recording; this is the CHOOSE-family rule, not a guessed golden.
//! - Each `col_num` argument may be a scalar or an array. Arrays flatten
//!   **row-major** (`{1,3}` and `{1;3}` both mean columns 1 then 3).
//! - Duplicates and reordering are allowed (`1,3,5,1`; `-1,-2`).
//! - Errors **in the source array** are copied into the result. A scalar
//!   error as `array` wins. An error in a `col_num` (scalar or array cell)
//!   wins left-to-right / row-major and suppresses the pick.
//! - Coercion of `col_num`: empty → 0 → `#VALUE!`; `TRUE` → 1; numeric
//!   text (`"2"`) → 2; other text → `#VALUE!`.
//!
//! ## Range walk (perf)
//!
//! When `array` is a worksheet range, width is known from the `RangeRef`,
//! so only the **selected** columns are evaluated. Errors sitting only in
//! dropped columns do not appear in the result (same as pick-after-materialize).
//! Those cells are not walked, so a circular formula that exists *only* in
//! a dropped column is not observed — documented model limit, not hidden.
//!
//! [`select`] clones only requested cells. [`select_naive`] transposes the
//! whole grid, retains columns as rows, transposes back — same answers,
//! more allocation. Used as the bench "before".

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{CellRef, EvalError, ExcelError, ExcelValue, RangeRef};

const EMPTY: ExcelValue = ExcelValue::Empty;

/// Excel `CHOOSECOLS` from already-evaluated `array` + `col_num` values.
pub fn select(array: &ExcelValue, col_nums: &[ExcelValue]) -> ExcelValue {
    choosecols_apply(array, col_nums, Strategy::Fast)
}

/// Allocation-heavy baseline: clone + transpose + retain + transpose.
///
/// Same answers as [`select`]. Used as the bench "before".
pub fn select_naive(array: &ExcelValue, col_nums: &[ExcelValue]) -> ExcelValue {
    choosecols_apply(array, col_nums, Strategy::Naive)
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
    if args.len() < 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }

    if let Expr::Range(range) = &args[0] {
        return eval_range(ev, range, &args[1..], ctx);
    }

    let array = ev.eval_expr(&args[0], ctx)?;
    let mut col_nums = Vec::with_capacity(args.len() - 1);
    for arg in &args[1..] {
        col_nums.push(ev.eval_expr(arg, ctx)?);
    }
    Ok(select(&array, &col_nums))
}

fn eval_range(
    ev: &Evaluator,
    range: &RangeRef,
    col_num_args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    let sheet_name = range
        .sheet
        .clone()
        .unwrap_or_else(|| ctx.current_sheet.clone());
    let sheet_missing = ctx.spec.workbook.sheet(Some(&sheet_name)).is_err();

    let mut col_nums = Vec::with_capacity(col_num_args.len());
    for arg in col_num_args {
        col_nums.push(ev.eval_expr(arg, ctx)?);
    }
    if sheet_missing {
        return Ok(ExcelValue::Error(ExcelError::Ref));
    }

    let ncols = range.col_count() as usize;
    let nrows = range.row_count() as usize;
    if ncols == 0 || nrows == 0 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let idx = match collect_indices(&col_nums, ncols) {
        Ok(i) => i,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };

    let mut out = Vec::with_capacity(nrows);
    for r in 0..nrows {
        let mut row = Vec::with_capacity(idx.len());
        for &c in &idx {
            let addr =
                xlsx_types::CellAddr::new(range.start.col + c as u32, range.start.row + r as u32);
            row.push(ev.eval_cell(
                &CellRef {
                    sheet: Some(sheet_name.clone()),
                    addr,
                },
                ctx,
            )?);
        }
        out.push(row);
    }
    Ok(ExcelValue::Array(out))
}

fn choosecols_apply(array: &ExcelValue, col_nums: &[ExcelValue], strategy: Strategy) -> ExcelValue {
    if let ExcelValue::Error(e) = array {
        return ExcelValue::Error(*e);
    }
    let grid = match as_grid(array) {
        Ok(g) => g,
        Err(e) => return ExcelValue::Error(e),
    };
    let (nrows, ncols) = grid.dims();
    if nrows == 0 || ncols == 0 {
        return ExcelValue::Error(ExcelError::Value);
    }
    let idx = match collect_indices(col_nums, ncols) {
        Ok(i) => i,
        Err(e) => return ExcelValue::Error(e),
    };
    match strategy {
        Strategy::Fast => take_fast(&grid, &idx),
        Strategy::Naive => take_naive(&grid, &idx),
    }
}

enum Grid<'a> {
    Array(&'a [Vec<ExcelValue>]),
    Scalar(&'a ExcelValue),
}

impl<'a> Grid<'a> {
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

fn as_grid(v: &ExcelValue) -> Result<Grid<'_>, ExcelError> {
    match v {
        ExcelValue::Array(rows) => {
            if rows.is_empty() {
                return Ok(Grid::Array(rows));
            }
            let cols = rows[0].len();
            if rows.iter().any(|r| r.len() != cols) {
                return Err(ExcelError::Value);
            }
            Ok(Grid::Array(rows))
        }
        other => Ok(Grid::Scalar(other)),
    }
}

fn collect_indices(col_nums: &[ExcelValue], ncols: usize) -> Result<Vec<usize>, ExcelError> {
    let mut out = Vec::new();
    for v in col_nums {
        push_indices(v, ncols, &mut out)?;
    }
    if out.is_empty() {
        return Err(ExcelError::Value);
    }
    Ok(out)
}

fn push_indices(v: &ExcelValue, ncols: usize, out: &mut Vec<usize>) -> Result<(), ExcelError> {
    match v {
        ExcelValue::Error(e) => Err(*e),
        ExcelValue::Array(rows) => {
            for row in rows {
                for cell in row {
                    push_indices(cell, ncols, out)?;
                }
            }
            Ok(())
        }
        other => {
            let n = coerce::to_number(other)?;
            out.push(resolve_index(n, ncols)?);
            Ok(())
        }
    }
}

/// Truncate toward zero, then apply Excel's abs-zero / abs-exceeds `#VALUE!`.
fn resolve_index(n: f64, ncols: usize) -> Result<usize, ExcelError> {
    if !n.is_finite() {
        return Err(ExcelError::Value);
    }
    let t = n.trunc();
    if t == 0.0 {
        return Err(ExcelError::Value);
    }
    let n_cols = ncols as f64;
    if t > 0.0 {
        if t > n_cols {
            return Err(ExcelError::Value);
        }
        Ok(t as usize - 1)
    } else {
        let abs = -t;
        if abs > n_cols {
            return Err(ExcelError::Value);
        }
        Ok(ncols - abs as usize)
    }
}

fn take_fast(grid: &Grid<'_>, idx: &[usize]) -> ExcelValue {
    let (nrows, _) = grid.dims();
    let mut out = Vec::with_capacity(nrows);
    for r in 0..nrows {
        let mut row = Vec::with_capacity(idx.len());
        for &c in idx {
            row.push(grid.get(r, c).clone());
        }
        out.push(row);
    }
    ExcelValue::Array(out)
}

/// Naive: materialize, transpose, pick requested columns as rows, transpose.
fn take_naive(grid: &Grid<'_>, idx: &[usize]) -> ExcelValue {
    let full = materialize(grid);
    let cols_as_rows = transpose(&full);
    let picked: Vec<Vec<ExcelValue>> = idx
        .iter()
        .map(|&c| cols_as_rows.get(c).cloned().unwrap_or_default())
        .collect();
    ExcelValue::Array(transpose(&picked))
}

fn materialize(grid: &Grid<'_>) -> Vec<Vec<ExcelValue>> {
    let (r, c) = grid.dims();
    let mut out = Vec::with_capacity(r);
    for i in 0..r {
        let mut row = Vec::with_capacity(c);
        for j in 0..c {
            row.push(grid.get(i, j).clone());
        }
        out.push(row);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn n(x: f64) -> ExcelValue {
        ExcelValue::Number(x)
    }
    fn row(vals: &[f64]) -> ExcelValue {
        ExcelValue::Array(vec![vals.iter().copied().map(n).collect()])
    }
    fn matrix(rows: &[&[f64]]) -> ExcelValue {
        ExcelValue::Array(
            rows.iter()
                .map(|r| r.iter().copied().map(n).collect())
                .collect(),
        )
    }

    fn both_eq(array: &ExcelValue, col_nums: &[ExcelValue]) {
        assert_eq!(select(array, col_nums), select_naive(array, col_nums));
    }

    #[test]
    fn pick_middle_column() {
        let a = matrix(&[&[1.0, 2.0, 3.0], &[4.0, 5.0, 6.0]]);
        let got = select(&a, &[n(2.0)]);
        assert_eq!(got, ExcelValue::Array(vec![vec![n(2.0)], vec![n(5.0)]]));
        both_eq(&a, &[n(2.0)]);
    }

    #[test]
    fn negative_is_from_the_right() {
        let a = row(&[1.0, 2.0, 3.0]);
        assert_eq!(
            select(&a, &[n(-1.0)]),
            ExcelValue::Array(vec![vec![n(3.0)]])
        );
        assert_eq!(
            select(&a, &[n(-2.0)]),
            ExcelValue::Array(vec![vec![n(2.0)]])
        );
        assert_eq!(
            select(&a, &[n(-1.0), n(-2.0)]),
            ExcelValue::Array(vec![vec![n(3.0), n(2.0)]])
        );
        both_eq(&a, &[n(-1.0), n(-2.0)]);
    }

    #[test]
    fn zero_and_oob_are_value_not_ref() {
        let a = row(&[1.0, 2.0, 3.0]);
        assert_eq!(select(&a, &[n(0.0)]), ExcelValue::Error(ExcelError::Value));
        assert_eq!(select(&a, &[n(4.0)]), ExcelValue::Error(ExcelError::Value));
        assert_eq!(select(&a, &[n(-4.0)]), ExcelValue::Error(ExcelError::Value));
        both_eq(&a, &[n(0.0)]);
        both_eq(&a, &[n(4.0)]);
    }

    #[test]
    fn trunc_toward_zero() {
        let a = row(&[1.0, 2.0, 3.0]);
        assert_eq!(select(&a, &[n(1.9)]), ExcelValue::Array(vec![vec![n(1.0)]]));
        assert_eq!(
            select(&a, &[n(-1.9)]),
            ExcelValue::Array(vec![vec![n(3.0)]])
        );
        assert_eq!(select(&a, &[n(0.9)]), ExcelValue::Error(ExcelError::Value));
        assert_eq!(select(&a, &[n(3.9)]), ExcelValue::Array(vec![vec![n(3.0)]]));
        both_eq(&a, &[n(1.9)]);
        both_eq(&a, &[n(-1.9)]);
    }

    #[test]
    fn array_col_nums_row_major() {
        let a = row(&[1.0, 2.0, 3.0]);
        let nums = ExcelValue::Array(vec![vec![n(1.0), n(3.0)]]);
        assert_eq!(
            select(&a, &[nums.clone()]),
            ExcelValue::Array(vec![vec![n(1.0), n(3.0)]])
        );
        let col = ExcelValue::Array(vec![vec![n(1.0)], vec![n(3.0)]]);
        assert_eq!(
            select(&a, &[col]),
            ExcelValue::Array(vec![vec![n(1.0), n(3.0)]])
        );
        both_eq(&a, &[nums]);
    }

    #[test]
    fn duplicates_and_reorder() {
        let a = matrix(&[&[1.0, 2.0, 3.0, 4.0, 5.0], &[6.0, 7.0, 8.0, 9.0, 10.0]]);
        let got = select(&a, &[n(1.0), n(3.0), n(5.0), n(1.0)]);
        assert_eq!(
            got,
            ExcelValue::Array(vec![
                vec![n(1.0), n(3.0), n(5.0), n(1.0)],
                vec![n(6.0), n(8.0), n(10.0), n(6.0)],
            ])
        );
        both_eq(&a, &[n(1.0), n(3.0), n(5.0), n(1.0)]);
    }

    #[test]
    fn false_and_blank_are_value() {
        let a = row(&[1.0, 2.0]);
        assert_eq!(
            select(&a, &[ExcelValue::Bool(false)]),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            select(&a, &[ExcelValue::Empty]),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            select(&a, &[ExcelValue::Bool(true)]),
            ExcelValue::Array(vec![vec![n(1.0)]])
        );
    }

    #[test]
    fn error_in_array_is_copied() {
        let a = ExcelValue::Array(vec![vec![
            n(1.0),
            ExcelValue::Error(ExcelError::Div0),
            n(3.0),
        ]]);
        assert_eq!(
            select(&a, &[n(2.0)]),
            ExcelValue::Array(vec![vec![ExcelValue::Error(ExcelError::Div0)]])
        );
        assert_eq!(select(&a, &[n(1.0)]), ExcelValue::Array(vec![vec![n(1.0)]]));
    }

    #[test]
    fn error_in_col_num_wins() {
        let a = row(&[1.0, 2.0]);
        assert_eq!(
            select(&a, &[ExcelValue::Error(ExcelError::Na)]),
            ExcelValue::Error(ExcelError::Na)
        );
        let nums = ExcelValue::Array(vec![vec![n(1.0), ExcelValue::Error(ExcelError::Div0)]]);
        assert_eq!(select(&a, &[nums]), ExcelValue::Error(ExcelError::Div0));
    }

    #[test]
    fn scalar_array_is_one_by_one() {
        let a = n(5.0);
        assert_eq!(select(&a, &[n(1.0)]), ExcelValue::Array(vec![vec![n(5.0)]]));
        assert_eq!(
            select(&a, &[n(-1.0)]),
            ExcelValue::Array(vec![vec![n(5.0)]])
        );
        assert_eq!(select(&a, &[n(2.0)]), ExcelValue::Error(ExcelError::Value));
        both_eq(&a, &[n(1.0)]);
    }

    #[test]
    fn text_col_num_coerces() {
        let a = row(&[1.0, 2.0, 3.0]);
        assert_eq!(
            select(&a, &[ExcelValue::Text("2".into())]),
            ExcelValue::Array(vec![vec![n(2.0)]])
        );
        assert_eq!(
            select(&a, &[ExcelValue::Text("x".into())]),
            ExcelValue::Error(ExcelError::Value)
        );
    }
}
