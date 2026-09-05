//! Excel `CHOOSEROWS(array, row_num1, [row_num2], ...)`.
//!
//! Returns the listed rows, in the listed order, as an [`ExcelValue::Array`]
//! (including a 1×1). This engine does **not** write a spill range, so a
//! blocked neighbor never produces `#SPILL!` — evaluate returns the array
//! that *would* spill.
//!
//! Documented Excel quirks this module implements:
//!
//! - Positive `row_num` is 1-based from the **top**.
//! - Negative `row_num` counts from the **bottom**: `-1` is the last row,
//!   `-2` the second-to-last. This is **not** `INDEX` (`INDEX(..., -1)` is
//!   `#REF!`).
//! - `0`, or `|row_num|` greater than the array height, is `#VALUE!` — not
//!   `INDEX`'s `#REF!`. Microsoft: *"a #VALUE error if the absolute value of
//!   any of the row_num arguments is zero or exceeds the number of rows"*.
//! - `FALSE` and a blank `row_num` coerce to `0` → `#VALUE!`. `TRUE` is row 1.
//! - Numeric text (`"2"`) coerces; other text is `#VALUE!`.
//! - Fractional `row_num` is truncated **toward zero** (same `TRUNC` as
//!   `CHOOSE` / `INDEX` in this engine): `1.9` → row 1, `-1.9` → `-1` (last
//!   row), `0.9` / `-0.5` → `0` → `#VALUE!`. Live Excel is not in CI; this
//!   is the modeled rule.
//! - Each `row_num` argument may be a scalar or an array (row-major flatten).
//!   Microsoft's `{1,3},5,1` form is supported. Duplicates are kept.
//! - An error in a `row_num` argument (or inside a `row_num` array) wins;
//!   no partial pick. An error **cell** in the source array is just a value
//!   and is returned if that row is selected.
//! - A scalar first argument is a 1×1 array.
//!
//! ## Spill / model limits
//!
//! - Result is an array **value**. Occupied cells below/right of the host
//!   never yield `#SPILL!`.
//! - Scalar operators (`CHOOSEROWS(...)+1`) take the top-left element
//!   (`scalarize`), not a host-aware intersection of a written spill.
//! - `SEQUENCE` is not implemented; reverse a column with explicit negatives
//!   (`-1,-2,-3`), not `CHOOSEROWS(a, SEQUENCE(...))`.
//! - Excel's ~1,048,576-row array cap is not enforced; size is memory-bounded.
//!
//! [`select`] resolves indices, then clones **only** the picked rows.
//! [`select_naive`] clones the whole grid first, then clones the picks from
//! that copy — same answers, more allocation. Used as the bench "before".

use super::coerce;
use xlsx_types::{ExcelError, ExcelValue};

const EMPTY: ExcelValue = ExcelValue::Empty;

/// Excel `CHOOSEROWS` from an already-evaluated array and `row_num` values.
pub fn select(array: &ExcelValue, row_nums: &[ExcelValue]) -> ExcelValue {
    apply(array, row_nums, Strategy::Fast)
}

/// Allocation-heavy baseline: clone the whole grid, then the picks.
///
/// Same answers as [`select`]. Used as the bench "before".
pub fn select_naive(array: &ExcelValue, row_nums: &[ExcelValue]) -> ExcelValue {
    apply(array, row_nums, Strategy::Naive)
}

#[derive(Clone, Copy)]
enum Strategy {
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

    fn row_count(&self) -> usize {
        match self {
            Self::Array(rows) => rows.len(),
            Self::Scalar(_) => 1,
        }
    }

    fn col_count(&self) -> usize {
        match self {
            Self::Array(rows) => rows.first().map(|r| r.len()).unwrap_or(0),
            Self::Scalar(_) => 1,
        }
    }

    fn row(&self, i: usize) -> Vec<ExcelValue> {
        match self {
            Self::Array(rows) => rows.get(i).cloned().unwrap_or_default(),
            Self::Scalar(v) if i == 0 => vec![(*v).clone()],
            Self::Scalar(_) => vec![EMPTY.clone()],
        }
    }

    fn materialize(&self) -> Vec<Vec<ExcelValue>> {
        (0..self.row_count()).map(|i| self.row(i)).collect()
    }
}

