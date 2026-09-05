//! Excel `DROP(array, rows, [cols])`.
//!
//! Removes `rows` from the top (or, if negative, the bottom) and `cols` from
//! the left (or, if negative, the right). The result is always an
//! [`ExcelValue::Array`] (including 1×1). This engine does **not** write a
//! spill range into the sheet, so occupied neighbors never produce `#SPILL!`.
//!
//! Documented Excel quirks this module implements:
//!
//! - Positive counts drop from the start; negative counts drop from the end.
//! - Omitted `cols` is `0` (no-op on that axis). `0` on an axis is a no-op.
//!   Microsoft's DROP page says `#CALC!` "when rows or columns is 0" — that
//!   contradicts their own examples (`=DROP(A2:C4,2)` omits columns;
//!   `=DROP(A2:C4,,2)` omits rows) and the useful `=DROP(data,0)` no-op.
//!   `#CALC!` is reserved for an **empty result**: `|rows| >= height` or
//!   `|cols| >= width`.
//! - At least one of `rows` / `cols` must be supplied (`DROP(array)` is
//!   `#VALUE!`).
//! - Counts truncate toward zero (`1.9` → 1, `-1.9` → −1). `|n| < 1` is 0.
//! - Non-numeric counts → `#VALUE!`; non-finite → `#NUM!`.
//! - Errors inside the array are ordinary cells and survive if not dropped.
//!   A scalar error `array` argument surfaces as that error.
//!
//! ## Spill / model limits
//!
//! - Result is an array **value**. The snippet workbook has no spill grid, so
//!   a blocked cell below/right of the host never yields `#SPILL!`.
//! - Scalar operators still take the top-left element (`DROP(...)+1`).
//! - Excel's ~1,048,576-row / 16,384-column array cap is not enforced; size
//!   is memory-bounded (live Excel would `#NUM!` on a too-large array).
//! - The parser does not accept omitted-middle arguments (`DROP(a,,2)`). Use
//!   `DROP(a,0,2)`.
//!
//! [`apply`] clones **only** the kept rectangle (keep-all can move the grid).
//! [`apply_naive`] clones the whole matrix, then `drain`s dropped edges —
//! same answers, more allocation. Used as the bench "before".

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

/// Excel `DROP` from already-evaluated `array` / counts.
pub fn apply(array: &ExcelValue, rows: f64, cols: f64) -> ExcelValue {
    drop_kernel(array, rows, cols, DropStrategy::Fast)
}

/// Allocation-heavy baseline: clone everything, then drain dropped edges.
///
/// Same answers as [`apply`]. Used as the bench "before".
pub fn apply_naive(array: &ExcelValue, rows: f64, cols: f64) -> ExcelValue {
    drop_kernel(array, rows, cols, DropStrategy::Naive)
}

#[derive(Clone, Copy)]
enum DropStrategy {
    Fast,
    Naive,
}

