//! Excel `SORT(array, [sort_index], [sort_order], [by_col])`.
//!
//! Dynamic-array result is an [`ExcelValue::Array`] (including 1×1). This
//! engine does **not** write a spill range into the sheet, so occupied
//! neighbors never produce `#SPILL!` — evaluate returns the array that
//! *would* spill.
//!
//! Documented Excel quirks this module implements:
//!
//! - Default: sort **rows** by column 1, ascending (`sort_index` 1,
//!   `sort_order` 1, `by_col` FALSE).
//! - `sort_index` is 1-based and truncated toward zero. `0`, negative, or
//!   past the sort axis is `#VALUE!`.
//! - `sort_order` must be `1` (asc) or `-1` (desc) after numeric coerce;
//!   anything else (`0`, `2`, text) is `#VALUE!`. `TRUE` → `1`.
//! - `by_col` TRUE sorts **columns** using a row as the key.
//! - Type groups (Excel Data Sort, **not** `<`/`>` ranking): numbers, then
//!   text, then FALSE/TRUE, then errors. **Blanks are last in both
//!   directions.**
//! - Text is case-insensitive. `1`, `"1"`, and `TRUE` stay in different
//!   groups. Numbers use 15-significant-digit equality.
//! - SORT is **stable**: equal keys keep first-occurrence order.
//! - Errors inside the array are values (they sort with the error group).
//!   A scalar error `array` argument surfaces as that error.
//!
//! ## Spill / model limits
//!
//! - The snippet workbook has **no spill grid**. A blocked cell below/right
//!   of the host never yields `#SPILL!`.
//! - Scalar operators (`SORT(...)+1`) take the top-left element via
//!   `scalarize`, not a host-aware intersection of a written spill.
//! - Text order is ASCII case-fold, not locale collation.
//! - Excel's ~1,048,576-row array cap is not enforced; size is memory-bounded.
//! - The parser does not accept omitted middle arguments (`SORT(a,,-1)`).
//!
//! [`sort_apply`] extracts keys once, stably sorts **indices**, then
//! assembles the permutation. [`sort_apply_naive`] insertion-sorts cloned
//! items (and transposes twice for `by_col`) — same answers, more work.
//! Used as the bench "before".

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use std::cmp::Ordering;
use xlsx_types::{excel_num_eq, excel_round_15, EvalError, ExcelError, ExcelValue};

/// Excel `SORT` from already-evaluated arguments.
pub fn sort_apply(
    array: &ExcelValue,
    sort_index: Option<&ExcelValue>,
    sort_order: Option<&ExcelValue>,
    by_col: Option<&ExcelValue>,
) -> ExcelValue {
    sort_kernel(array, sort_index, sort_order, by_col, SortStrategy::Fast)
}

/// O(n²) insertion-sort baseline that clones every item (and transposes for
/// a column sort). Same answers as [`sort_apply`]. Bench "before" only.
pub fn sort_apply_naive(
    array: &ExcelValue,
    sort_index: Option<&ExcelValue>,
    sort_order: Option<&ExcelValue>,
    by_col: Option<&ExcelValue>,
) -> ExcelValue {
    sort_kernel(array, sort_index, sort_order, by_col, SortStrategy::Naive)
}

#[derive(Clone, Copy)]
enum SortStrategy {
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

    let array = ev.eval_expr(&args[0], ctx)?;
    if let ExcelValue::Error(e) = array {
        return Ok(ExcelValue::Error(e));
    }

    let sort_index = if args.len() >= 2 {
        Some(ev.eval_scalar(&args[1], ctx)?)
    } else {
        None
    };
    let sort_order = if args.len() >= 3 {
        Some(ev.eval_scalar(&args[2], ctx)?)
    } else {
        None
    };
    let by_col = if args.len() >= 4 {
        Some(ev.eval_scalar(&args[3], ctx)?)
    } else {
        None
    };

    Ok(sort_apply(
        &array,
        sort_index.as_ref(),
        sort_order.as_ref(),
        by_col.as_ref(),
    ))
}

