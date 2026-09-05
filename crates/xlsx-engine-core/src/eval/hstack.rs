//! Excel `HSTACK(array1, [array2], ...)`.
//!
//! Appends arguments left-to-right. Result height is the max row count of the
//! kept arguments; result width is the sum of their column counts.
//!
//! Documented Excel quirks this module implements:
//!
//! - A shorter argument is **padded with `#N/A`** in the extra rows — not
//!   blank, not `0`. Those pad cells are real `#N/A` values (`ISNA` true,
//!   `COUNTA` counts them, `COUNTBLANK` does not, `SUM` surfaces `#N/A`).
//! - In-bounds blank cells stay [`ExcelValue::Empty`]. They are **not**
//!   coerced to `0`. (Microsoft’s published example table sometimes shows a
//!   `0` for a source blank; this engine does not invent that `0`.)
//! - A 0-row or 0-column array is ignored. If every argument is ignored, the
//!   result is `#CALC!` — Excel cannot return a 0×0 array (same rule as
//!   `FILTER` with no matches).
//! - `HSTACK()` (no arguments) is `#VALUE!`.
//! - Scalars, including scalar errors, are 1×1 arrays and are stacked. A
//!   leading `#DIV/0!` does **not** abort the whole call the way `UNIQUE`
//!   surfaces a scalar error.
//! - Jagged arrays (rows of unequal length) are `#VALUE!`.
//! - The result is always an [`ExcelValue::Array`] (including 1×1).
//!
//! ## Spill / model limits
//!
//! - The engine returns an array **value**. The snippet workbook has no spill
//!   grid, so a blocked cell to the right of the host never yields `#SPILL!`.
//! - Scalar operators still scalarize. `HSTACK(...)+1` is top-left `+ 1`,
//!   not a host-aware intersection of a written spill.
//! - `IFNA` / `IFERROR` in this engine are **not** element-wise, so
//!   `IFNA(HSTACK(...),"")` does **not** blank pad `#N/A` cells. Pick a pad
//!   cell with `INDEX`, or wrap after a live Excel spill.
//! - Omitted arguments (`HSTACK(a,,b)`) do not parse. Excel would treat the
//!   hole as a missing optional; this parser requires an expression.
//! - Excel’s 254-argument cap and ~1,048,576-row / 16,384-column array caps
//!   are not enforced; size is memory-bounded.
//!
//! [`hstack`] pre-sizes the result and copies once. [`hstack_naive`] grows
//! by extending every row per argument (realloc). Same answers; the naive
//! path is the bench “before”.

use super::{Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

const NA: ExcelValue = ExcelValue::Error(ExcelError::Na);

/// Excel `HSTACK` from already-evaluated arguments.
pub fn hstack(args: &[ExcelValue]) -> ExcelValue {
    hstack_apply(args, StackStrategy::Fast)
}

/// Allocation-heavy baseline: clone the first grid, then `extend` every row
/// for each later argument (and push `#N/A` pad rows when height grows).
///
/// Same answers as [`hstack`]. Used as the bench "before".
pub fn hstack_naive(args: &[ExcelValue]) -> ExcelValue {
    hstack_apply(args, StackStrategy::Naive)
}

#[derive(Clone, Copy)]
enum StackStrategy {
    Fast,
    Naive,
}

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
        values.push(ev.eval_expr(arg, ctx)?);
    }
    Ok(hstack(&values))
}

