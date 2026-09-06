//! Excel `TAKE(array, rows, [cols])`.
//!
//! Positive `rows` / `cols` take from the start (top / left). Negative counts
//! take from the end (bottom / right). Omitted `cols` keeps every column.
//! `TAKE(array)` with neither count is `#VALUE!` — at least one of `rows` or
//! `cols` is required.
//!
//! Documented Excel quirks this module implements:
//!
//! - `rows` or `cols` of `0` (after toward-zero truncate) is `#CALC!` — Excel
//!   cannot return an empty array.
//! - `|rows|` / `|cols|` larger than the axis returns the whole axis (no pad).
//! - Counts coerce like arithmetic: empty → 0 → `#CALC!`; `TRUE` → 1;
//!   `FALSE` → 0 → `#CALC!`; numeric text parses; other text → `#VALUE!`.
//! - `1.9` / `-1.9` truncate toward zero (`1` / `-1`). `|n| < 1` and `n != 0`
//!   therefore become `0` and `#CALC!`.
//! - Errors inside the array are values and travel with the slice. A scalar
//!   error `array` wins left-to-right over a later count error.
//!
//! ## Spill / model limits
//!
//! - The engine returns an [`ExcelValue::Array`]; it does **not** write a
//!   spill range into the workbook snippet. `#SPILL!` from a blocked cell is
//!   therefore never produced here.
//! - Scalar operators (`TAKE(...)+1`) still `scalarize` to the top-left
//!   element. They do not intersect a written spill with the host cell.
//! - The parser does not accept omitted middle arguments (`TAKE(a,,2)`).
//!   Use an oversize `rows` count to keep every row while slicing columns.
//! - Excel's `#NUM!` when an array exceeds ~1,048,576 rows is not enforced;
//!   allocation is memory-bounded.
//!
//! [`take`] clones **only** the selected window. [`take_naive`] materializes
//! the whole grid, then `retain`s (and transposes twice when column-slicing).
//! Same answers; the naive path is the bench "before".

use super::coerce;
use xlsx_types::{ExcelError, ExcelValue};

const EMPTY: ExcelValue = ExcelValue::Empty;

/// Excel `TAKE` from already-evaluated arguments.
///
/// `rows` / `cols` are `None` when the argument was omitted (keep the axis).
pub fn take(
    array: &ExcelValue,
    rows: Option<&ExcelValue>,
    cols: Option<&ExcelValue>,
) -> ExcelValue {
    take_apply(array, rows, cols, TakeStrategy::Fast)
}

/// Allocation-heavy baseline: clone everything, then retain / transpose.
///
/// Same answers as [`take`]. Used as the bench "before".
pub fn take_naive(
    array: &ExcelValue,
    rows: Option<&ExcelValue>,
    cols: Option<&ExcelValue>,
) -> ExcelValue {
    take_apply(array, rows, cols, TakeStrategy::Naive)
}

#[derive(Clone, Copy)]
enum TakeStrategy {
    Fast,
    Naive,
}