fn apply(array: &ExcelValue, row_nums: &[ExcelValue], strategy: Strategy) -> ExcelValue {
    let grid = match Matrix::from_value(array) {
        Ok(g) => g,
        Err(e) => return ExcelValue::Error(e),
    };
    let nrows = grid.row_count();
    if grid.col_count() == 0 && nrows > 0 {
        return ExcelValue::Error(ExcelError::Value);
    }
    let idxs = match collect_indices(row_nums, nrows) {
        Ok(i) => i,
        Err(e) => return ExcelValue::Error(e),
    };
    match strategy {
        Strategy::Fast => take_fast(&grid, &idxs),
        Strategy::Naive => take_naive(&grid, &idxs),
    }
}

fn collect_indices(row_nums: &[ExcelValue], nrows: usize) -> Result<Vec<usize>, ExcelError> {
    let mut out = Vec::new();
    for v in row_nums {
        collect_indices_value(v, nrows, &mut out)?;
    }
    if out.is_empty() {
        return Err(ExcelError::Value);
    }
    Ok(out)
}

fn collect_indices_value(
    v: &ExcelValue,
    nrows: usize,
    out: &mut Vec<usize>,
) -> Result<(), ExcelError> {
    match v {
        ExcelValue::Error(e) => Err(*e),
        ExcelValue::Array(rows) => {
            for row in rows {
                for cell in row {
                    collect_indices_value(cell, nrows, out)?;
                }
            }
            Ok(())
        }
        other => {
            let n = coerce::to_number(other)?;
            out.push(resolve_row(n, nrows)?);
            Ok(())
        }
    }
}

/// 1-based / negative-from-end → 0-based. `0` and out-of-range are `#VALUE!`.
fn resolve_row(n: f64, nrows: usize) -> Result<usize, ExcelError> {
    if !n.is_finite() {
        return Err(ExcelError::Value);
    }
    let i = n.trunc();
    if i == 0.0 {
        return Err(ExcelError::Value);
    }
    let height = nrows as f64;
    if i > 0.0 {
        if i > height {
            return Err(ExcelError::Value);
        }
        Ok((i as usize) - 1)
    } else {
        let abs = -i;
        if abs > height {
            return Err(ExcelError::Value);
        }
        Ok(nrows - (abs as usize))
    }
}

fn take_fast(grid: &Matrix<'_>, idxs: &[usize]) -> ExcelValue {
    let mut out = Vec::with_capacity(idxs.len());
    for &i in idxs {
        out.push(grid.row(i));
    }
    ExcelValue::Array(out)
}

