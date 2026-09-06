//! Excel `EXPAND(array, rows, [columns], [pad_with])`.
//!
//! Grows an array to `rows` × `columns`, keeping the original block in the
//! top-left. EXPAND **cannot shrink**.
//!
//! Documented Excel behavior this module implements:
//!
//! - Omitted `pad_with` fills new cells with `#N/A`. A supplied pad (including
//!   a blank cell → empty, or an error) is written as a **value**. That is
//!   the pad/#N/A quirk: default is the `#N/A` error, not `0` and not blank.
//! - `rows` / `columns` smaller than the source (after truncate-toward-zero)
//!   is `#VALUE!`. `0` and negatives shrink. That is the shrink/#VALUE! quirk.
//!   Use `TAKE` / `DROP` to cut an array down — we do not invent a shrink
//!   success golden.
//! - Omitted **or empty** `rows` / `columns` keep the current size. Empty is
//!   **not** coerced to `0` here (a blank `0` would be `#VALUE!`). `FALSE` is
//!   `0` and therefore `#VALUE!` when the source has any rows/columns.
//! - `TRUE` → 1; numeric text coerces; other text → `#VALUE!`.
//! - Source blanks, types, and in-array errors are kept. A scalar error as
//!   `array` / `rows` / `columns` propagates left-to-right. A pad error is
//!   not a function failure.
//!
//! ## Spill / size limits (honest)
//!
//! - The engine returns an [`ExcelValue::Array`]; it does **not** write a
//!   spill range into the workbook snippet. Occupied cells below/right of
//!   the host never produce `#SPILL!`. Consume the array with `INDEX` /
//!   `SUM` / `COUNTA`.
//! - Scalar operators (`EXPAND(...)+1`) take the top-left element
//!   (`scalarize`), not a host-aware intersection of a written spill.
//! - Worksheet caps **are** enforced as `#NUM!` before allocate: output
//!   height `> 1,048,576` or width `> 16,384`. That is a size error, not
//!   occupancy `#SPILL!`. We do not invent a `#SPILL!` golden.
//! - The parser does not accept empty commas (`EXPAND(A1:B2,,4)`). Omit
//!   `columns` as a missing trailing argument, or pass a blank cell for an
//!   empty `rows` / `columns`.
//!
//! [`expand`] clones each output cell once into a pre-sized grid.
//! [`expand_naive`] clones the source, rebuilds every row (re-cloning
//! source cells), then clones pad-rows — same answers, more allocation.

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

/// Excel worksheet column cap (XFD).
pub const EXPAND_MAX_COLS: usize = 16_384;
/// Excel worksheet row cap.
pub const EXPAND_MAX_ROWS: usize = 1_048_576;

