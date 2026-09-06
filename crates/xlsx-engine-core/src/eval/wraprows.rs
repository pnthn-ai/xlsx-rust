//! Excel `WRAPROWS(vector, wrap_count, [pad_with])`.
//!
//! Wraps a row or column of values into a 2-D array **by rows** after every
//! `wrap_count` elements. Omitted `pad_with` is `#N/A`.
//!
//! Documented Excel behavior this module implements:
//!
//! - `vector` must be a scalar or a one-dimensional array (single row **or**
//!   single column). A 2-D block is `#VALUE!`.
//! - `wrap_count` is coerced to a number and truncated toward zero. `< 1`
//!   (including `0`, blanks → `0`, `FALSE` → `0`) is `#NUM!`. Non-numeric
//!   text is `#VALUE!`. `TRUE` is `1`.
//! - Each output row has exactly `wrap_count` columns. A short last row is
//!   padded. When `wrap_count >= n` the result is a single padded row.
//! - Source blanks, types, and in-array errors are kept (not skipped).
//! - A scalar error as `vector` or `wrap_count` propagates. An error used as
//!   `pad_with` is a pad **value** (default `#N/A`), not a function failure.
//!
//! ## Spill / size limits (honest)
//!
//! - The engine returns an [`ExcelValue::Array`]; it does **not** write a
//!   spill range into the workbook snippet. Occupied cells below/right of
//!   the host never produce `#SPILL!`. Consume the array with `INDEX` /
//!   `SUM` / `COUNTA` instead of expecting a grid write.
//! - Scalar operators (`WRAPROWS(...)+1`) take the top-left element
//!   (`scalarize`), not a host-aware intersection of a written spill.
//! - Excel cannot place more than **16,384** columns or **1,048,576** rows
//!   on a worksheet. This kernel rejects those shapes with `#NUM!` *before*
//!   allocating: `wrap_count > 16,384`, or `ceil(n / wrap_count) > 1,048,576`.
//!   That matches worksheet limits; it is **not** `#SPILL!`.
//! - We do **not** invent a `#SPILL!` golden for occupancy. A fixture that
//!   needs that Excel-only outcome is documented, not faked.
//!
//! [`wraprows`] walks the source vector once and clones each cell into a
//! pre-sized row (pad cells clone `pad_with` only). [`wraprows_naive`]
//! materializes a flat copy, then a padded copy, then chunks — same
//! answers, more allocation. Used as the bench "before".

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

/// Excel worksheet column cap (XFD). `WRAPROWS` output width is `wrap_count`.
pub const WRAPROWS_MAX_COLS: usize = 16_384;
/// Excel worksheet row cap. `WRAPROWS` output height is `ceil(n / wrap_count)`.
pub const WRAPROWS_MAX_ROWS: usize = 1_048_576;

pub(crate) fn eval(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 || args.len() > 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }

    // Evaluate every supplied argument (Excel does). Vector error still wins
    // left-to-right over a wrap_count error.
    let vector = ev.eval_expr(&args[0], ctx)?;
    let wrap_v = ev.eval_scalar(&args[1], ctx)?;
    let pad = if args.len() >= 3 {
        ev.eval_scalar(&args[2], ctx)?
    } else {
        ExcelValue::Error(ExcelError::Na)
    };

    if let ExcelValue::Error(e) = vector {
        return Ok(ExcelValue::Error(e));
    }
    let wrap_count = match coerce::to_number(&wrap_v) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    Ok(wraprows(&vector, wrap_count, &pad))
}

/// Production kernel: one clone per source cell into pre-sized rows.
pub fn wraprows(vector: &ExcelValue, wrap_count: f64, pad_with: &ExcelValue) -> ExcelValue {
    wrap_apply(vector, wrap_count, pad_with, Strategy::Fast)
}

/// Allocation-heavy baseline: flatten + pad-copy + chunk.
///
/// Same answers as [`wraprows`]. Used as the bench "before".
pub fn wraprows_naive(vector: &ExcelValue, wrap_count: f64, pad_with: &ExcelValue) -> ExcelValue {
    wrap_apply(vector, wrap_count, pad_with, Strategy::Naive)
}

#[derive(Clone, Copy)]
enum Strategy {
    Fast,
    Naive,
}

fn wrap_apply(
    vector: &ExcelValue,
    wrap_count: f64,
    pad_with: &ExcelValue,
    strategy: Strategy,
) -> ExcelValue {
    if let ExcelValue::Error(e) = vector {
        return ExcelValue::Error(*e);
    }
    let wrap = match parse_wrap_count(wrap_count) {
        Ok(n) => n,
        Err(e) => return ExcelValue::Error(e),
    };
    let src = match VectorView::from_value(vector) {
        Ok(v) => v,
        Err(e) => return ExcelValue::Error(e),
    };
    let n = src.len();
    if n == 0 {
        return ExcelValue::Error(ExcelError::Value);
    }
    let (out_rows, out_cols) = match output_shape(n, wrap) {
        Ok(dims) => dims,
        Err(e) => return ExcelValue::Error(e),
    };
    debug_assert_eq!(out_cols, wrap);

    match strategy {
        Strategy::Fast => fill_fast(&src, out_rows, wrap, pad_with),
        Strategy::Naive => fill_naive(&src, out_rows, wrap, pad_with),
    }
}