fn take_naive(grid: &Matrix<'_>, idxs: &[usize]) -> ExcelValue {
    let owned = grid.materialize();
    let mut out = Vec::with_capacity(idxs.len());
    for &i in idxs {
        out.push(owned.get(i).cloned().unwrap_or_default());
    }
    ExcelValue::Array(out)
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
    fn nums(vals: &[f64]) -> Vec<ExcelValue> {
        vals.iter().copied().map(n).collect()
    }

    fn both_eq(array: &ExcelValue, row_nums: &[ExcelValue]) {
        assert_eq!(select(array, row_nums), select_naive(array, row_nums));
    }

    #[test]
    fn picks_listed_rows_in_order() {
        let array = col(&[1.0, 2.0, 3.0]);
        let got = select(&array, &nums(&[1.0, 3.0]));
        assert_eq!(got, ExcelValue::Array(vec![vec![n(1.0)], vec![n(3.0)]]));
        both_eq(&array, &nums(&[1.0, 3.0]));
    }

    #[test]
    fn negative_counts_from_end() {
        let array = col(&[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(
            select(&array, &nums(&[-1.0, -2.0])),
            ExcelValue::Array(vec![vec![n(4.0)], vec![n(3.0)]])
        );
        both_eq(&array, &nums(&[-1.0, -2.0]));
    }

    #[test]
    fn zero_is_value() {
        let array = col(&[1.0, 2.0]);
        assert_eq!(
            select(&array, &nums(&[0.0])),
            ExcelValue::Error(ExcelError::Value)
        );
        both_eq(&array, &nums(&[0.0]));
    }

    #[test]
    fn oob_positive_is_value_not_ref() {
        let array = col(&[1.0, 2.0]);
        assert_eq!(
            select(&array, &nums(&[3.0])),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn oob_negative_is_value() {
        let array = col(&[1.0, 2.0]);
        assert_eq!(
            select(&array, &nums(&[-3.0])),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn false_coerces_to_zero_value() {
        let array = col(&[1.0, 2.0]);
        assert_eq!(
            select(&array, &[ExcelValue::Bool(false)]),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn true_is_first_row() {
        let array = col(&[1.0, 2.0]);
        assert_eq!(
            select(&array, &[ExcelValue::Bool(true)]),
            ExcelValue::Array(vec![vec![n(1.0)]])
        );
    }

    #[test]
    fn blank_row_num_is_value() {
        let array = col(&[1.0, 2.0]);
        assert_eq!(
            select(&array, &[ExcelValue::Empty]),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn trunc_toward_zero() {
        let array = col(&[1.0, 2.0, 3.0]);
        assert_eq!(
            select(&array, &nums(&[1.9])),
            ExcelValue::Array(vec![vec![n(1.0)]])
        );
        assert_eq!(
            select(&array, &nums(&[-1.9])),
            ExcelValue::Array(vec![vec![n(3.0)]])
        );
        assert_eq!(
            select(&array, &nums(&[0.9])),
            ExcelValue::Error(ExcelError::Value)
        );
        both_eq(&array, &nums(&[1.9]));
        both_eq(&array, &nums(&[-1.9]));
    }

    #[test]
    fn array_row_nums_flatten_row_major() {
        let array = col(&[10.0, 20.0, 30.0, 40.0]);
        let nums = ExcelValue::Array(vec![vec![n(1.0), n(3.0)]]);
        assert_eq!(
            select(&array, &[nums.clone()]),
            ExcelValue::Array(vec![vec![n(10.0)], vec![n(30.0)]])
        );
        both_eq(&array, &[nums]);
    }

    #[test]
    fn duplicates_kept() {
        let array = col(&[1.0, 2.0]);
        assert_eq!(
            select(&array, &nums(&[1.0, 1.0])),
            ExcelValue::Array(vec![vec![n(1.0)], vec![n(1.0)]])
        );
    }

    #[test]
    fn error_in_row_num_wins() {
        let array = col(&[1.0, 2.0]);
        assert_eq!(
            select(&array, &[ExcelValue::Error(ExcelError::Div0)]),
            ExcelValue::Error(ExcelError::Div0)
        );
    }

    #[test]
    fn error_cell_in_picked_row_is_kept() {
        let array = ExcelValue::Array(vec![
            vec![ExcelValue::Error(ExcelError::Div0)],
            vec![n(2.0)],
        ]);
        assert_eq!(
            select(&array, &nums(&[1.0])),
            ExcelValue::Array(vec![vec![ExcelValue::Error(ExcelError::Div0)]])
        );
    }

    #[test]
    fn array_error_wins_before_row_nums() {
        assert_eq!(
            select(&ExcelValue::Error(ExcelError::Na), &nums(&[1.0])),
            ExcelValue::Error(ExcelError::Na)
        );
    }

    #[test]
    fn scalar_is_one_by_one() {
        assert_eq!(
            select(&n(5.0), &nums(&[1.0])),
            ExcelValue::Array(vec![vec![n(5.0)]])
        );
        assert_eq!(
            select(&n(5.0), &nums(&[-1.0])),
            ExcelValue::Array(vec![vec![n(5.0)]])
        );
        assert_eq!(
            select(&n(5.0), &nums(&[2.0])),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn matrix_keeps_columns() {
        let array = ExcelValue::Array(vec![
            vec![n(1.0), n(2.0)],
            vec![n(3.0), n(4.0)],
            vec![n(5.0), n(6.0)],
        ]);
        assert_eq!(
            select(&array, &nums(&[1.0, 3.0])),
            ExcelValue::Array(vec![vec![n(1.0), n(2.0)], vec![n(5.0), n(6.0)]])
        );
        both_eq(&array, &nums(&[1.0, 3.0]));
    }

    #[test]
    fn mix_pos_neg() {
        let array = col(&[1.0, 2.0, 3.0]);
        assert_eq!(
            select(&array, &nums(&[1.0, -1.0])),
            ExcelValue::Array(vec![vec![n(1.0)], vec![n(3.0)]])
        );
    }

    #[test]
    fn empty_row_num_list_is_value() {
        let array = col(&[1.0]);
        assert_eq!(
            select(&array, &[]),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn large_pick_matches_naive() {
        let n_rows = 2_048usize;
        let rows: Vec<Vec<ExcelValue>> = (0..n_rows)
            .map(|i| vec![n(i as f64), n((i * 3) as f64)])
            .collect();
        let array = ExcelValue::Array(rows);
        let picks: Vec<ExcelValue> = (0..n_rows)
            .filter(|i| i % 7 == 0)
            .map(|i| n((i + 1) as f64))
            .collect();
        let a = select(&array, &picks);
        let b = select_naive(&array, &picks);
        assert_eq!(a, b);
        match a {
            ExcelValue::Array(out) => {
                assert_eq!(out.len(), n_rows.div_ceil(7));
                assert_eq!(out[0][0], n(0.0));
            }
            other => panic!("{other:?}"),
        }
    }
}
