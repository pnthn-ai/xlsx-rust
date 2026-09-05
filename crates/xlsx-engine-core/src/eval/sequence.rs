//! Excel `SEQUENCE(rows, [columns], [start], [step])`.
//!
//! Generates a row-major numeric array:
//! `value[r, c] = start + (r * columns + c) * step`.
//!
//! Documented Excel behavior this module implements:
//!
//! - Omitted `columns` / `start` / `step` default to `1`.
//! - `rows` is required: `SEQUENCE()` is `#VALUE!`. More than 4 args is `#VALUE!`.
//! - `rows` / `columns` truncate toward zero. After truncation, `0` is `#CALC!`
//!   (Excel cannot return an empty array) and a negative size is `#VALUE!`.
//! - Non-numeric arguments are `#VALUE!`. Errors evaluate left-to-right.
//! - `start` / `step` may be any finite number (negative, zero, fractional).
//! - The result is always an [`ExcelValue::Array`], including `1×1`.
//!
//! ## Spill / size limits
//!
//! - The engine returns an array **value**. It does **not** write a spill
//!   range into the workbook snippet, so a blocked cell below/right of the
//!   host never yields `#SPILL!`. Consume the array with `INDEX` / `SUM` /
//!   `COUNTA`.
//! - Excel would `#SPILL!` if the sequence could not fit from the host cell
//!   to the sheet edge (`1,048,576` rows × `16,384` columns). That sheet-edge
//!   cap is **not** enforced here — there is no spill grid.
//! - A safety cap of [`MAX_CELLS`] (`2^24` = 16,777,216) elements prevents
//!   unbounded allocation (`SEQUENCE(1E20)` / `SEQUENCE(5000,5000)`). Oversize
//!   is `#VALUE!`, not `#SPILL!`. This is a model limit, not a hidden ignore.
//!
//! [`sequence`] pre-sizes the grid and uses an `i64` increment when `start`
//! and `step` are exact integers inside the binary-safe range. [`sequence_naive`]
//! grows `Vec`s without capacity and walks with `f64` multiply — same answers,
//! more allocator traffic. Used as the bench "before".

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

/// Dynamic-array element safety cap (`2^24`). Product of truncated
/// `rows * columns` above this is `#VALUE!`.
pub const MAX_CELLS: usize = 16_777_216;

/// Production kernel. Used by the evaluator and by benches.
pub fn sequence(rows: f64, columns: f64, start: f64, step: f64) -> ExcelValue {
    sequence_apply(rows, columns, start, step, SequenceStrategy::Fast)
}

/// Allocation-heavy baseline: no `with_capacity`, no integer fast path.
///
/// Same answers as [`sequence`]. Used as the bench "before".
pub fn sequence_naive(rows: f64, columns: f64, start: f64, step: f64) -> ExcelValue {
    sequence_apply(rows, columns, start, step, SequenceStrategy::Naive)
}

#[derive(Clone, Copy)]
enum SequenceStrategy {
    Fast,
    Naive,
}