enum Matrix<'a> {
    Array(&'a [Vec<ExcelValue>]),
    Scalar(&'a ExcelValue),
}

impl<'a> Matrix<'a> {
    fn from_value(v: &'a ExcelValue) -> Result<Self, ExcelError> {
        match v {
            ExcelValue::Error(e) => Err(*e),
            ExcelValue::Array(rows) => {
                if rows.is_empty() {
                    return Ok(Self::Array(rows));
                }
                let cols = rows[0].len();
                if rows.iter().any(|r| r.len() != cols) {
                    return Err(ExcelError::Value);
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

fn take_apply(
    array: &ExcelValue,
    rows: Option<&ExcelValue>,
    cols: Option<&ExcelValue>,
    strategy: TakeStrategy,
) -> ExcelValue {
    if rows.is_none() && cols.is_none() {
        return ExcelValue::Error(ExcelError::Value);
    }
    let grid = match Matrix::from_value(array) {
        Ok(m) => m,
        Err(e) => return ExcelValue::Error(e),
    };
    let row_n = match parse_count(rows) {
        Ok(n) => n,
        Err(e) => return ExcelValue::Error(e),
    };
    let col_n = match parse_count(cols) {
        Ok(n) => n,
        Err(e) => return ExcelValue::Error(e),
    };

    let (height, width) = grid.dims();
    let (r0, r1) = match axis_span(row_n, height) {
        Ok(span) => span,
        Err(e) => return ExcelValue::Error(e),
    };
    let (c0, c1) = match axis_span(col_n, width) {
        Ok(span) => span,
        Err(e) => return ExcelValue::Error(e),
    };
    if r0 >= r1 || c0 >= c1 {
        return ExcelValue::Error(ExcelError::Calc);
    }

    match strategy {
        TakeStrategy::Fast => slice_fast(&grid, r0, r1, c0, c1),
        TakeStrategy::Naive => slice_naive(&grid, r0, r1, c0, c1),
    }
}

/// `None` = omitted (keep the whole axis). `Some(0)` = `#CALC!`.
fn parse_count(arg: Option<&ExcelValue>) -> Result<Option<i64>, ExcelError> {
    let Some(v) = arg else {
        return Ok(None);
    };
    if let ExcelValue::Error(e) = v {
        return Err(*e);
    }
    let n = coerce::to_number(v)?;
    if !n.is_finite() {
        return Err(ExcelError::Num);
    }
    Ok(Some(n.trunc() as i64))
}

fn axis_span(count: Option<i64>, len: usize) -> Result<(usize, usize), ExcelError> {
    match count {
        None => Ok((0, len)),
        Some(0) => Err(ExcelError::Calc),
        Some(n) if n > 0 => {
            let take_n = (n as u64).min(len as u64) as usize;
            Ok((0, take_n))
        }
        Some(n) => {
            let take_n = n.unsigned_abs().min(len as u64) as usize;
            Ok((len - take_n, len))
        }
    }
}

fn slice_fast(grid: &Matrix<'_>, r0: usize, r1: usize, c0: usize, c1: usize) -> ExcelValue {
    let mut out = Vec::with_capacity(r1 - r0);
    for r in r0..r1 {
        let mut row = Vec::with_capacity(c1 - c0);
        for c in c0..c1 {
            row.push(grid.get(r, c).clone());
        }
        out.push(row);
    }
    ExcelValue::Array(out)
}

/// Naive: materialize the full matrix, `retain` the row window, then
/// transpose / retain / transpose for a column window.
fn slice_naive(grid: &Matrix<'_>, r0: usize, r1: usize, c0: usize, c1: usize) -> ExcelValue {
    let (h, w) = grid.dims();
    let mut all = Vec::with_capacity(h);
    for r in 0..h {
        let mut row = Vec::with_capacity(w);
        for c in 0..w {
            row.push(grid.get(r, c).clone());
        }
        all.push(row);
    }
    let mut i = 0;
    all.retain(|_| {
        let keep = i >= r0 && i < r1;
        i += 1;
        keep
    });
    if c0 == 0 && c1 == w {
        return ExcelValue::Array(all);
    }
    let mut cols = transpose(&all);
    let mut j = 0;
    cols.retain(|_| {
        let keep = j >= c0 && j < c1;
        j += 1;
        keep
    });
    ExcelValue::Array(transpose(&cols))
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
    fn col(vals: &[f64]) -> ExcelValue {
        ExcelValue::Array(vals.iter().map(|x| vec![n(*x)]).collect())
    }
    fn row(vals: &[f64]) -> ExcelValue {
        ExcelValue::Array(vec![vals.iter().map(|x| n(*x)).collect()])
    }
    fn matrix(vals: &[&[f64]]) -> ExcelValue {
        ExcelValue::Array(
            vals.iter()
                .map(|r| r.iter().map(|x| n(*x)).collect())
                .collect(),
        )
    }

    fn both_eq(array: &ExcelValue, rows: Option<&ExcelValue>, cols: Option<&ExcelValue>) {
        assert_eq!(take(array, rows, cols), take_naive(array, rows, cols));
    }

    #[test]
    fn first_rows() {
        let array = col(&[1.0, 2.0, 3.0]);
        let rows = n(2.0);
        assert_eq!(
            take(&array, Some(&rows), None),
            ExcelValue::Array(vec![vec![n(1.0)], vec![n(2.0)]])
        );
        both_eq(&array, Some(&rows), None);
    }

    #[test]
    fn last_rows() {
        let array = col(&[1.0, 2.0, 3.0]);
        let rows = n(-2.0);
        assert_eq!(
            take(&array, Some(&rows), None),
            ExcelValue::Array(vec![vec![n(2.0)], vec![n(3.0)]])
        );
        both_eq(&array, Some(&rows), None);
    }

    #[test]
    fn first_cols() {
        let array = row(&[1.0, 2.0, 3.0]);
        let cols = n(2.0);
        // rows omitted → keep the one row
        assert_eq!(
            take(&array, None, Some(&cols)),
            ExcelValue::Array(vec![vec![n(1.0), n(2.0)]])
        );
        both_eq(&array, None, Some(&cols));
    }

    #[test]
    fn last_cols() {
        let array = row(&[1.0, 2.0, 3.0]);
        let cols = n(-2.0);
        assert_eq!(
            take(&array, None, Some(&cols)),
            ExcelValue::Array(vec![vec![n(2.0), n(3.0)]])
        );
        both_eq(&array, None, Some(&cols));
    }

    #[test]
    fn both_dims_and_negatives() {
        let array = matrix(&[&[1.0, 2.0, 3.0], &[4.0, 5.0, 6.0], &[7.0, 8.0, 9.0]]);
        assert_eq!(
            take(&array, Some(&n(2.0)), Some(&n(2.0))),
            matrix(&[&[1.0, 2.0], &[4.0, 5.0]])
        );
        assert_eq!(
            take(&array, Some(&n(-2.0)), Some(&n(-2.0))),
            matrix(&[&[5.0, 6.0], &[8.0, 9.0]])
        );
        assert_eq!(
            take(&array, Some(&n(1.0)), Some(&n(-1.0))),
            matrix(&[&[3.0]])
        );
        both_eq(&array, Some(&n(-2.0)), Some(&n(2.0)));
    }

    #[test]
    fn oversize_returns_all() {
        let array = col(&[1.0, 2.0]);
        assert_eq!(take(&array, Some(&n(10.0)), None), col(&[1.0, 2.0]));
        assert_eq!(take(&array, Some(&n(-10.0)), None), col(&[1.0, 2.0]));
        both_eq(&array, Some(&n(10.0)), None);
        both_eq(&array, Some(&n(-10.0)), None);
    }

    #[test]
    fn zero_is_calc() {
        let array = col(&[1.0, 2.0]);
        assert_eq!(
            take(&array, Some(&n(0.0)), None),
            ExcelValue::Error(ExcelError::Calc)
        );
        assert_eq!(
            take(&array, Some(&n(2.0)), Some(&n(0.0))),
            ExcelValue::Error(ExcelError::Calc)
        );
        both_eq(&array, Some(&n(0.0)), None);
    }

    #[test]
    fn neither_count_is_value() {
        let array = col(&[1.0]);
        assert_eq!(
            take(&array, None, None),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn scalar_error_array_wins_ltr() {
        let array = ExcelValue::Error(ExcelError::Na);
        let rows = ExcelValue::Error(ExcelError::Div0);
        assert_eq!(
            take(&array, Some(&rows), None),
            ExcelValue::Error(ExcelError::Na)
        );
    }

    #[test]
    fn count_error_surfaces() {
        let array = col(&[1.0, 2.0]);
        let boom = ExcelValue::Error(ExcelError::Div0);
        assert_eq!(
            take(&array, Some(&boom), None),
            ExcelValue::Error(ExcelError::Div0)
        );
    }

    #[test]
    fn error_in_array_is_kept() {
        let array = ExcelValue::Array(vec![
            vec![ExcelValue::Error(ExcelError::Div0)],
            vec![n(2.0)],
        ]);
        assert_eq!(
            take(&array, Some(&n(1.0)), None),
            ExcelValue::Array(vec![vec![ExcelValue::Error(ExcelError::Div0)]])
        );
    }

    #[test]
    fn trunc_toward_zero() {
        let array = col(&[1.0, 2.0, 3.0]);
        assert_eq!(
            take(&array, Some(&n(1.9)), None),
            ExcelValue::Array(vec![vec![n(1.0)]])
        );
        assert_eq!(
            take(&array, Some(&n(-1.9)), None),
            ExcelValue::Array(vec![vec![n(3.0)]])
        );
        assert_eq!(
            take(&array, Some(&n(0.9)), None),
            ExcelValue::Error(ExcelError::Calc)
        );
        assert_eq!(
            take(&array, Some(&n(-0.9)), None),
            ExcelValue::Error(ExcelError::Calc)
        );
    }

    #[test]
    fn empty_and_false_counts_are_calc() {
        let array = col(&[1.0]);
        assert_eq!(
            take(&array, Some(&ExcelValue::Empty), None),
            ExcelValue::Error(ExcelError::Calc)
        );
        assert_eq!(
            take(&array, Some(&ExcelValue::Bool(false)), None),
            ExcelValue::Error(ExcelError::Calc)
        );
        assert_eq!(
            take(&array, Some(&ExcelValue::Bool(true)), None),
            ExcelValue::Array(vec![vec![n(1.0)]])
        );
    }

    #[test]
    fn text_count_is_value() {
        let array = col(&[1.0]);
        assert_eq!(
            take(&array, Some(&ExcelValue::Text("x".into())), None),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            take(&array, Some(&ExcelValue::Text("2".into())), None),
            ExcelValue::Array(vec![vec![n(1.0)]])
        );
    }

    #[test]
    fn scalar_is_one_by_one() {
        let array = n(5.0);
        assert_eq!(
            take(&array, Some(&n(1.0)), None),
            ExcelValue::Array(vec![vec![n(5.0)]])
        );
        both_eq(&array, Some(&n(1.0)), None);
    }

    #[test]
    fn jagged_is_value() {
        let array = ExcelValue::Array(vec![vec![n(1.0), n(2.0)], vec![n(3.0)]]);
        assert_eq!(
            take(&array, Some(&n(1.0)), None),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn blanks_are_kept() {
        let array = ExcelValue::Array(vec![vec![n(1.0)], vec![ExcelValue::Empty], vec![n(3.0)]]);
        assert_eq!(
            take(&array, Some(&n(2.0)), None),
            ExcelValue::Array(vec![vec![n(1.0)], vec![ExcelValue::Empty]])
        );
    }

    #[test]
    fn large_window_matches_naive() {
        let n_rows = 2_048usize;
        let mut rows = Vec::with_capacity(n_rows);
        for i in 0..n_rows {
            rows.push(vec![n(i as f64), n((i * 3) as f64), n((i * 5) as f64)]);
        }
        let array = ExcelValue::Array(rows);
        for (r, c) in [
            (Some(n(16.0)), None),
            (Some(n(-16.0)), None),
            (Some(n(32.0)), Some(n(2.0))),
            (Some(n(-8.0)), Some(n(-1.0))),
            (None, Some(n(-2.0))),
        ] {
            both_eq(&array, r.as_ref(), c.as_ref());
        }
    }
}