fn sort_kernel(
    array: &ExcelValue,
    sort_index: Option<&ExcelValue>,
    sort_order: Option<&ExcelValue>,
    by_col: Option<&ExcelValue>,
    strategy: SortStrategy,
) -> ExcelValue {
    if let ExcelValue::Error(e) = array {
        return ExcelValue::Error(*e);
    }

    let index_n = match parse_sort_index(sort_index) {
        Ok(n) => n,
        Err(e) => return ExcelValue::Error(e),
    };
    let descending = match parse_sort_order(sort_order) {
        Ok(d) => d,
        Err(e) => return ExcelValue::Error(e),
    };
    let by_col = match parse_by_col(by_col) {
        Ok(b) => b,
        Err(e) => return ExcelValue::Error(e),
    };

    let grid = match to_grid(array) {
        Ok(g) => g,
        Err(e) => return ExcelValue::Error(e),
    };
    if grid.is_empty() || grid[0].is_empty() {
        return ExcelValue::Array(grid);
    }
    let rows = grid.len();
    let cols = grid[0].len();
    if grid.iter().any(|r| r.len() != cols) {
        return ExcelValue::Error(ExcelError::Value);
    }

    let axis_len = if by_col { rows } else { cols };
    if index_n < 1 || index_n > axis_len as i64 {
        return ExcelValue::Error(ExcelError::Value);
    }
    let key_at = (index_n as usize) - 1;

    match strategy {
        SortStrategy::Fast => permute_fast(&grid, by_col, key_at, descending),
        SortStrategy::Naive => permute_naive(&grid, by_col, key_at, descending),
    }
}

fn parse_by_col(v: Option<&ExcelValue>) -> Result<bool, ExcelError> {
    match v {
        None => Ok(false),
        Some(ExcelValue::Error(e)) => Err(*e),
        Some(other) => coerce::to_logical(other),
    }
}

fn parse_sort_order(v: Option<&ExcelValue>) -> Result<bool, ExcelError> {
    match v {
        None => Ok(false),
        Some(ExcelValue::Error(e)) => Err(*e),
        Some(other) => {
            let n = coerce::to_number(other)?;
            if excel_num_eq(n, 1.0) {
                Ok(false)
            } else if excel_num_eq(n, -1.0) {
                Ok(true)
            } else {
                Err(ExcelError::Value)
            }
        }
    }
}

fn parse_sort_index(v: Option<&ExcelValue>) -> Result<i64, ExcelError> {
    match v {
        None => Ok(1),
        Some(ExcelValue::Error(e)) => Err(*e),
        Some(other) => {
            let n = coerce::to_number(other)?;
            if !n.is_finite() {
                return Err(ExcelError::Value);
            }
            Ok(n.trunc() as i64)
        }
    }
}

fn to_grid(v: &ExcelValue) -> Result<Vec<Vec<ExcelValue>>, ExcelError> {
    match v {
        ExcelValue::Array(rows) => {
            if rows.is_empty() {
                return Ok(rows.clone());
            }
            let cols = rows[0].len();
            if rows.iter().any(|r| r.len() != cols) {
                return Err(ExcelError::Value);
            }
            Ok(rows.clone())
        }
        ExcelValue::Error(e) => Err(*e),
        other => Ok(vec![vec![other.clone()]]),
    }
}

fn permute_fast(
    grid: &[Vec<ExcelValue>],
    by_col: bool,
    key_at: usize,
    descending: bool,
) -> ExcelValue {
    if by_col {
        let cols = grid[0].len();
        let keys: Vec<SortKey> = (0..cols)
            .map(|c| SortKey::from_value(&grid[key_at][c]))
            .collect();
        let mut order: Vec<usize> = (0..cols).collect();
        order.sort_by(|&a, &b| keys[a].cmp_excel(&keys[b], descending));
        let rows = grid.len();
        let mut out = Vec::with_capacity(rows);
        for r in 0..rows {
            let mut row = Vec::with_capacity(cols);
            for &c in &order {
                row.push(grid[r][c].clone());
            }
            out.push(row);
        }
        ExcelValue::Array(out)
    } else {
        let keys: Vec<SortKey> = grid
            .iter()
            .map(|row| SortKey::from_value(&row[key_at]))
            .collect();
        let mut order: Vec<usize> = (0..grid.len()).collect();
        order.sort_by(|&a, &b| keys[a].cmp_excel(&keys[b], descending));
        let mut out = Vec::with_capacity(order.len());
        for i in order {
            out.push(grid[i].clone());
        }
        ExcelValue::Array(out)
    }
}

/// Clone every item, insertion-sort with on-the-fly compares, transpose
/// twice for `by_col`. Same answers as [`permute_fast`].
fn permute_naive(
    grid: &[Vec<ExcelValue>],
    by_col: bool,
    key_at: usize,
    descending: bool,
) -> ExcelValue {
    if by_col {
        let mut items = transpose(grid);
        insertion_sort(&mut items, |a, b| {
            sort_cmp(&a[key_at], &b[key_at], descending)
        });
        ExcelValue::Array(transpose(&items))
    } else {
        let mut items = grid.to_vec();
        insertion_sort(&mut items, |a, b| {
            sort_cmp(&a[key_at], &b[key_at], descending)
        });
        ExcelValue::Array(items)
    }
}