pub(crate) fn eval(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.is_empty() || args.len() > 4 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }

    let rows = match num_arg(ev, &args[0], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let columns = if args.len() >= 2 {
        match num_arg(ev, &args[1], ctx)? {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        1.0
    };
    let start = if args.len() >= 3 {
        match num_arg(ev, &args[2], ctx)? {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        1.0
    };
    let step = if args.len() >= 4 {
        match num_arg(ev, &args[3], ctx)? {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        1.0
    };

    Ok(sequence(rows, columns, start, step))
}

fn num_arg(
    ev: &Evaluator,
    expr: &Expr,
    ctx: &mut Ctx<'_>,
) -> Result<Result<f64, ExcelError>, EvalError> {
    let v = ev.eval_scalar(expr, ctx)?;
    Ok(coerce::to_number(&v))
}

fn sequence_apply(
    rows: f64,
    columns: f64,
    start: f64,
    step: f64,
    strategy: SequenceStrategy,
) -> ExcelValue {
    let nrows = match dimension(rows) {
        Ok(n) => n,
        Err(e) => return ExcelValue::Error(e),
    };
    let ncols = match dimension(columns) {
        Ok(n) => n,
        Err(e) => return ExcelValue::Error(e),
    };
    if !start.is_finite() || !step.is_finite() {
        return ExcelValue::Error(ExcelError::Num);
    }
    let ncells = match nrows.checked_mul(ncols) {
        Some(n) if n <= MAX_CELLS => n,
        _ => return ExcelValue::Error(ExcelError::Value),
    };

    match strategy {
        SequenceStrategy::Fast => fill_fast(nrows, ncols, ncells, start, step),
        SequenceStrategy::Naive => fill_naive(nrows, ncols, start, step),
    }
}

fn dimension(n: f64) -> Result<usize, ExcelError> {
    if !n.is_finite() {
        return Err(ExcelError::Num);
    }
    let t = n.trunc();
    if t < 0.0 {
        return Err(ExcelError::Value);
    }
    if t == 0.0 {
        return Err(ExcelError::Calc);
    }
    if t > MAX_CELLS as f64 {
        return Err(ExcelError::Value);
    }
    Ok(t as usize)
}

/// `i64` increment is exact when every cell is an integer whose `f64`
/// representation is unique (`|n| < 2^53`).
const SAFE_INT: f64 = (1u64 << 53) as f64;

fn integer_step(start: f64, step: f64, ncells: usize) -> Option<(i64, i64)> {
    if start.fract() != 0.0 || step.fract() != 0.0 {
        return None;
    }
    if start.abs() >= SAFE_INT || step.abs() >= SAFE_INT {
        return None;
    }
    let last = start + (ncells.saturating_sub(1) as f64) * step;
    if !last.is_finite() || last.abs() >= SAFE_INT {
        return None;
    }
    Some((start as i64, step as i64))
}

fn fill_fast(nrows: usize, ncols: usize, ncells: usize, start: f64, step: f64) -> ExcelValue {
    let mut rows = Vec::with_capacity(nrows);
    if let Some((start_i, step_i)) = integer_step(start, step, ncells) {
        let mut v = start_i;
        for _ in 0..nrows {
            let mut row = Vec::with_capacity(ncols);
            for _ in 0..ncols {
                row.push(ExcelValue::Number(v as f64));
                v += step_i;
            }
            rows.push(row);
        }
    } else {
        let mut i = 0usize;
        for _ in 0..nrows {
            let mut row = Vec::with_capacity(ncols);
            for _ in 0..ncols {
                row.push(ExcelValue::Number(start + (i as f64) * step));
                i += 1;
            }
            rows.push(row);
        }
    }
    ExcelValue::Array(rows)
}

fn fill_naive(nrows: usize, ncols: usize, start: f64, step: f64) -> ExcelValue {
    let mut rows = Vec::new();
    let mut i = 0usize;
    for _ in 0..nrows {
        let mut row = Vec::new();
        for _ in 0..ncols {
            row.push(ExcelValue::Number(start + (i as f64) * step));
            i += 1;
        }
        rows.push(row);
    }
    ExcelValue::Array(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(x: f64) -> ExcelValue {
        ExcelValue::Number(x)
    }

    fn col(vals: &[f64]) -> ExcelValue {
        ExcelValue::Array(vals.iter().copied().map(|v| vec![n(v)]).collect())
    }

    fn row(vals: &[f64]) -> ExcelValue {
        ExcelValue::Array(vec![vals.iter().copied().map(n).collect()])
    }

    fn both_eq(rows: f64, cols: f64, start: f64, step: f64) -> ExcelValue {
        let a = sequence(rows, cols, start, step);
        let b = sequence_naive(rows, cols, start, step);
        assert_eq!(a, b, "fast vs naive mismatch");
        a
    }

    #[test]
    fn column_default() {
        assert_eq!(both_eq(4.0, 1.0, 1.0, 1.0), col(&[1.0, 2.0, 3.0, 4.0]));
    }

    #[test]
    fn row_vector() {
        assert_eq!(both_eq(1.0, 5.0, 1.0, 1.0), row(&[1.0, 2.0, 3.0, 4.0, 5.0]));
    }

    #[test]
    fn matrix_row_major() {
        assert_eq!(
            both_eq(2.0, 3.0, 1.0, 1.0),
            ExcelValue::Array(vec![
                vec![n(1.0), n(2.0), n(3.0)],
                vec![n(4.0), n(5.0), n(6.0)]
            ])
        );
    }

    #[test]
    fn start_and_step() {
        assert_eq!(
            both_eq(4.0, 5.0, 100.0, 10.0),
            ExcelValue::Array(vec![
                vec![n(100.0), n(110.0), n(120.0), n(130.0), n(140.0)],
                vec![n(150.0), n(160.0), n(170.0), n(180.0), n(190.0)],
                vec![n(200.0), n(210.0), n(220.0), n(230.0), n(240.0)],
                vec![n(250.0), n(260.0), n(270.0), n(280.0), n(290.0)],
            ])
        );
    }

    #[test]
    fn negative_step() {
        assert_eq!(both_eq(3.0, 1.0, 10.0, -2.0), col(&[10.0, 8.0, 6.0]));
    }

    #[test]
    fn zero_step_repeats_start() {
        assert_eq!(
            both_eq(2.0, 2.0, 5.0, 0.0),
            ExcelValue::Array(vec![vec![n(5.0), n(5.0)], vec![n(5.0), n(5.0)]])
        );
    }

    #[test]
    fn fractional_step() {
        assert_eq!(both_eq(3.0, 1.0, 1.0, 0.5), col(&[1.0, 1.5, 2.0]));
    }

    #[test]
    fn one_by_one_is_array() {
        assert_eq!(
            both_eq(1.0, 1.0, 7.0, 1.0),
            ExcelValue::Array(vec![vec![n(7.0)]])
        );
    }

    #[test]
    fn truncates_toward_zero() {
        assert_eq!(both_eq(2.9, 1.0, 1.0, 1.0), col(&[1.0, 2.0]));
        assert_eq!(both_eq(1.0, 3.7, 1.0, 1.0), row(&[1.0, 2.0, 3.0]));
    }

    #[test]
    fn zero_dim_is_calc() {
        assert_eq!(
            sequence(0.0, 1.0, 1.0, 1.0),
            ExcelValue::Error(ExcelError::Calc)
        );
        assert_eq!(
            sequence(1.0, 0.0, 1.0, 1.0),
            ExcelValue::Error(ExcelError::Calc)
        );
        assert_eq!(
            sequence(0.5, 1.0, 1.0, 1.0),
            ExcelValue::Error(ExcelError::Calc)
        );
        assert_eq!(
            sequence(-0.5, 1.0, 1.0, 1.0),
            ExcelValue::Error(ExcelError::Calc)
        );
    }

    #[test]
    fn negative_dim_is_value() {
        assert_eq!(
            sequence(-1.0, 1.0, 1.0, 1.0),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            sequence(1.0, -2.0, 1.0, 1.0),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            sequence(-1.9, 1.0, 1.0, 1.0),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn oversize_is_value_without_allocating() {
        assert_eq!(
            sequence((MAX_CELLS as f64) + 1.0, 1.0, 1.0, 1.0),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            sequence(1.0, (MAX_CELLS as f64) + 1.0, 1.0, 1.0),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            sequence(5_000.0, 5_000.0, 1.0, 1.0),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            sequence(1e20, 1.0, 1.0, 1.0),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn non_finite_is_num() {
        assert_eq!(
            sequence(f64::INFINITY, 1.0, 1.0, 1.0),
            ExcelValue::Error(ExcelError::Num)
        );
        assert_eq!(
            sequence(1.0, 1.0, f64::NAN, 1.0),
            ExcelValue::Error(ExcelError::Num)
        );
    }

    #[test]
    fn large_integer_column_matches_naive() {
        let n_rows = 4_096.0;
        let got = both_eq(n_rows, 3.0, 10.0, 2.0);
        match got {
            ExcelValue::Array(rows) => {
                assert_eq!(rows.len(), 4_096);
                assert_eq!(rows[0], vec![n(10.0), n(12.0), n(14.0)]);
                assert_eq!(
                    rows[4_095],
                    vec![
                        n(10.0 + 4_095.0 * 3.0 * 2.0),
                        n(12.0 + 4_095.0 * 3.0 * 2.0),
                        n(14.0 + 4_095.0 * 3.0 * 2.0)
                    ]
                );
            }
            other => panic!("{other:?}"),
        }
    }
}
