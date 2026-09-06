//! Excel `WRAPCOLS(vector, wrap_count, [pad_with])`.
//!
//! Wraps a one-dimensional row or column into a 2-D array by filling **down**
//! each column, then starting the next column to the right.
//!
//! - `wrap_count` is the column height. Coerced like other numeric args
//!   (`TRUE` → 1, `"2"` → 2), then truncated toward zero. `< 1` → `#NUM!`.
//! - A 2-D `vector` (more than one row **and** more than one column) is
//!   `#VALUE!`. A scalar is a 1×1 vector.
//! - If `wrap_count >= n`, the vector is returned as a single column (no pad).
//! - Remainder cells in the last column use `pad_with` (default `#N/A`).
//! - Blanks stay [`ExcelValue::Empty`]; stored `""` is text. Errors inside
//!   the vector are data. A scalar error argument still surfaces.
//! - Empty vector (0 cells) → `#CALC!` (Excel cannot return a 0-size array).
//!
//! ## Spill / size limits (honest)
//!
//! - The engine returns an [`ExcelValue::Array`]; it does **not** write a
//!   spill range into the workbook snippet. `#SPILL!` from a blocked neighbor
//!   is never produced here.
//! - Scalar operators (`WRAPCOLS(...)+1`) take the top-left element
//!   (`scalarize`), not a host-aware intersection of a written spill. Consume
//!   with `INDEX` / `SUM` / `COUNTA` / `TYPE`.
//! - Excel’s worksheet caps (1,048,576 rows / 16,384 columns) are **not**
//!   enforced; a result that would `#NUM!` in live Excel is memory-bounded
//!   here. `TOCOL` / `TOROW` / `WRAPROWS` are separate workstreams.
//!
//! [`wrapcols`] places each source cell once (no intermediate flat clone, no
//! transpose). [`wrapcols_naive`] clones the grid, flattens, chunks into
//! columns, then transposes — same answers, more allocation. Used as the
//! bench "before".

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

const EMPTY: ExcelValue = ExcelValue::Empty;

/// Evaluate `WRAPCOLS` arguments and run the production kernel.
pub(crate) fn eval(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 || args.len() > 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let vector = ev.eval_expr(&args[0], ctx)?;
    let wrap_count = ev.eval_scalar(&args[1], ctx)?;
    let pad_with = if args.len() >= 3 {
        Some(ev.eval_scalar(&args[2], ctx)?)
    } else {
        None
    };
    Ok(wrapcols(&vector, &wrap_count, pad_with.as_ref()))
}

/// Excel `WRAPCOLS` from already-evaluated arguments.
pub fn wrapcols(
    vector: &ExcelValue,
    wrap_count: &ExcelValue,
    pad_with: Option<&ExcelValue>,
) -> ExcelValue {
    wrapcols_apply(vector, wrap_count, pad_with, WrapStrategy::Fast)
}

/// Allocation-heavy baseline: clone-all + flatten + column-chunk + transpose.
///
/// Same answers as [`wrapcols`]. Used as the bench "before".
pub fn wrapcols_naive(
    vector: &ExcelValue,
    wrap_count: &ExcelValue,
    pad_with: Option<&ExcelValue>,
) -> ExcelValue {
    wrapcols_apply(vector, wrap_count, pad_with, WrapStrategy::Naive)
}

#[derive(Clone, Copy)]
enum WrapStrategy {
    Fast,
    Naive,
}

fn wrapcols_apply(
    vector: &ExcelValue,
    wrap_count: &ExcelValue,
    pad_with: Option<&ExcelValue>,
    strategy: WrapStrategy,
) -> ExcelValue {
    if let ExcelValue::Error(e) = vector {
        return ExcelValue::Error(*e);
    }
    let wc = match parse_wrap_count(wrap_count) {
        Ok(n) => n,
        Err(e) => return ExcelValue::Error(e),
    };
    let view = match VectorView::from_value(vector) {
        Ok(v) => v,
        Err(e) => return ExcelValue::Error(e),
    };
    let n = view.len();
    if n == 0 {
        return ExcelValue::Error(ExcelError::Calc);
    }
    let height = wrap_height(wc, n);
    let pad = pad_value(pad_with);
    match strategy {
        WrapStrategy::Fast => wrap_fast(&view, height, &pad),
        WrapStrategy::Naive => wrap_naive(&view, height, &pad),
    }
}