pub(crate) fn eval(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.is_empty() || args.len() > 4 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }

    // Evaluate every supplied argument (Excel does). Array error still wins
    // left-to-right over a rows/columns error.
    let array = ev.eval_expr(&args[0], ctx)?;
    let rows_v = if args.len() >= 2 {
        Some(ev.eval_scalar(&args[1], ctx)?)
    } else {
        None
    };
    let cols_v = if args.len() >= 3 {
        Some(ev.eval_scalar(&args[2], ctx)?)
    } else {
        None
    };
    let pad = if args.len() >= 4 {
        ev.eval_scalar(&args[3], ctx)?
    } else {
        ExcelValue::Error(ExcelError::Na)
    };

    if let ExcelValue::Error(e) = array {
        return Ok(ExcelValue::Error(e));
    }
    let rows = match rows_v.as_ref().map(dim_from_value).transpose() {
        Ok(n) => n.flatten(),
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let columns = match cols_v.as_ref().map(dim_from_value).transpose() {
        Ok(n) => n.flatten(),
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    Ok(expand(&array, rows, columns, &pad))
}

/// Empty / omitted dimension → keep current size. Errors propagate.
pub fn dim_from_value(v: &ExcelValue) -> Result<Option<f64>, ExcelError> {
    match v {
        ExcelValue::Empty => Ok(None),
        other => coerce::to_number(other).map(Some),
    }
}

/// Production kernel: one clone per output cell into a pre-sized grid.
pub fn expand(
    array: &ExcelValue,
    rows: Option<f64>,
    columns: Option<f64>,
    pad_with: &ExcelValue,
) -> ExcelValue {
    expand_apply(array, rows, columns, pad_with, Strategy::Fast)
}

/// Allocation-heavy baseline: clone source, rebuild rows, clone pad-rows.
///
/// Same answers as [`expand`]. Used as the bench "before".
pub fn expand_naive(
    array: &ExcelValue,
    rows: Option<f64>,
    columns: Option<f64>,
    pad_with: &ExcelValue,
) -> ExcelValue {
    expand_apply(array, rows, columns, pad_with, Strategy::Naive)
}

#[derive(Clone, Copy)]
enum Strategy {
    Fast,
    Naive,
}

fn expand_apply(
    array: &ExcelValue,
    rows: Option<f64>,
    columns: Option<f64>,
    pad_with: &ExcelValue,
    strategy: Strategy,
) -> ExcelValue {
    if let ExcelValue::Error(e) = array {
        return ExcelValue::Error(*e);
    }
    let src = match GridView::from_value(array) {
        Ok(g) => g,
        Err(e) => return ExcelValue::Error(e),
    };
    let (out_rows, out_cols) = match output_shape(src.rows, src.cols, rows, columns) {
        Ok(dims) => dims,
        Err(e) => return ExcelValue::Error(e),
    };
    match strategy {
        Strategy::Fast => fill_fast(&src, out_rows, out_cols, pad_with),
        Strategy::Naive => fill_naive(&src, out_rows, out_cols, pad_with),
    }
}

/// Truncate toward zero; omit / empty keeps `current`; shrink is `#VALUE!`.
pub fn resolve_dim(
    requested: Option<f64>,
    current: usize,
    max: usize,
) -> Result<usize, ExcelError> {
    match requested {
        None => {
            if current == 0 {
                return Err(ExcelError::Value);
            }
            if current > max {
                return Err(ExcelError::Num);
            }
            Ok(current)
        }
        Some(n) => {
            if !n.is_finite() {
                return Err(ExcelError::Num);
            }
            let t = n.trunc();
            if t < current as f64 {
                return Err(ExcelError::Value);
            }
            if t > max as f64 {
                return Err(ExcelError::Num);
            }
            Ok(t as usize)
        }
    }
}

/// `(rows, cols)` of the EXPAND result, or `#VALUE!` / `#NUM!`.
pub fn output_shape(
    src_rows: usize,
    src_cols: usize,
    rows: Option<f64>,
    columns: Option<f64>,
) -> Result<(usize, usize), ExcelError> {
    if src_rows == 0 || src_cols == 0 {
        return Err(ExcelError::Value);
    }
    let out_rows = resolve_dim(rows, src_rows, EXPAND_MAX_ROWS)?;
    let out_cols = resolve_dim(columns, src_cols, EXPAND_MAX_COLS)?;
    Ok((out_rows, out_cols))
}

struct GridView<'a> {
    rows: usize,
    cols: usize,
    kind: GridKind<'a>,
}

enum GridKind<'a> {
    Array(&'a [Vec<ExcelValue>]),
    Scalar(&'a ExcelValue),
}

impl<'a> GridView<'a> {
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
                Ok(Self {
                    rows: rows.len(),
                    cols,
                    kind: GridKind::Array(rows),
                })
            }
            other => Ok(Self {
                rows: 1,
                cols: 1,
                kind: GridKind::Scalar(other),
            }),
        }
    }

    fn get(&self, r: usize, c: usize) -> &ExcelValue {
        match self.kind {
            GridKind::Array(rows) => &rows[r][c],
            GridKind::Scalar(v) => v,
        }
    }

    fn clone_grid(&self) -> Vec<Vec<ExcelValue>> {
        match self.kind {
            GridKind::Array(rows) => rows.to_vec(),
            GridKind::Scalar(v) => vec![vec![v.clone()]],
        }
    }
}