fn insertion_sort<T, F>(items: &mut [T], mut cmp: F)
where
    F: FnMut(&T, &T) -> Ordering,
{
    for i in 1..items.len() {
        let mut j = i;
        while j > 0 && cmp(&items[j - 1], &items[j]) == Ordering::Greater {
            items.swap(j - 1, j);
            j -= 1;
        }
    }
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

fn sort_cmp(a: &ExcelValue, b: &ExcelValue, descending: bool) -> Ordering {
    SortKey::from_value(a).cmp_excel(&SortKey::from_value(b), descending)
}

/// Compact key extracted once on the fast path.
#[derive(Clone, Debug)]
enum SortKey {
    Number(f64),
    Text(String),
    Bool(bool),
    Error,
    Empty,
    Array,
}

impl SortKey {
    fn from_value(v: &ExcelValue) -> Self {
        match v {
            ExcelValue::Empty => Self::Empty,
            ExcelValue::Number(n) => Self::Number(number_key(*n)),
            ExcelValue::Text(s) => Self::Text(s.to_ascii_lowercase()),
            ExcelValue::Bool(b) => Self::Bool(*b),
            ExcelValue::Error(_) => Self::Error,
            ExcelValue::Array(_) => Self::Array,
        }
    }

    fn group(self: &Self) -> u8 {
        match self {
            Self::Number(_) => 0,
            Self::Text(_) => 1,
            Self::Bool(_) => 2,
            Self::Error => 3,
            Self::Array => 4,
            Self::Empty => 5,
        }
    }

    fn cmp_excel(&self, other: &Self, descending: bool) -> Ordering {
        let a_empty = matches!(self, Self::Empty);
        let b_empty = matches!(other, Self::Empty);
        if a_empty && b_empty {
            return Ordering::Equal;
        }
        if a_empty {
            return Ordering::Greater;
        }
        if b_empty {
            return Ordering::Less;
        }

        let ga = self.group();
        let gb = other.group();
        let payload = if ga != gb {
            ga.cmp(&gb)
        } else {
            match (self, other) {
                (Self::Number(a), Self::Number(b)) => a.total_cmp(b),
                (Self::Text(a), Self::Text(b)) => a.cmp(b),
                (Self::Bool(a), Self::Bool(b)) => a.cmp(b),
                _ => Ordering::Equal,
            }
        };
        if descending {
            payload.reverse()
        } else {
            payload
        }
    }
}

fn number_key(n: f64) -> f64 {
    if n == 0.0 {
        0.0
    } else {
        excel_round_15(n)
    }
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
    fn both_eq(
        array: &ExcelValue,
        idx: Option<&ExcelValue>,
        order: Option<&ExcelValue>,
        by_col: Option<&ExcelValue>,
    ) {
        assert_eq!(
            sort_apply(array, idx, order, by_col),
            sort_apply_naive(array, idx, order, by_col)
        );
    }

    #[test]
    fn column_asc_default() {
        let array = col(&[n(3.0), n(1.0), n(2.0)]);
        assert_eq!(
            sort_apply(&array, None, None, None),
            col(&[n(1.0), n(2.0), n(3.0)])
        );
        both_eq(&array, None, None, None);
    }

    #[test]
    fn column_desc() {
        let array = col(&[n(3.0), n(1.0), n(2.0)]);
        let order = n(-1.0);
        assert_eq!(
            sort_apply(&array, None, Some(&order), None),
            col(&[n(3.0), n(2.0), n(1.0)])
        );
        both_eq(&array, None, Some(&order), None);
    }

    #[test]
    fn row_default_is_one_row() {
        let array = ExcelValue::Array(vec![vec![n(3.0), n(1.0), n(2.0)]]);
        assert_eq!(sort_apply(&array, None, None, None), array);
        both_eq(&array, None, None, None);
    }

    #[test]
    fn row_by_col() {
        let array = ExcelValue::Array(vec![vec![n(3.0), n(1.0), n(2.0)]]);
        let by = ExcelValue::Bool(true);
        assert_eq!(
            sort_apply(&array, None, None, Some(&by)),
            ExcelValue::Array(vec![vec![n(1.0), n(2.0), n(3.0)]])
        );
        both_eq(&array, None, None, Some(&by));
    }

    #[test]
    fn sort_index_second_column() {
        let array = ExcelValue::Array(vec![
            vec![n(1.0), n(30.0)],
            vec![n(2.0), n(10.0)],
            vec![n(3.0), n(20.0)],
        ]);
        let idx = n(2.0);
        assert_eq!(
            sort_apply(&array, Some(&idx), None, None),
            ExcelValue::Array(vec![
                vec![n(2.0), n(10.0)],
                vec![n(3.0), n(20.0)],
                vec![n(1.0), n(30.0)],
            ])
        );
        both_eq(&array, Some(&idx), None, None);
    }

    #[test]
    fn type_groups_numbers_text_bools_errors_blanks() {
        let array = col(&[
            ExcelValue::Bool(true),
            t("b"),
            n(2.0),
            ExcelValue::Empty,
            t("a"),
            ExcelValue::Error(ExcelError::Na),
            ExcelValue::Bool(false),
            n(10.0),
        ]);
        assert_eq!(
            sort_apply(&array, None, None, None),
            col(&[
                n(2.0),
                n(10.0),
                t("a"),
                t("b"),
                ExcelValue::Bool(false),
                ExcelValue::Bool(true),
                ExcelValue::Error(ExcelError::Na),
                ExcelValue::Empty,
            ])
        );
        both_eq(&array, None, None, None);
    }

    #[test]
    fn blanks_last_when_descending() {
        let array = col(&[n(2.0), ExcelValue::Empty, n(5.0), n(1.0)]);
        let order = n(-1.0);
        assert_eq!(
            sort_apply(&array, None, Some(&order), None),
            col(&[n(5.0), n(2.0), n(1.0), ExcelValue::Empty])
        );
        both_eq(&array, None, Some(&order), None);
    }

    #[test]
    fn casefold_is_stable() {
        let array = col(&[t("b"), t("A"), t("a")]);
        assert_eq!(
            sort_apply(&array, None, None, None),
            col(&[t("A"), t("a"), t("b")])
        );
        both_eq(&array, None, None, None);
    }

    #[test]
    fn number_vs_text_vs_bool() {
        let array = col(&[n(1.0), t("1"), ExcelValue::Bool(true)]);
        assert_eq!(
            sort_apply(&array, None, None, None),
            col(&[n(1.0), t("1"), ExcelValue::Bool(true)])
        );
        both_eq(&array, None, None, None);
    }

    #[test]
    fn stable_equal_keys() {
        let array = ExcelValue::Array(vec![
            vec![n(2.0), t("first")],
            vec![n(1.0), t("mid")],
            vec![n(2.0), t("second")],
        ]);
        assert_eq!(
            sort_apply(&array, None, None, None),
            ExcelValue::Array(vec![
                vec![n(1.0), t("mid")],
                vec![n(2.0), t("first")],
                vec![n(2.0), t("second")],
            ])
        );
        both_eq(&array, None, None, None);
    }

    #[test]
    fn sort_index_oob_is_value() {
        let array = col(&[n(1.0), n(2.0)]);
        let idx = n(2.0);
        assert_eq!(
            sort_apply(&array, Some(&idx), None, None),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn sort_order_zero_is_value() {
        let array = col(&[n(1.0)]);
        let order = n(0.0);
        assert_eq!(
            sort_apply(&array, None, Some(&order), None),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn sort_index_truncates() {
        let array = ExcelValue::Array(vec![vec![n(1.0), n(30.0)], vec![n(2.0), n(10.0)]]);
        let idx = n(1.9);
        assert_eq!(
            sort_apply(&array, Some(&idx), None, None),
            ExcelValue::Array(vec![vec![n(1.0), n(30.0)], vec![n(2.0), n(10.0)]])
        );
    }

    #[test]
    fn fifteen_digit_numbers_tie() {
        let a = 0.1 + 0.2;
        let array = col(&[n(a), n(0.3), n(0.2)]);
        assert_eq!(
            sort_apply(&array, None, None, None),
            col(&[n(0.2), n(a), n(0.3)])
        );
        both_eq(&array, None, None, None);
    }

    #[test]
    fn scalar_is_one_by_one() {
        assert_eq!(
            sort_apply(&n(5.0), None, None, None),
            ExcelValue::Array(vec![vec![n(5.0)]])
        );
    }

    #[test]
    fn empty_string_is_text_not_blank() {
        let array = col(&[n(1.0), t(""), ExcelValue::Empty]);
        assert_eq!(
            sort_apply(&array, None, None, None),
            col(&[n(1.0), t(""), ExcelValue::Empty])
        );
        both_eq(&array, None, None, None);
    }

    #[test]
    fn negatives_before_positives() {
        let array = col(&[n(1.0), n(-2.0), n(0.0), n(-0.5)]);
        assert_eq!(
            sort_apply(&array, None, None, None),
            col(&[n(-2.0), n(-0.5), n(0.0), n(1.0)])
        );
        both_eq(&array, None, None, None);
    }

    #[test]
    fn large_row_sort_matches_naive() {
        let n_rows = 256usize;
        let mut rows = Vec::with_capacity(n_rows);
        for i in 0..n_rows {
            rows.push(vec![n((n_rows - 1 - i) as f64), t("x")]);
        }
        let array = ExcelValue::Array(rows);
        both_eq(&array, None, None, None);
        match sort_apply(&array, None, None, None) {
            ExcelValue::Array(out) => {
                assert_eq!(out[0][0], n(0.0));
                assert_eq!(out[n_rows - 1][0], n((n_rows - 1) as f64));
            }
            other => panic!("{other:?}"),
        }
    }
}