/// Truncate toward zero; reject non-finite / `< 1` / wider than the sheet.
pub fn parse_wrap_count(n: f64) -> Result<usize, ExcelError> {
    if !n.is_finite() {
        return Err(ExcelError::Num);
    }
    let t = n.trunc();
    if t < 1.0 {
        return Err(ExcelError::Num);
    }
    if t > WRAPROWS_MAX_COLS as f64 {
        return Err(ExcelError::Num);
    }
    Ok(t as usize)
}

/// `(rows, cols)` of the WRAPROWS result, or `#NUM!` if it cannot fit a sheet.
pub fn output_shape(n: usize, wrap: usize) -> Result<(usize, usize), ExcelError> {
    if wrap == 0 || wrap > WRAPROWS_MAX_COLS {
        return Err(ExcelError::Num);
    }
    if n == 0 {
        return Err(ExcelError::Value);
    }
    let rows = n.div_ceil(wrap);
    if rows > WRAPROWS_MAX_ROWS {
        return Err(ExcelError::Num);
    }
    Ok((rows, wrap))
}

enum VectorView<'a> {
    Row(&'a [ExcelValue]),
    Col(&'a [Vec<ExcelValue>]),
    Scalar(&'a ExcelValue),
}

impl<'a> VectorView<'a> {
    fn from_value(v: &'a ExcelValue) -> Result<Self, ExcelError> {
        match v {
            ExcelValue::Error(e) => Err(*e),
            ExcelValue::Array(rows) => {
                if rows.is_empty() {
                    return Err(ExcelError::Value);
                }
                let cols = rows[0].len();
                if cols == 0 || rows.iter().any(|r| r.len() != cols) {
                    return Err(ExcelError::Value);
                }
                let nr = rows.len();
                if nr > 1 && cols > 1 {
                    return Err(ExcelError::Value);
                }
                if nr == 1 {
                    Ok(Self::Row(&rows[0]))
                } else {
                    Ok(Self::Col(rows))
                }
            }
            other => Ok(Self::Scalar(other)),
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Row(r) => r.len(),
            Self::Col(rows) => rows.len(),
            Self::Scalar(_) => 1,
        }
    }

    fn get(&self, i: usize) -> &ExcelValue {
        match self {
            Self::Row(r) => &r[i],
            Self::Col(rows) => &rows[i][0],
            Self::Scalar(v) => v,
        }
    }
}

fn fill_fast(src: &VectorView<'_>, out_rows: usize, wrap: usize, pad: &ExcelValue) -> ExcelValue {
    let n = src.len();
    let mut out = Vec::with_capacity(out_rows);
    let mut i = 0usize;
    for _ in 0..out_rows {
        let mut row = Vec::with_capacity(wrap);
        for _ in 0..wrap {
            if i < n {
                row.push(src.get(i).clone());
                i += 1;
            } else {
                row.push(pad.clone());
            }
        }
        out.push(row);
    }
    ExcelValue::Array(out)
}

/// Flatten (clone all) → padded flat (clone again) → chunked rows (clone again).
fn fill_naive(src: &VectorView<'_>, out_rows: usize, wrap: usize, pad: &ExcelValue) -> ExcelValue {
    let mut flat: Vec<ExcelValue> = (0..src.len()).map(|i| src.get(i).clone()).collect();
    let need = out_rows * wrap;
    while flat.len() < need {
        flat.push(pad.clone());
    }
    let mut out = Vec::new();
    for chunk in flat.chunks(wrap) {
        out.push(chunk.to_vec());
    }
    ExcelValue::Array(out)
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

    fn row(vals: &[ExcelValue]) -> ExcelValue {
        ExcelValue::Array(vec![vals.to_vec()])
    }

    fn na() -> ExcelValue {
        ExcelValue::Error(ExcelError::Na)
    }

    #[test]
    fn row_wraps_by_rows() {
        let v = row(&[n(1.0), n(2.0), n(3.0), n(4.0), n(5.0), n(6.0)]);
        assert_eq!(
            wraprows(&v, 3.0, &na()),
            ExcelValue::Array(vec![
                vec![n(1.0), n(2.0), n(3.0)],
                vec![n(4.0), n(5.0), n(6.0)],
            ])
        );
    }

    #[test]
    fn column_flattens_top_to_bottom() {
        let v = col(&[n(1.0), n(2.0), n(3.0), n(4.0), n(5.0)]);
        assert_eq!(
            wraprows(&v, 2.0, &na()),
            ExcelValue::Array(vec![
                vec![n(1.0), n(2.0)],
                vec![n(3.0), n(4.0)],
                vec![n(5.0), na()],
            ])
        );
    }

    #[test]
    fn pad_with_custom_and_default() {
        let v = row(&[n(1.0), n(2.0), n(3.0)]);
        assert_eq!(
            wraprows(&v, 2.0, &n(0.0)),
            ExcelValue::Array(vec![vec![n(1.0), n(2.0)], vec![n(3.0), n(0.0)]])
        );
        assert_eq!(
            wraprows(&v, 2.0, &na()),
            ExcelValue::Array(vec![vec![n(1.0), n(2.0)], vec![n(3.0), na()]])
        );
    }

    #[test]
    fn wrap_ge_n_is_one_padded_row() {
        let v = row(&[n(1.0), n(2.0)]);
        assert_eq!(
            wraprows(&v, 4.0, &t("x")),
            ExcelValue::Array(vec![vec![n(1.0), n(2.0), t("x"), t("x")]])
        );
        assert_eq!(
            wraprows(&v, 2.0, &na()),
            ExcelValue::Array(vec![vec![n(1.0), n(2.0)]])
        );
    }

    #[test]
    fn two_d_is_value() {
        let v = ExcelValue::Array(vec![vec![n(1.0), n(2.0)], vec![n(3.0), n(4.0)]]);
        assert_eq!(
            wraprows(&v, 2.0, &na()),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn wrap_count_domain() {
        let v = n(1.0);
        assert_eq!(wraprows(&v, 0.0, &na()), ExcelValue::Error(ExcelError::Num));
        assert_eq!(
            wraprows(&v, -1.0, &na()),
            ExcelValue::Error(ExcelError::Num)
        );
        assert_eq!(wraprows(&v, 0.9, &na()), ExcelValue::Error(ExcelError::Num));
        assert_eq!(
            wraprows(&v, f64::NAN, &na()),
            ExcelValue::Error(ExcelError::Num)
        );
        assert_eq!(
            wraprows(&v, 2.9, &na()),
            ExcelValue::Array(vec![vec![n(1.0), na()]])
        );
        assert_eq!(
            wraprows(&v, 1.0, &na()),
            ExcelValue::Array(vec![vec![n(1.0)]])
        );
    }

    #[test]
    fn sheet_width_cap() {
        let v = n(1.0);
        assert_eq!(
            wraprows(&v, (WRAPROWS_MAX_COLS + 1) as f64, &na()),
            ExcelValue::Error(ExcelError::Num)
        );
        match wraprows(&v, WRAPROWS_MAX_COLS as f64, &na()) {
            ExcelValue::Array(rows) => {
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].len(), WRAPROWS_MAX_COLS);
                assert_eq!(rows[0][0], n(1.0));
                assert_eq!(rows[0][WRAPROWS_MAX_COLS - 1], na());
            }
            other => panic!("expected 1×{WRAPROWS_MAX_COLS} array, got {other:?}"),
        }
    }

    #[test]
    fn sheet_height_cap_via_shape() {
        assert_eq!(
            output_shape(WRAPROWS_MAX_ROWS, 1).unwrap(),
            (WRAPROWS_MAX_ROWS, 1)
        );
        assert_eq!(output_shape(WRAPROWS_MAX_ROWS + 1, 1), Err(ExcelError::Num));
        assert_eq!(parse_wrap_count(16384.9).unwrap(), 16384);
        assert_eq!(parse_wrap_count(16385.0), Err(ExcelError::Num));
    }

    #[test]
    fn scalar_error_propagates_not_wrapped() {
        let v = ExcelValue::Error(ExcelError::Div0);
        assert_eq!(
            wraprows(&v, 2.0, &na()),
            ExcelValue::Error(ExcelError::Div0)
        );
    }

    #[test]
    fn in_array_error_is_kept() {
        let v = col(&[ExcelValue::Error(ExcelError::Div0), n(1.0)]);
        assert_eq!(
            wraprows(&v, 2.0, &na()),
            ExcelValue::Array(vec![vec![ExcelValue::Error(ExcelError::Div0), n(1.0)]])
        );
    }

    #[test]
    fn blanks_and_types_kept() {
        let v = col(&[n(1.0), ExcelValue::Empty, t("a"), ExcelValue::Bool(true)]);
        assert_eq!(
            wraprows(&v, 2.0, &n(0.0)),
            ExcelValue::Array(vec![
                vec![n(1.0), ExcelValue::Empty],
                vec![t("a"), ExcelValue::Bool(true)],
            ])
        );
    }

    #[test]
    fn naive_matches_fast() {
        let cases = [
            row(&[n(1.0), n(2.0), n(3.0), n(4.0), n(5.0)]),
            col(&[n(1.0), n(2.0), n(3.0), n(4.0), n(5.0)]),
            n(9.0),
            col(&[t("A"), ExcelValue::Empty, ExcelValue::Error(ExcelError::Na)]),
        ];
        for v in cases {
            for wrap in [1.0, 2.0, 3.0, 8.0] {
                for pad in [na(), n(0.0), t("x")] {
                    assert_eq!(
                        wraprows(&v, wrap, &pad),
                        wraprows_naive(&v, wrap, &pad),
                        "wrap={wrap} pad={pad:?} vec={v:?}"
                    );
                }
            }
        }
    }
}