fn fill_fast(src: &GridView<'_>, out_rows: usize, out_cols: usize, pad: &ExcelValue) -> ExcelValue {
    let mut out = Vec::with_capacity(out_rows);
    for r in 0..out_rows {
        let mut row = Vec::with_capacity(out_cols);
        for c in 0..out_cols {
            if r < src.rows && c < src.cols {
                row.push(src.get(r, c).clone());
            } else {
                row.push(pad.clone());
            }
        }
        out.push(row);
    }
    ExcelValue::Array(out)
}

/// Clone source → rebuild each row (re-clone cells + pad) → clone pad-rows.
fn fill_naive(
    src: &GridView<'_>,
    out_rows: usize,
    out_cols: usize,
    pad: &ExcelValue,
) -> ExcelValue {
    let grid = src.clone_grid();
    let mut out = Vec::with_capacity(out_rows);
    for row in &grid {
        let mut new_row = row.clone();
        new_row.resize(out_cols, pad.clone());
        out.push(new_row);
    }
    if out_rows > src.rows {
        let pad_row: Vec<ExcelValue> = (0..out_cols).map(|_| pad.clone()).collect();
        for _ in src.rows..out_rows {
            out.push(pad_row.clone());
        }
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

    fn grid2() -> ExcelValue {
        ExcelValue::Array(vec![vec![n(1.0), n(2.0)], vec![n(3.0), n(4.0)]])
    }

    fn na() -> ExcelValue {
        ExcelValue::Error(ExcelError::Na)
    }

    #[test]
    fn ms_example_2x2_to_3x3_na() {
        assert_eq!(
            expand(&grid2(), Some(3.0), Some(3.0), &na()),
            ExcelValue::Array(vec![
                vec![n(1.0), n(2.0), na()],
                vec![n(3.0), n(4.0), na()],
                vec![na(), na(), na()],
            ])
        );
    }

    #[test]
    fn ms_example_scalar_dash() {
        assert_eq!(
            expand(&n(1.0), Some(3.0), Some(3.0), &t("-")),
            ExcelValue::Array(vec![
                vec![n(1.0), t("-"), t("-")],
                vec![t("-"), t("-"), t("-")],
                vec![t("-"), t("-"), t("-")],
            ])
        );
    }

    #[test]
    fn omitted_dims_keep_shape() {
        assert_eq!(
            expand(&grid2(), None, None, &na()),
            ExcelValue::Array(vec![vec![n(1.0), n(2.0)], vec![n(3.0), n(4.0)]])
        );
        assert_eq!(
            expand(&grid2(), Some(4.0), None, &n(0.0)),
            ExcelValue::Array(vec![
                vec![n(1.0), n(2.0)],
                vec![n(3.0), n(4.0)],
                vec![n(0.0), n(0.0)],
                vec![n(0.0), n(0.0)],
            ])
        );
        assert_eq!(
            expand(&grid2(), None, Some(3.0), &n(0.0)),
            ExcelValue::Array(vec![
                vec![n(1.0), n(2.0), n(0.0)],
                vec![n(3.0), n(4.0), n(0.0)],
            ])
        );
    }

    #[test]
    fn shrink_is_value() {
        assert_eq!(
            expand(&grid2(), Some(1.0), Some(2.0), &na()),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            expand(&grid2(), Some(2.0), Some(1.0), &na()),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            expand(&grid2(), Some(0.0), Some(3.0), &na()),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            expand(&grid2(), Some(-1.0), None, &na()),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn same_size_no_pad_cells() {
        assert_eq!(
            expand(&grid2(), Some(2.0), Some(2.0), &t("x")),
            ExcelValue::Array(vec![vec![n(1.0), n(2.0)], vec![n(3.0), n(4.0)]])
        );
    }

    #[test]
    fn fraction_trunc_then_compare() {
        assert_eq!(
            expand(&grid2(), Some(3.9), Some(2.9), &na()),
            ExcelValue::Array(vec![
                vec![n(1.0), n(2.0)],
                vec![n(3.0), n(4.0)],
                vec![na(), na()],
            ])
        );
        assert_eq!(
            expand(&grid2(), Some(1.9), None, &na()),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn sheet_caps() {
        assert_eq!(
            expand(&n(1.0), Some((EXPAND_MAX_ROWS + 1) as f64), None, &na()),
            ExcelValue::Error(ExcelError::Num)
        );
        assert_eq!(
            expand(&n(1.0), None, Some((EXPAND_MAX_COLS + 1) as f64), &na()),
            ExcelValue::Error(ExcelError::Num)
        );
        assert_eq!(
            expand(&n(1.0), Some(f64::NAN), None, &na()),
            ExcelValue::Error(ExcelError::Num)
        );
        assert_eq!(
            output_shape(1, 1, Some(EXPAND_MAX_ROWS as f64), Some(1.0)).unwrap(),
            (EXPAND_MAX_ROWS, 1)
        );
        assert_eq!(
            resolve_dim(Some(16384.9), 1, EXPAND_MAX_COLS).unwrap(),
            16384
        );
        assert_eq!(
            resolve_dim(Some(16385.0), 1, EXPAND_MAX_COLS),
            Err(ExcelError::Num)
        );
    }

    #[test]
    fn scalar_error_propagates() {
        let v = ExcelValue::Error(ExcelError::Div0);
        assert_eq!(
            expand(&v, Some(3.0), Some(3.0), &na()),
            ExcelValue::Error(ExcelError::Div0)
        );
    }

    #[test]
    fn in_array_error_is_kept() {
        let v = ExcelValue::Array(vec![vec![ExcelValue::Error(ExcelError::Div0), n(1.0)]]);
        assert_eq!(
            expand(&v, Some(2.0), Some(2.0), &n(0.0)),
            ExcelValue::Array(vec![
                vec![ExcelValue::Error(ExcelError::Div0), n(1.0)],
                vec![n(0.0), n(0.0)],
            ])
        );
    }

    #[test]
    fn blanks_and_types_kept() {
        let v = ExcelValue::Array(vec![vec![
            n(1.0),
            ExcelValue::Empty,
            t("a"),
            ExcelValue::Bool(true),
        ]]);
        assert_eq!(
            expand(&v, None, None, &na()),
            ExcelValue::Array(vec![vec![
                n(1.0),
                ExcelValue::Empty,
                t("a"),
                ExcelValue::Bool(true),
            ]])
        );
    }

    #[test]
    fn dim_from_value_empty_is_omit() {
        assert_eq!(dim_from_value(&ExcelValue::Empty).unwrap(), None);
        assert_eq!(dim_from_value(&n(3.0)).unwrap(), Some(3.0));
        assert_eq!(
            dim_from_value(&ExcelValue::Error(ExcelError::Na)),
            Err(ExcelError::Na)
        );
        assert_eq!(dim_from_value(&t("x")), Err(ExcelError::Value));
    }

    #[test]
    fn naive_matches_fast() {
        let cases = [
            grid2(),
            n(9.0),
            ExcelValue::Array(vec![vec![t("A"), ExcelValue::Empty]]),
            ExcelValue::Array(vec![vec![n(1.0)], vec![ExcelValue::Error(ExcelError::Na)]]),
        ];
        for v in cases {
            for rows in [None, Some(2.0), Some(4.0)] {
                for cols in [None, Some(2.0), Some(5.0)] {
                    for pad in [na(), n(0.0), t("x")] {
                        assert_eq!(
                            expand(&v, rows, cols, &pad),
                            expand_naive(&v, rows, cols, &pad),
                            "rows={rows:?} cols={cols:?} pad={pad:?} arr={v:?}"
                        );
                    }
                }
            }
        }
    }
}
