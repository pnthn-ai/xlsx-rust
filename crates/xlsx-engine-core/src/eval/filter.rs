//! Excel `FILTER(array, include, [if_empty])` kernel.
//!
//! Documented Excel quirks this module implements:
//!
//! - No matches and `if_empty` omitted → `#CALC!` (Excel cannot return an
//!   empty array).
//! - No matches and `if_empty` supplied → that value, as-is (scalar or array).
//! - `include` dimension must be a vector matching the filtered axis, or a
//!   1×1 broadcast; otherwise `#VALUE!`.
//! - Any error inside `include` wins (row-major, left-to-right).
//! - Non-logical `include` cells (`"x"`, `"TRUE"` text) are `#VALUE!`.
//! - Numbers: nonzero is TRUE, `0` is FALSE. Blank is FALSE.
//!
//! `if_empty` is evaluated by the caller (Excel evaluates function arguments)
//! but is **used** only when the filtered set is empty. An unused `1/0` in
//! `if_empty` does not replace a non-empty result.
//!
//! ## Spill / model limits
//!
//! - The engine returns an [`ExcelValue::Array`]; it does **not** write a
//!   spill range into the workbook snippet. `#SPILL!` from a blocked cell is
//!   therefore never produced here.
//! - Comparison / arithmetic operators still scalarize. `FILTER(A1:A3, A1:A3>1)`
//!   is **not** a boolean-array include — supply a logical/numeric vector
//!   (literal or range of logicals). `*` / `+` broadcasting of criteria is
//!   not modeled.
//! - Excel's worksheet array-size cap (~1,048,576 rows) is not enforced;
//!   allocation is bounded only by memory.
//!
//! [`select`] scans `include` into a mask, then clones **only** matching rows
//! or columns. [`select_naive`] clones the whole array (and transposes for a
//! column filter) before retaining — same answers, more allocation. Used as
//! the bench "before".

use super::coerce;
use xlsx_types::{ExcelError, ExcelValue};

const EMPTY: ExcelValue = ExcelValue::Empty;

/// Excel `FILTER` from already-evaluated arguments.
pub fn select(
    array: &ExcelValue,
    include: &ExcelValue,
    if_empty: Option<&ExcelValue>,
) -> ExcelValue {
    filter_apply(array, include, if_empty, FilterStrategy::Fast)
}

/// Allocation-heavy baseline: clone everything, transpose for column filters.
///
/// Same answers as [`select`]. Used as the bench "before".
pub fn select_naive(
    array: &ExcelValue,
    include: &ExcelValue,
    if_empty: Option<&ExcelValue>,
) -> ExcelValue {
    filter_apply(array, include, if_empty, FilterStrategy::Naive)
}

#[derive(Clone, Copy)]
enum FilterStrategy {
    Fast,
    Naive,
}

#[derive(Clone, Copy)]
enum Axis {
    Broadcast,
    Rows,
    Cols,
}