pub(crate) fn eval(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 || args.len() > 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }

    let array = ev.eval_expr(&args[0], ctx)?;
    if let ExcelValue::Error(e) = array {
        return Ok(ExcelValue::Error(e));
    }

    let rows = match count_arg(ev, &args[1], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let cols = if args.len() >= 3 {
        match count_arg(ev, &args[2], ctx)? {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        0.0
    };

    Ok(apply(&array, rows, cols))
}

fn count_arg(
    ev: &Evaluator,
    expr: &Expr,
    ctx: &mut Ctx<'_>,
) -> Result<Result<f64, ExcelError>, EvalError> {
    let v = ev.eval_scalar(expr, ctx)?;
    Ok(coerce::to_number(&v))
}

fn drop_kernel(array: &ExcelValue, rows: f64, cols: f64, strategy: DropStrategy) -> ExcelValue {
    match array {
        ExcelValue::Error(e) => ExcelValue::Error(*e),
        ExcelValue::Array(grid) => drop_grid(grid, rows, cols, strategy),
        other => {
            let one = vec![vec![other.clone()]];
            drop_grid(&one, rows, cols, strategy)
        }
    }
}

fn drop_grid(grid: &[Vec<ExcelValue>], rows: f64, cols: f64, strategy: DropStrategy) -> ExcelValue {
    let height = grid.len();
    let width = grid.first().map(|r| r.len()).unwrap_or(0);
    if height == 0 || width == 0 {
        return ExcelValue::Error(ExcelError::Calc);
    }
    if grid.iter().any(|r| r.len() != width) {
        return ExcelValue::Error(ExcelError::Value);
    }

    let (r0, r1) = match axis_span(height, rows) {
        Ok(s) => s,
        Err(e) => return ExcelValue::Error(e),
    };
    let (c0, c1) = match axis_span(width, cols) {
        Ok(s) => s,
        Err(e) => return ExcelValue::Error(e),
    };

    match strategy {
        DropStrategy::Fast => take_rect(grid, r0, r1, c0, c1),
        DropStrategy::Naive => take_rect_naive(grid, r0, r1, c0, c1),
    }
}

/// Kept half-open span on one axis. `count == 0` is a no-op.
/// `|count| >= dim` (after toward-zero truncate) is `#CALC!`.
fn axis_span(dim: usize, count: f64) -> Result<(usize, usize), ExcelError> {
    if !count.is_finite() {
        return Err(ExcelError::Num);
    }
    if dim == 0 {
        return Err(ExcelError::Calc);
    }
    let truncated = count.trunc();
    if truncated == 0.0 {
        return Ok((0, dim));
    }
    let abs = truncated.abs();
    if abs >= dim as f64 {
        return Err(ExcelError::Calc);
    }
    let k = abs as usize;
    if truncated > 0.0 {
        Ok((k, dim))
    } else {
        Ok((0, dim - k))
    }
}

fn take_rect(grid: &[Vec<ExcelValue>], r0: usize, r1: usize, c0: usize, c1: usize) -> ExcelValue {
    let mut out = Vec::with_capacity(r1 - r0);
    for row in &grid[r0..r1] {
        out.push(row[c0..c1].to_vec());
    }
    ExcelValue::Array(out)
}

/// Naive: clone the full matrix (including cells that will be discarded),
/// then `drain` dropped row / column edges.
fn take_rect_naive(
    grid: &[Vec<ExcelValue>],
    r0: usize,
    r1: usize,
    c0: usize,
    c1: usize,
) -> ExcelValue {
    let mut all = grid.to_vec();
    let height = all.len();
    if r1 < height {
        all.drain(r1..);
    }
    if r0 > 0 {
        all.drain(..r0);
    }
    for row in &mut all {
        let width = row.len();
        if c1 < width {
            row.drain(c1..);
        }
        if c0 > 0 {
            row.drain(..c0);
        }
    }
    ExcelValue::Array(all)
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
        ExcelValue::Array(vec![vals.iter().copied().map(n).collect()])
    }
    fn matrix(rows: &[&[f64]]) -> ExcelValue {
        ExcelValue::Array(
            rows.iter()
                .map(|r| r.iter().copied().map(n).collect())
                .collect(),
        )
    }

    fn both_eq(array: &ExcelValue, rows: f64, cols: f64) {
        assert_eq!(
            apply(array, rows, cols),
            apply_naive(array, rows, cols),
            "fast vs naive mismatch rows={rows} cols={cols}"
        );
    }

    #[test]
    fn drop_first_rows() {
        let a = col(&[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(apply(&a, 1.0, 0.0), col(&[2.0, 3.0, 4.0]));
        assert_eq!(apply(&a, 2.0, 0.0), col(&[3.0, 4.0]));
        both_eq(&a, 1.0, 0.0);
        both_eq(&a, 2.0, 0.0);
    }

    #[test]
    fn drop_last_rows_negative() {
        let a = col(&[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(apply(&a, -1.0, 0.0), col(&[1.0, 2.0, 3.0]));
        assert_eq!(apply(&a, -2.0, 0.0), col(&[1.0, 2.0]));
        both_eq(&a, -1.0, 0.0);
        both_eq(&a, -2.0, 0.0);
    }

    #[test]
    fn drop_first_and_last_cols() {
        let a = row(&[1.0, 2.0, 3.0]);
        assert_eq!(apply(&a, 0.0, 1.0), row(&[2.0, 3.0]));
        assert_eq!(apply(&a, 0.0, -1.0), row(&[1.0, 2.0]));
        both_eq(&a, 0.0, 1.0);
        both_eq(&a, 0.0, -1.0);
    }

    #[test]
    fn matrix_both_axes() {
        let a = matrix(&[&[1.0, 2.0, 3.0], &[4.0, 5.0, 6.0], &[7.0, 8.0, 9.0]]);
        assert_eq!(apply(&a, 1.0, 1.0), matrix(&[&[5.0, 6.0], &[8.0, 9.0]]));
        assert_eq!(apply(&a, -1.0, -1.0), matrix(&[&[1.0, 2.0], &[4.0, 5.0]]));
        assert_eq!(apply(&a, -1.0, 1.0), matrix(&[&[2.0, 3.0], &[5.0, 6.0]]));
        both_eq(&a, 1.0, 1.0);
        both_eq(&a, -1.0, -1.0);
        both_eq(&a, -1.0, 1.0);
    }

    #[test]
    fn zero_is_noop_not_calc() {
        let a = col(&[1.0, 2.0, 3.0]);
        assert_eq!(apply(&a, 0.0, 0.0), a);
        both_eq(&a, 0.0, 0.0);
    }

    #[test]
    fn empty_result_is_calc() {
        let a = col(&[1.0, 2.0, 3.0]);
        assert_eq!(apply(&a, 3.0, 0.0), ExcelValue::Error(ExcelError::Calc));
        assert_eq!(apply(&a, 4.0, 0.0), ExcelValue::Error(ExcelError::Calc));
        assert_eq!(apply(&a, -3.0, 0.0), ExcelValue::Error(ExcelError::Calc));
        let row = row(&[1.0, 2.0]);
        assert_eq!(apply(&row, 0.0, 2.0), ExcelValue::Error(ExcelError::Calc));
        // One-row vector: dropping the only row is empty.
        assert_eq!(apply(&row, 1.0, 0.0), ExcelValue::Error(ExcelError::Calc));
        both_eq(&a, 3.0, 0.0);
        both_eq(&row, 0.0, 2.0);
    }

    #[test]
    fn toward_zero_truncate() {
        let a = col(&[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(apply(&a, 1.9, 0.0), col(&[2.0, 3.0, 4.0]));
        assert_eq!(apply(&a, -1.9, 0.0), col(&[1.0, 2.0, 3.0]));
        assert_eq!(apply(&a, 0.9, 0.0), a);
        assert_eq!(apply(&a, -0.9, 0.0), a);
        both_eq(&a, 1.9, 0.0);
        both_eq(&a, -1.9, 0.0);
    }

    #[test]
    fn non_finite_count_is_num() {
        let a = col(&[1.0, 2.0]);
        assert_eq!(apply(&a, f64::NAN, 0.0), ExcelValue::Error(ExcelError::Num));
        assert_eq!(
            apply(&a, f64::INFINITY, 0.0),
            ExcelValue::Error(ExcelError::Num)
        );
    }

    #[test]
    fn scalar_zero_is_1x1() {
        let a = n(5.0);
        assert_eq!(apply(&a, 0.0, 0.0), ExcelValue::Array(vec![vec![n(5.0)]]));
        assert_eq!(apply(&a, 1.0, 0.0), ExcelValue::Error(ExcelError::Calc));
        both_eq(&a, 0.0, 0.0);
    }

    #[test]
    fn keeps_errors_inside_array() {
        let a = ExcelValue::Array(vec![
            vec![ExcelValue::Error(ExcelError::Div0)],
            vec![n(2.0)],
            vec![n(3.0)],
        ]);
        assert_eq!(
            apply(&a, 1.0, 0.0),
            ExcelValue::Array(vec![vec![n(2.0)], vec![n(3.0)]])
        );
        assert_eq!(
            apply(&a, -2.0, 0.0),
            ExcelValue::Array(vec![vec![ExcelValue::Error(ExcelError::Div0)]])
        );
        both_eq(&a, 1.0, 0.0);
    }

    #[test]
    fn scalar_error_surfaces() {
        let a = ExcelValue::Error(ExcelError::Div0);
        assert_eq!(apply(&a, 0.0, 0.0), ExcelValue::Error(ExcelError::Div0));
    }

    #[test]
    fn large_rect_matches_naive() {
        let n_rows = 2_048usize;
        let n_cols = 8usize;
        let rows: Vec<Vec<ExcelValue>> = (0..n_rows)
            .map(|i| (0..n_cols).map(|c| n((i * n_cols + c) as f64)).collect())
            .collect();
        let array = ExcelValue::Array(rows);
        for (r, c) in [(64.0, 0.0), (-64.0, 0.0), (0.0, 2.0), (32.0, -2.0)] {
            both_eq(&array, r, c);
        }
        match apply(&array, 64.0, 2.0) {
            ExcelValue::Array(out) => {
                assert_eq!(out.len(), n_rows - 64);
                assert_eq!(out[0].len(), n_cols - 2);
                assert_eq!(out[0][0], n((64 * n_cols + 2) as f64));
            }
            other => panic!("{other:?}"),
        }
    }
}