fn parse_wrap_count(v: &ExcelValue) -> Result<f64, ExcelError> {
    let n = coerce::to_number(v)?;
    if !n.is_finite() {
        return Err(ExcelError::Num);
    }
    let t = n.trunc();
    if t < 1.0 {
        return Err(ExcelError::Num);
    }
    Ok(t)
}

/// Result height: `min(wrap_count, n)` — when `wrap_count >= n` Excel returns
/// the vector as a single column.
fn wrap_height(wrap_count: f64, n: usize) -> usize {
    if wrap_count >= n as f64 {
        n
    } else {
        wrap_count as usize
    }
}

fn pad_value(pad_with: Option<&ExcelValue>) -> ExcelValue {
    match pad_with {
        None => ExcelValue::Error(ExcelError::Na),
        Some(v) => coerce::scalarize(v.clone()),
    }
}

enum VectorView<'a> {
    Scalar(&'a ExcelValue),
    Row(&'a [ExcelValue]),
    Col(&'a [Vec<ExcelValue>]),
}

impl<'a> VectorView<'a> {
    fn from_value(v: &'a ExcelValue) -> Result<Self, ExcelError> {
        match v {
            ExcelValue::Error(e) => Err(*e),
            ExcelValue::Array(rows) => {
                if rows.is_empty() {
                    return Ok(Self::Row(&[]));
                }
                let width = rows[0].len();
                if rows.iter().any(|r| r.len() != width) {
                    return Err(ExcelError::Value);
                }
                if rows.len() == 1 {
                    Ok(Self::Row(&rows[0]))
                } else if width == 1 {
                    Ok(Self::Col(rows))
                } else if width == 0 {
                    Ok(Self::Row(&[]))
                } else {
                    Err(ExcelError::Value)
                }
            }
            other => Ok(Self::Scalar(other)),
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Scalar(_) => 1,
            Self::Row(row) => row.len(),
            Self::Col(rows) => rows.len(),
        }
    }

    fn get(&self, i: usize) -> &ExcelValue {
        match self {
            Self::Scalar(v) => v,
            Self::Row(row) => &row[i],
            Self::Col(rows) => rows[i].first().unwrap_or(&EMPTY),
        }
    }
}

/// Place each source cell once: fill down columns (`i % height`), no transpose.
fn wrap_fast(view: &VectorView<'_>, height: usize, pad: &ExcelValue) -> ExcelValue {
    let n = view.len();
    debug_assert!(height > 0 && height <= n);
    let cols = n.div_ceil(height);
    let mut rows: Vec<Vec<ExcelValue>> = (0..height).map(|_| Vec::with_capacity(cols)).collect();
    for i in 0..n {
        rows[i % height].push(view.get(i).clone());
    }
    if cols * height > n {
        for row in rows.iter_mut() {
            while row.len() < cols {
                row.push(pad.clone());
            }
        }
    }
    ExcelValue::Array(rows)
}

/// Clone-all flatten, chunk into columns, transpose back to row-major.
fn wrap_naive(view: &VectorView<'_>, height: usize, pad: &ExcelValue) -> ExcelValue {
    let n = view.len();
    let flat: Vec<ExcelValue> = (0..n).map(|i| view.get(i).clone()).collect();
    let cols = n.div_ceil(height);
    let mut columns: Vec<Vec<ExcelValue>> = Vec::with_capacity(cols);
    let mut col = Vec::with_capacity(height);
    for v in flat {
        col.push(v);
        if col.len() == height {
            columns.push(col);
            col = Vec::with_capacity(height);
        }
    }
    if !col.is_empty() {
        while col.len() < height {
            col.push(pad.clone());
        }
        columns.push(col);
    }
    let out: Vec<Vec<ExcelValue>> = (0..height)
        .map(|r| columns.iter().map(|c| c[r].clone()).collect())
        .collect();
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

    fn col(vals: &[f64]) -> ExcelValue {
        ExcelValue::Array(vals.iter().map(|x| vec![n(*x)]).collect())
    }

    fn row(vals: &[f64]) -> ExcelValue {
        ExcelValue::Array(vec![vals.iter().copied().map(n).collect()])
    }

    fn both_eq(vector: &ExcelValue, wc: &ExcelValue, pad: Option<&ExcelValue>) {
        assert_eq!(
            wrapcols(vector, wc, pad),
            wrapcols_naive(vector, wc, pad),
            "fast vs naive"
        );
    }

    #[test]
    fn row_wraps_down_columns() {
        let v = row(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        let got = wrapcols(&v, &n(2.0), None);
        assert_eq!(
            got,
            ExcelValue::Array(vec![
                vec![n(1.0), n(3.0), n(5.0)],
                vec![n(2.0), n(4.0), n(6.0)]
            ])
        );
        both_eq(&v, &n(2.0), None);
    }

    #[test]
    fn col_matches_row_order() {
        let c = col(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        let r = row(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(wrapcols(&c, &n(2.0), None), wrapcols(&r, &n(2.0), None));
        both_eq(&c, &n(2.0), None);
    }

    #[test]
    fn default_pad_is_na() {
        let v = row(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        let got = wrapcols(&v, &n(2.0), None);
        assert_eq!(
            got,
            ExcelValue::Array(vec![
                vec![n(1.0), n(3.0), n(5.0)],
                vec![n(2.0), n(4.0), ExcelValue::Error(ExcelError::Na)],
            ])
        );
        both_eq(&v, &n(2.0), None);
    }

    #[test]
    fn custom_pad() {
        let v = row(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        let pad = t("x");
        let got = wrapcols(&v, &n(3.0), Some(&pad));
        assert_eq!(
            got,
            ExcelValue::Array(vec![
                vec![n(1.0), n(4.0)],
                vec![n(2.0), n(5.0)],
                vec![n(3.0), t("x")],
            ])
        );
        both_eq(&v, &n(3.0), Some(&pad));
    }

    #[test]
    fn wrap_count_ge_n_is_single_column() {
        let v = row(&[1.0, 2.0, 3.0]);
        let got = wrapcols(&v, &n(10.0), Some(&t("pad")));
        assert_eq!(
            got,
            ExcelValue::Array(vec![vec![n(1.0)], vec![n(2.0)], vec![n(3.0)]])
        );
        both_eq(&v, &n(10.0), Some(&t("pad")));
    }

    #[test]
    fn wrap_count_truncates_toward_zero() {
        let v = row(&[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(wrapcols(&v, &n(2.9), None), wrapcols(&v, &n(2.0), None));
        assert_eq!(
            wrapcols(&v, &n(0.9), None),
            ExcelValue::Error(ExcelError::Num)
        );
        assert_eq!(
            wrapcols(&v, &n(0.0), None),
            ExcelValue::Error(ExcelError::Num)
        );
        assert_eq!(
            wrapcols(&v, &n(-2.0), None),
            ExcelValue::Error(ExcelError::Num)
        );
    }

    #[test]
    fn wrap_count_true_is_one() {
        let v = row(&[1.0, 2.0, 3.0]);
        let got = wrapcols(&v, &ExcelValue::Bool(true), None);
        assert_eq!(got, ExcelValue::Array(vec![vec![n(1.0), n(2.0), n(3.0)]]));
        assert_eq!(
            wrapcols(&v, &ExcelValue::Bool(false), None),
            ExcelValue::Error(ExcelError::Num)
        );
    }

    #[test]
    fn wrap_count_numeric_text() {
        let v = row(&[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(wrapcols(&v, &t("2"), None), wrapcols(&v, &n(2.0), None));
        assert_eq!(
            wrapcols(&v, &t("x"), None),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn two_d_is_value() {
        let v = ExcelValue::Array(vec![vec![n(1.0), n(2.0)], vec![n(3.0), n(4.0)]]);
        assert_eq!(
            wrapcols(&v, &n(2.0), None),
            ExcelValue::Error(ExcelError::Value)
        );
        both_eq(&v, &n(2.0), None);
    }

    #[test]
    fn scalar_is_one_by_one() {
        let v = n(7.0);
        assert_eq!(
            wrapcols(&v, &n(3.0), None),
            ExcelValue::Array(vec![vec![n(7.0)]])
        );
        both_eq(&v, &n(3.0), None);
    }

    #[test]
    fn scalar_error_surfaces() {
        let v = ExcelValue::Error(ExcelError::Div0);
        assert_eq!(
            wrapcols(&v, &n(2.0), None),
            ExcelValue::Error(ExcelError::Div0)
        );
    }

    #[test]
    fn wrap_count_error_surfaces() {
        let v = row(&[1.0, 2.0]);
        assert_eq!(
            wrapcols(&v, &ExcelValue::Error(ExcelError::Div0), None),
            ExcelValue::Error(ExcelError::Div0)
        );
    }

    #[test]
    fn error_in_vector_is_data() {
        let v = ExcelValue::Array(vec![vec![
            n(1.0),
            ExcelValue::Error(ExcelError::Div0),
            n(3.0),
        ]]);
        let got = wrapcols(&v, &n(2.0), None);
        assert_eq!(
            got,
            ExcelValue::Array(vec![
                vec![n(1.0), n(3.0)],
                vec![
                    ExcelValue::Error(ExcelError::Div0),
                    ExcelValue::Error(ExcelError::Na)
                ],
            ])
        );
        both_eq(&v, &n(2.0), None);
    }

    #[test]
    fn pad_error_is_data_not_function_error() {
        let v = row(&[1.0, 2.0, 3.0]);
        let pad = ExcelValue::Error(ExcelError::Div0);
        let got = wrapcols(&v, &n(2.0), Some(&pad));
        assert_eq!(
            got,
            ExcelValue::Array(vec![
                vec![n(1.0), n(3.0)],
                vec![n(2.0), ExcelValue::Error(ExcelError::Div0)],
            ])
        );
    }

    #[test]
    fn unused_pad_is_not_written() {
        let v = row(&[1.0, 2.0, 3.0, 4.0]);
        let pad = ExcelValue::Error(ExcelError::Div0);
        let got = wrapcols(&v, &n(2.0), Some(&pad));
        assert_eq!(
            got,
            ExcelValue::Array(vec![vec![n(1.0), n(3.0)], vec![n(2.0), n(4.0)]])
        );
    }

    #[test]
    fn blanks_and_empty_text_stay() {
        let v = ExcelValue::Array(vec![vec![n(1.0), ExcelValue::Empty, t(""), n(4.0)]]);
        let got = wrapcols(&v, &n(2.0), Some(&ExcelValue::Empty));
        assert_eq!(
            got,
            ExcelValue::Array(vec![vec![n(1.0), t("")], vec![ExcelValue::Empty, n(4.0)],])
        );
        both_eq(&v, &n(2.0), Some(&ExcelValue::Empty));
    }

    #[test]
    fn empty_vector_is_calc() {
        let v = ExcelValue::Array(Vec::new());
        assert_eq!(
            wrapcols(&v, &n(2.0), None),
            ExcelValue::Error(ExcelError::Calc)
        );
        both_eq(&v, &n(2.0), None);
    }

    #[test]
    fn inf_wrap_count_is_num() {
        let v = row(&[1.0]);
        assert_eq!(
            wrapcols(&v, &n(f64::INFINITY), None),
            ExcelValue::Error(ExcelError::Num)
        );
    }

    #[test]
    fn huge_wrap_count_is_single_column() {
        let v = row(&[1.0, 2.0]);
        let got = wrapcols(&v, &n(1e20), None);
        assert_eq!(got, ExcelValue::Array(vec![vec![n(1.0)], vec![n(2.0)]]));
        both_eq(&v, &n(1e20), None);
    }

    #[test]
    fn large_column_matches_naive() {
        let n_items = 2_048usize;
        let v = ExcelValue::Array((0..n_items).map(|i| vec![n(i as f64)]).collect());
        let pad = t("p");
        both_eq(&v, &n(17.0), Some(&pad));
        match wrapcols(&v, &n(17.0), Some(&pad)) {
            ExcelValue::Array(rows) => {
                assert_eq!(rows.len(), 17);
                assert_eq!(rows[0].len(), n_items.div_ceil(17));
                assert_eq!(rows[0][0], n(0.0));
                assert_eq!(rows[1][0], n(1.0));
            }
            other => panic!("{other:?}"),
        }
    }
}