enum View<'a> {
    Scalar(&'a ExcelValue),
    Grid(&'a [Vec<ExcelValue>]),
}

impl<'a> View<'a> {
    fn from_value(v: &'a ExcelValue) -> Result<Option<Self>, ExcelError> {
        match v {
            ExcelValue::Array(rows) => {
                if rows.is_empty() {
                    return Ok(None);
                }
                let width = rows[0].len();
                if width == 0 {
                    return Ok(None);
                }
                if rows.iter().any(|row| row.len() != width) {
                    return Err(ExcelError::Value);
                }
                Ok(Some(Self::Grid(rows)))
            }
            other => Ok(Some(Self::Scalar(other))),
        }
    }

    fn height(&self) -> usize {
        match self {
            Self::Scalar(_) => 1,
            Self::Grid(rows) => rows.len(),
        }
    }

    fn width(&self) -> usize {
        match self {
            Self::Scalar(_) => 1,
            Self::Grid(rows) => rows[0].len(),
        }
    }

    fn get(&self, r: usize, c: usize) -> &ExcelValue {
        match self {
            Self::Scalar(v) if r == 0 && c == 0 => v,
            Self::Scalar(_) => &ExcelValue::Empty,
            Self::Grid(rows) => &rows[r][c],
        }
    }

    fn clone_grid(&self) -> Vec<Vec<ExcelValue>> {
        let h = self.height();
        let w = self.width();
        let mut out = Vec::with_capacity(h);
        for r in 0..h {
            let mut row = Vec::with_capacity(w);
            for c in 0..w {
                row.push(self.get(r, c).clone());
            }
            out.push(row);
        }
        out
    }
}

fn hstack_apply(args: &[ExcelValue], strategy: StackStrategy) -> ExcelValue {
    if args.is_empty() {
        return ExcelValue::Error(ExcelError::Value);
    }

    let mut views = Vec::with_capacity(args.len());
    for arg in args {
        match View::from_value(arg) {
            Ok(Some(v)) => views.push(v),
            Ok(None) => {}
            Err(e) => return ExcelValue::Error(e),
        }
    }
    if views.is_empty() {
        return ExcelValue::Error(ExcelError::Calc);
    }

    match strategy {
        StackStrategy::Fast => stack_fast(&views),
        StackStrategy::Naive => stack_naive(&views),
    }
}

fn stack_fast(views: &[View<'_>]) -> ExcelValue {
    let height = views.iter().map(View::height).max().unwrap_or(0);
    let width: usize = views.iter().map(View::width).sum();
    let mut out = Vec::with_capacity(height);
    for r in 0..height {
        let mut row = Vec::with_capacity(width);
        for view in views {
            let h = view.height();
            let w = view.width();
            if r < h {
                for c in 0..w {
                    row.push(view.get(r, c).clone());
                }
            } else {
                row.extend(std::iter::repeat_n(NA, w));
            }
        }
        out.push(row);
    }
    ExcelValue::Array(out)
}

fn stack_naive(views: &[View<'_>]) -> ExcelValue {
    let mut out = views[0].clone_grid();
    let mut width = views[0].width();
    for view in &views[1..] {
        let add_h = view.height();
        let add_w = view.width();
        while out.len() < add_h {
            out.push(vec![NA; width]);
        }
        for (r, row) in out.iter_mut().enumerate() {
            if r < add_h {
                for c in 0..add_w {
                    row.push(view.get(r, c).clone());
                }
            } else {
                row.extend(std::iter::repeat_n(NA, add_w));
            }
        }
        width += add_w;
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

    fn col(vals: &[f64]) -> ExcelValue {
        ExcelValue::Array(vals.iter().copied().map(|x| vec![n(x)]).collect())
    }

    fn row(vals: &[f64]) -> ExcelValue {
        ExcelValue::Array(vec![vals.iter().copied().map(n).collect()])
    }

    fn both_eq(args: &[ExcelValue]) {
        assert_eq!(hstack(args), hstack_naive(args), "{args:?}");
    }

    #[test]
    fn two_equal_columns() {
        let a = col(&[1.0, 2.0, 3.0]);
        let b = col(&[4.0, 5.0, 6.0]);
        let got = hstack(&[a.clone(), b.clone()]);
        assert_eq!(
            got,
            ExcelValue::Array(vec![
                vec![n(1.0), n(4.0)],
                vec![n(2.0), n(5.0)],
                vec![n(3.0), n(6.0)],
            ])
        );
        both_eq(&[a, b]);
    }

    #[test]
    fn pad_shorter_right() {
        let a = col(&[1.0, 2.0, 3.0]);
        let b = col(&[4.0]);
        let got = hstack(&[a.clone(), b.clone()]);
        assert_eq!(
            got,
            ExcelValue::Array(vec![
                vec![n(1.0), n(4.0)],
                vec![n(2.0), NA],
                vec![n(3.0), NA],
            ])
        );
        both_eq(&[a, b]);
    }

    #[test]
    fn pad_shorter_left() {
        let a = col(&[1.0]);
        let b = col(&[2.0, 3.0, 4.0]);
        let got = hstack(&[a.clone(), b.clone()]);
        assert_eq!(
            got,
            ExcelValue::Array(vec![
                vec![n(1.0), n(2.0)],
                vec![NA, n(3.0)],
                vec![NA, n(4.0)],
            ])
        );
        both_eq(&[a, b]);
    }

    #[test]
    fn pad_wide_short_against_tall() {
        let a = ExcelValue::Array(vec![vec![n(1.0), n(2.0)]]);
        let b = col(&[3.0, 4.0, 5.0]);
        let got = hstack(&[a.clone(), b.clone()]);
        assert_eq!(
            got,
            ExcelValue::Array(vec![
                vec![n(1.0), n(2.0), n(3.0)],
                vec![NA, NA, n(4.0)],
                vec![NA, NA, n(5.0)],
            ])
        );
        both_eq(&[a, b]);
    }

    #[test]
    fn scalars_are_1x1() {
        let got = hstack(&[n(1.0), n(2.0), n(3.0)]);
        assert_eq!(got, ExcelValue::Array(vec![vec![n(1.0), n(2.0), n(3.0)]]));
        both_eq(&[n(1.0), n(2.0), n(3.0)]);
    }

    #[test]
    fn single_scalar_is_1x1_array() {
        assert_eq!(hstack(&[n(5.0)]), ExcelValue::Array(vec![vec![n(5.0)]]));
        both_eq(&[n(5.0)]);
    }

    #[test]
    fn scalar_error_is_stacked_not_aborted() {
        let err = ExcelValue::Error(ExcelError::Div0);
        let got = hstack(&[err.clone(), n(1.0)]);
        assert_eq!(got, ExcelValue::Array(vec![vec![err, n(1.0)]]));
    }

    #[test]
    fn error_inside_array_is_kept() {
        let a = ExcelValue::Array(vec![vec![n(1.0), ExcelValue::Error(ExcelError::Div0)]]);
        let b = row(&[2.0]);
        let got = hstack(&[a.clone(), b.clone()]);
        assert_eq!(
            got,
            ExcelValue::Array(vec![vec![
                n(1.0),
                ExcelValue::Error(ExcelError::Div0),
                n(2.0)
            ]])
        );
        both_eq(&[a, b]);
    }

    #[test]
    fn in_bounds_blank_stays_empty() {
        let a = ExcelValue::Array(vec![
            vec![n(1.0)],
            vec![ExcelValue::Empty],
            vec![n(3.0)],
        ]);
        let b = col(&[4.0, 5.0, 6.0]);
        let got = hstack(&[a.clone(), b.clone()]);
        assert_eq!(
            got,
            ExcelValue::Array(vec![
                vec![n(1.0), n(4.0)],
                vec![ExcelValue::Empty, n(5.0)],
                vec![n(3.0), n(6.0)],
            ])
        );
        both_eq(&[a, b]);
    }

    #[test]
    fn empty_array_is_ignored() {
        let empty = ExcelValue::Array(vec![]);
        let a = col(&[1.0, 2.0]);
        assert_eq!(hstack(&[empty.clone(), a.clone()]), a);
        assert_eq!(
            hstack(&[empty.clone()]),
            ExcelValue::Error(ExcelError::Calc)
        );
        both_eq(&[empty, a]);
    }

    #[test]
    fn zero_width_array_is_ignored() {
        let empty = ExcelValue::Array(vec![vec![]]);
        let a = row(&[1.0, 2.0]);
        assert_eq!(hstack(&[a.clone(), empty.clone()]), a);
        both_eq(&[a, empty]);
    }

    #[test]
    fn no_args_is_value() {
        assert_eq!(hstack(&[]), ExcelValue::Error(ExcelError::Value));
        assert_eq!(hstack_naive(&[]), ExcelValue::Error(ExcelError::Value));
    }

    #[test]
    fn jagged_is_value() {
        let jagged = ExcelValue::Array(vec![vec![n(1.0), n(2.0)], vec![n(3.0)]]);
        assert_eq!(
            hstack(&[jagged.clone(), n(0.0)]),
            ExcelValue::Error(ExcelError::Value)
        );
        both_eq(&[jagged, n(0.0)]);
    }

    #[test]
    fn types_preserved() {
        let a = ExcelValue::Array(vec![vec![n(1.0)], vec![t("x")], vec![ExcelValue::Bool(true)]]);
        let b = ExcelValue::Array(vec![
            vec![ExcelValue::Empty],
            vec![ExcelValue::Error(ExcelError::Na)],
            vec![n(0.0)],
        ]);
        let got = hstack(&[a.clone(), b.clone()]);
        assert_eq!(
            got,
            ExcelValue::Array(vec![
                vec![n(1.0), ExcelValue::Empty],
                vec![t("x"), NA],
                vec![ExcelValue::Bool(true), n(0.0)],
            ])
        );
        both_eq(&[a, b]);
    }

    #[test]
    fn three_args_and_matrix() {
        let a = ExcelValue::Array(vec![vec![n(1.0), n(2.0)], vec![n(3.0), n(4.0)]]);
        let b = col(&[5.0, 6.0]);
        let c = n(7.0);
        let got = hstack(&[a.clone(), b.clone(), c.clone()]);
        assert_eq!(
            got,
            ExcelValue::Array(vec![
                vec![n(1.0), n(2.0), n(5.0), n(7.0)],
                vec![n(3.0), n(4.0), n(6.0), NA],
            ])
        );
        both_eq(&[a, b, c]);
    }

    #[test]
    fn large_uneven_matches_naive() {
        let tall: Vec<Vec<ExcelValue>> = (0..2_048).map(|i| vec![n(i as f64)]).collect();
        let short: Vec<Vec<ExcelValue>> = (0..128).map(|i| vec![n((i + 10_000) as f64)]).collect();
        let args = [ExcelValue::Array(tall), ExcelValue::Array(short)];
        let a = hstack(&args);
        let b = hstack_naive(&args);
        assert_eq!(a, b);
        match a {
            ExcelValue::Array(rows) => {
                assert_eq!(rows.len(), 2_048);
                assert_eq!(rows[0], vec![n(0.0), n(10_000.0)]);
                assert_eq!(rows[128], vec![n(128.0), NA]);
                assert_eq!(rows[2_047][0], n(2_047.0));
            }
            other => panic!("{other:?}"),
        }
    }
}