enum Matrix<'a> {
    Array(&'a [Vec<ExcelValue>]),
    Scalar(&'a ExcelValue),
}

impl<'a> Matrix<'a> {
    fn from_value(v: &'a ExcelValue) -> Result<Self, ExcelError> {
        match v {
            ExcelValue::Error(e) => Err(*e),
            ExcelValue::Array(rows) => Ok(Self::Array(rows)),
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

    fn row_count(&self) -> usize {
        self.dims().0
    }
}

fn filter_apply(
    array: &ExcelValue,
    include: &ExcelValue,
    if_empty: Option<&ExcelValue>,
    strategy: FilterStrategy,
) -> ExcelValue {
    let rows = match Matrix::from_value(array) {
        Ok(m) => m,
        Err(e) => return ExcelValue::Error(e),
    };
    let inc = match Matrix::from_value(include) {
        Ok(m) => m,
        Err(e) => return ExcelValue::Error(e),
    };
    if let Some(e) = first_include_error(&inc) {
        return ExcelValue::Error(e);
    }

    let (ar, ac) = rows.dims();
    let (ir, ic) = inc.dims();
    let axis = match classify_axis(ar, ac, ir, ic) {
        Ok(a) => a,
        Err(e) => return ExcelValue::Error(e),
    };

    let mask = match build_mask(&inc, axis, ar, ac) {
        Ok(m) => m,
        Err(e) => return ExcelValue::Error(e),
    };

    let hits = match axis {
        Axis::Broadcast => {
            if mask.first().copied().unwrap_or(false) {
                ar.max(1)
            } else {
                0
            }
        }
        _ => mask.iter().filter(|b| **b).count(),
    };
    if hits == 0 {
        return empty_result(if_empty);
    }

    match strategy {
        FilterStrategy::Fast => take_matches(&rows, axis, &mask, hits),
        FilterStrategy::Naive => take_matches_naive(&rows, axis, &mask),
    }
}

fn empty_result(if_empty: Option<&ExcelValue>) -> ExcelValue {
    match if_empty {
        Some(v) => v.clone(),
        None => ExcelValue::Error(ExcelError::Calc),
    }
}

fn first_include_error(inc: &Matrix<'_>) -> Option<ExcelError> {
    let (r, c) = inc.dims();
    for i in 0..r {
        for j in 0..c {
            if let ExcelValue::Error(e) = inc.get(i, j) {
                return Some(*e);
            }
        }
    }
    None
}

fn classify_axis(
    array_r: usize,
    array_c: usize,
    inc_r: usize,
    inc_c: usize,
) -> Result<Axis, ExcelError> {
    if inc_r == 1 && inc_c == 1 {
        return Ok(Axis::Broadcast);
    }
    let inc_is_col = inc_c == 1;
    let inc_is_row = inc_r == 1;
    if inc_is_col && inc_r == array_r {
        return Ok(Axis::Rows);
    }
    if inc_is_row && inc_c == array_c {
        return Ok(Axis::Cols);
    }
    Err(ExcelError::Value)
}

fn build_mask(
    inc: &Matrix<'_>,
    axis: Axis,
    array_r: usize,
    array_c: usize,
) -> Result<Vec<bool>, ExcelError> {
    match axis {
        Axis::Broadcast => Ok(vec![coerce::to_logical(inc.get(0, 0))?]),
        Axis::Rows => {
            let mut mask = Vec::with_capacity(array_r);
            for i in 0..array_r {
                mask.push(coerce::to_logical(inc.get(i, 0))?);
            }
            Ok(mask)
        }
        Axis::Cols => {
            let mut mask = Vec::with_capacity(array_c);
            for j in 0..array_c {
                mask.push(coerce::to_logical(inc.get(0, j))?);
            }
            Ok(mask)
        }
    }
}

fn take_matches(rows: &Matrix<'_>, axis: Axis, mask: &[bool], hits: usize) -> ExcelValue {
    match axis {
        Axis::Broadcast => clone_all(rows),
        Axis::Rows => {
            let mut out = Vec::with_capacity(hits);
            let cols = rows.dims().1;
            for i in 0..rows.row_count() {
                if mask.get(i).copied().unwrap_or(false) {
                    let mut row = Vec::with_capacity(cols);
                    for j in 0..cols {
                        row.push(rows.get(i, j).clone());
                    }
                    out.push(row);
                }
            }
            ExcelValue::Array(out)
        }
        Axis::Cols => {
            let cols: Vec<usize> = mask
                .iter()
                .enumerate()
                .filter_map(|(i, k)| k.then_some(i))
                .collect();
            let mut out = Vec::with_capacity(rows.row_count());
            for i in 0..rows.row_count() {
                let mut new_row = Vec::with_capacity(cols.len());
                for &c in &cols {
                    new_row.push(rows.get(i, c).clone());
                }
                out.push(new_row);
            }
            ExcelValue::Array(out)
        }
    }
}

fn clone_all(rows: &Matrix<'_>) -> ExcelValue {
    let (r, c) = rows.dims();
    let mut out = Vec::with_capacity(r);
    for i in 0..r {
        let mut row = Vec::with_capacity(c);
        for j in 0..c {
            row.push(rows.get(i, j).clone());
        }
        out.push(row);
    }
    ExcelValue::Array(out)
}

/// Naive: materialize the full matrix, then `retain` (row) or transpose twice
/// (column). Same answers as [`take_matches`].
fn take_matches_naive(rows: &Matrix<'_>, axis: Axis, mask: &[bool]) -> ExcelValue {
    match axis {
        Axis::Broadcast => clone_all(rows),
        Axis::Rows => {
            let mut all = materialize(rows);
            let mut i = 0;
            all.retain(|_| {
                let keep = mask.get(i).copied().unwrap_or(false);
                i += 1;
                keep
            });
            ExcelValue::Array(all)
        }
        Axis::Cols => {
            let mut all = transpose(&materialize(rows));
            let mut i = 0;
            all.retain(|_| {
                let keep = mask.get(i).copied().unwrap_or(false);
                i += 1;
                keep
            });
            ExcelValue::Array(transpose(&all))
        }
    }
}

fn materialize(rows: &Matrix<'_>) -> Vec<Vec<ExcelValue>> {
    let (r, c) = rows.dims();
    let mut out = Vec::with_capacity(r);
    for i in 0..r {
        let mut row = Vec::with_capacity(c);
        for j in 0..c {
            row.push(rows.get(i, j).clone());
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
    fn col(vals: &[f64]) -> ExcelValue {
        ExcelValue::Array(vals.iter().map(|x| vec![n(*x)]).collect())
    }
    fn bool_col(vals: &[bool]) -> ExcelValue {
        ExcelValue::Array(vals.iter().map(|b| vec![ExcelValue::Bool(*b)]).collect())
    }
    fn bool_row(vals: &[bool]) -> ExcelValue {
        ExcelValue::Array(vec![vals.iter().map(|b| ExcelValue::Bool(*b)).collect()])
    }

    fn both_eq(array: &ExcelValue, include: &ExcelValue, if_empty: Option<&ExcelValue>) {
        assert_eq!(
            select(array, include, if_empty),
            select_naive(array, include, if_empty)
        );
    }

    #[test]
    fn column_filter_keeps_true_rows() {
        let array = col(&[1.0, 2.0, 3.0]);
        let include = bool_col(&[true, false, true]);
        let got = select(&array, &include, None);
        assert_eq!(got, ExcelValue::Array(vec![vec![n(1.0)], vec![n(3.0)]]));
        both_eq(&array, &include, None);
    }

    #[test]
    fn no_match_is_calc() {
        let array = col(&[1.0, 2.0]);
        let include = bool_col(&[false, false]);
        assert_eq!(
            select(&array, &include, None),
            ExcelValue::Error(ExcelError::Calc)
        );
        both_eq(&array, &include, None);
    }

    #[test]
    fn if_empty_replaces_calc() {
        let array = col(&[1.0]);
        let include = bool_col(&[false]);
        let empty = ExcelValue::Text("none".into());
        assert_eq!(select(&array, &include, Some(&empty)), empty);
        both_eq(&array, &include, Some(&empty));
    }

    #[test]
    fn unused_if_empty_error_is_ignored() {
        let array = col(&[1.0]);
        let include = bool_col(&[true]);
        let boom = ExcelValue::Error(ExcelError::Div0);
        assert_eq!(
            select(&array, &include, Some(&boom)),
            ExcelValue::Array(vec![vec![n(1.0)]])
        );
    }

    #[test]
    fn used_if_empty_error_surfaces() {
        let array = col(&[1.0]);
        let include = bool_col(&[false]);
        let boom = ExcelValue::Error(ExcelError::Div0);
        assert_eq!(
            select(&array, &include, Some(&boom)),
            ExcelValue::Error(ExcelError::Div0)
        );
    }

    #[test]
    fn dim_mismatch_is_value() {
        let array = col(&[1.0, 2.0, 3.0]);
        let include = bool_col(&[true, false]);
        assert_eq!(
            select(&array, &include, None),
            ExcelValue::Error(ExcelError::Value)
        );
        both_eq(&array, &include, None);
    }

    #[test]
    fn include_error_wins() {
        let array = col(&[1.0, 2.0]);
        let include = ExcelValue::Array(vec![
            vec![ExcelValue::Bool(true)],
            vec![ExcelValue::Error(ExcelError::Div0)],
        ]);
        assert_eq!(
            select(&array, &include, None),
            ExcelValue::Error(ExcelError::Div0)
        );
        both_eq(&array, &include, None);
    }

    #[test]
    fn row_filter_keeps_true_cols() {
        let array = ExcelValue::Array(vec![vec![n(1.0), n(2.0), n(3.0)]]);
        let include = bool_row(&[true, false, true]);
        let got = select(&array, &include, None);
        assert_eq!(got, ExcelValue::Array(vec![vec![n(1.0), n(3.0)]]));
        both_eq(&array, &include, None);
    }

    #[test]
    fn matrix_row_and_col() {
        let array = ExcelValue::Array(vec![
            vec![n(1.0), n(2.0)],
            vec![n(3.0), n(4.0)],
            vec![n(5.0), n(6.0)],
        ]);
        let rows = bool_col(&[true, false, true]);
        assert_eq!(
            select(&array, &rows, None),
            ExcelValue::Array(vec![vec![n(1.0), n(2.0)], vec![n(5.0), n(6.0)]])
        );
        let cols = bool_row(&[true, false]);
        assert_eq!(
            select(&array, &cols, None),
            ExcelValue::Array(vec![vec![n(1.0)], vec![n(3.0)], vec![n(5.0)]])
        );
        both_eq(&array, &rows, None);
        both_eq(&array, &cols, None);
    }

    #[test]
    fn broadcast_true_keeps_all() {
        let array = col(&[1.0, 2.0]);
        let include = ExcelValue::Bool(true);
        assert_eq!(
            select(&array, &include, None),
            ExcelValue::Array(vec![vec![n(1.0)], vec![n(2.0)]])
        );
        both_eq(&array, &include, None);
    }

    #[test]
    fn broadcast_false_is_calc() {
        let array = col(&[1.0, 2.0]);
        let include = ExcelValue::Bool(false);
        assert_eq!(
            select(&array, &include, None),
            ExcelValue::Error(ExcelError::Calc)
        );
    }

    #[test]
    fn number_include_coerces() {
        let array = col(&[1.0, 2.0]);
        let include = ExcelValue::Array(vec![vec![n(1.0)], vec![n(0.0)]]);
        assert_eq!(
            select(&array, &include, None),
            ExcelValue::Array(vec![vec![n(1.0)]])
        );
    }

    #[test]
    fn text_include_is_value() {
        let array = col(&[1.0]);
        let include = ExcelValue::Array(vec![vec![ExcelValue::Text("TRUE".into())]]);
        assert_eq!(
            select(&array, &include, None),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn include_2d_is_value() {
        let array = ExcelValue::Array(vec![vec![n(1.0), n(2.0)], vec![n(3.0), n(4.0)]]);
        let include = ExcelValue::Array(vec![
            vec![ExcelValue::Bool(true), ExcelValue::Bool(false)],
            vec![ExcelValue::Bool(false), ExcelValue::Bool(true)],
        ]);
        assert_eq!(
            select(&array, &include, None),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn matching_error_in_array_is_kept() {
        let array = ExcelValue::Array(vec![
            vec![ExcelValue::Error(ExcelError::Div0)],
            vec![n(2.0)],
        ]);
        let include = bool_col(&[true, false]);
        assert_eq!(
            select(&array, &include, None),
            ExcelValue::Array(vec![vec![ExcelValue::Error(ExcelError::Div0)]])
        );
    }

    #[test]
    fn large_row_filter_matches_naive() {
        let n_rows = 2_048usize;
        let mut rows = Vec::with_capacity(n_rows);
        let mut inc = Vec::with_capacity(n_rows);
        for i in 0..n_rows {
            rows.push(vec![n(i as f64), n((i * 3) as f64)]);
            inc.push(vec![ExcelValue::Bool(i % 3 == 0)]);
        }
        let array = ExcelValue::Array(rows);
        let include = ExcelValue::Array(inc);
        let a = select(&array, &include, None);
        let b = select_naive(&array, &include, None);
        assert_eq!(a, b);
        match a {
            ExcelValue::Array(out) => {
                assert_eq!(out.len(), n_rows.div_ceil(3));
                assert_eq!(out[0][0], n(0.0));
            }
            other => panic!("{other:?}"),
        }
    }
}
