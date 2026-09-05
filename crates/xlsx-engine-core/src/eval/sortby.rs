//! Excel `SORTBY(array, by_array1, [sort_order1], [by_array2, sort_order2], …)`.
//!
//! Dynamic-array result is an [`ExcelValue::Array`] (including 1×1). This
//! engine does **not** write a spill range into the sheet, so occupied
//! neighbors never produce `#SPILL!` — evaluate returns the array that
//! *would* spill.
//!
//! Documented Excel quirks this module implements:
//!
//! - `by_array` must be a **vector** (one row or one column). A matrix is
//!   `#VALUE!`.
//! - The first `by_array` picks the axis: a column matching `array` height
//!   sorts **rows**; a row matching `array` width sorts **columns**. A 1-D
//!   transpose (column `array` + row keys of the same length, or the reverse)
//!   is accepted. A 1×1 key only matches a 1×1 `array`.
//! - Later keys must be vectors of the **same length** as the first.
//! - `sort_order` is `1` ascending (default) or `-1` descending after numeric
//!   coerce; anything else is `#VALUE!`. `TRUE` → `1`. Arguments after
//!   `array` are `(by, order)` pairs; a missing trailing order defaults to 1.
//!   Skipping an order in the middle shifts the next `by_array` into the
//!   order slot (usually `#VALUE!`).
//! - At most [`MAX_SORT_KEYS`] (64) `by_array` arguments, matching Excel.
//! - Type groups follow Excel Data Sort — **not** `<`/`>` ranking: numbers,
//!   then text, then FALSE/TRUE, then errors. **Blanks are last in both
//!   directions.** Text is case-insensitive ASCII. `1`, `"1"`, and `TRUE`
//!   stay in different groups. Numbers use 15-significant-digit equality.
//! - SORTBY is **stable**: equal keys (after all tie-breakers) keep
//!   first-occurrence order.
//! - Errors inside `array` / `by_array` are values. A scalar error argument
//!   surfaces left-to-right (`array`, then each `by_array`, then its order).
//!
//! ## Spill / model limits
//!
//! - The snippet workbook has **no spill grid**. A blocked cell below/right
//!   of the host never yields `#SPILL!`.
//! - Scalar operators (`SORTBY(...)+1`) take the top-left element via
//!   `scalarize`, not a host-aware intersection of a written spill.
//! - Text order is ASCII case-fold, not locale collation.
//! - Excel's ~1,048,576-row array cap is not enforced; size is memory-bounded.
//! - The parser does not accept omitted middle arguments
//!   (`SORTBY(a, by1,, by2)`).
//!
//! [`sortby_apply`] extracts keys once, stably sorts **indices**, then
//! assembles the permutation. [`sortby_apply_naive`] insertion-sorts cloned
//! items (and transposes twice for a column sort) — same answers, more work.
//! Used as the bench "before".

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use std::cmp::Ordering;
use xlsx_types::{excel_num_eq, excel_round_15, EvalError, ExcelError, ExcelValue};

/// Excel's documented cap on `by_array` / `sort_order` pairings.
pub const MAX_SORT_KEYS: usize = 64;

/// One evaluated `(by_array, sort_order)` pair. `sort_order` `None` means
/// omitted (ascending).
pub type SortByPair<'a> = (&'a ExcelValue, Option<&'a ExcelValue>);

/// Excel `SORTBY` from already-evaluated arguments.
pub fn sortby_apply(array: &ExcelValue, keys: &[SortByPair<'_>]) -> ExcelValue {
    sortby_kernel(array, keys, SortStrategy::Fast)
}

/// O(n²) insertion-sort baseline that clones every item (and transposes for
/// a column sort). Same answers as [`sortby_apply`]. Bench "before" only.
pub fn sortby_apply_naive(array: &ExcelValue, keys: &[SortByPair<'_>]) -> ExcelValue {
    sortby_kernel(array, keys, SortStrategy::Naive)
}

#[derive(Clone, Copy)]
enum SortStrategy {
    Fast,
    Naive,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Axis {
    Rows,
    Cols,
}

pub(crate) fn eval(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }

    let array = ev.eval_expr(&args[0], ctx)?;

    let mut owned: Vec<(ExcelValue, Option<ExcelValue>)> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        let by = ev.eval_expr(&args[i], ctx)?;
        i += 1;
        let order = if i < args.len() {
            let o = ev.eval_scalar(&args[i], ctx)?;
            i += 1;
            Some(o)
        } else {
            None
        };
        owned.push((by, order));
    }

    if owned.len() > MAX_SORT_KEYS {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }

    let refs: Vec<SortByPair<'_>> = owned
        .iter()
        .map(|(by, order)| (by, order.as_ref()))
        .collect();
    Ok(sortby_apply(&array, &refs))
}

fn sortby_kernel(
    array: &ExcelValue,
    keys: &[SortByPair<'_>],
    strategy: SortStrategy,
) -> ExcelValue {
    if let ExcelValue::Error(e) = array {
        return ExcelValue::Error(*e);
    }
    if keys.is_empty() || keys.len() > MAX_SORT_KEYS {
        return ExcelValue::Error(ExcelError::Value);
    }

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

    let (first_keys, first_desc, axis) = match parse_first_key(keys[0], rows, cols) {
        Ok(v) => v,
        Err(e) => return ExcelValue::Error(e),
    };
    let n = first_keys.len();

    let mut extracted: Vec<(Vec<SortKey>, bool)> = Vec::with_capacity(keys.len());
    extracted.push((first_keys, first_desc));
    for pair in &keys[1..] {
        match parse_next_key(*pair, n) {
            Ok((ks, desc)) => extracted.push((ks, desc)),
            Err(e) => return ExcelValue::Error(e),
        }
    }

    match strategy {
        SortStrategy::Fast => permute_fast(&grid, axis, &extracted),
        SortStrategy::Naive => permute_naive(&grid, axis, &extracted),
    }
}

fn parse_first_key(
    pair: SortByPair<'_>,
    rows: usize,
    cols: usize,
) -> Result<(Vec<SortKey>, bool, Axis), ExcelError> {
    let (by, order) = pair;
    if let ExcelValue::Error(e) = by {
        return Err(*e);
    }
    let descending = parse_sort_order(order)?;
    let (vec, br, bc) = to_vector(by)?;
    let axis = resolve_axis(rows, cols, br, bc)?;
    let expected = match axis {
        Axis::Rows => rows,
        Axis::Cols => cols,
    };
    if vec.len() != expected {
        return Err(ExcelError::Value);
    }
    Ok((
        vec.iter().map(SortKey::from_value).collect(),
        descending,
        axis,
    ))
}

fn parse_next_key(
    pair: SortByPair<'_>,
    expected_len: usize,
) -> Result<(Vec<SortKey>, bool), ExcelError> {
    let (by, order) = pair;
    if let ExcelValue::Error(e) = by {
        return Err(*e);
    }
    let descending = parse_sort_order(order)?;
    let (vec, br, bc) = to_vector(by)?;
    if br > 1 && bc > 1 {
        return Err(ExcelError::Value);
    }
    if vec.len() != expected_len {
        return Err(ExcelError::Value);
    }
    Ok((vec.iter().map(SortKey::from_value).collect(), descending))
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

/// Flatten a vector-shaped value. Returns `(cells, nrows, ncols)`.
fn to_vector(v: &ExcelValue) -> Result<(Vec<ExcelValue>, usize, usize), ExcelError> {
    match v {
        ExcelValue::Error(e) => Err(*e),
        ExcelValue::Array(rows) => {
            if rows.is_empty() {
                return Ok((Vec::new(), 0, 0));
            }
            let cols = rows[0].len();
            if cols == 0 || rows.iter().any(|r| r.len() != cols) {
                return Err(ExcelError::Value);
            }
            let br = rows.len();
            if br > 1 && cols > 1 {
                return Err(ExcelError::Value);
            }
            let mut out = Vec::with_capacity(br * cols);
            for row in rows {
                out.extend(row.iter().cloned());
            }
            Ok((out, br, cols))
        }
        other => Ok((vec![other.clone()], 1, 1)),
    }
}

fn resolve_axis(
    array_rows: usize,
    array_cols: usize,
    by_rows: usize,
    by_cols: usize,
) -> Result<Axis, ExcelError> {
    if by_rows > 1 && by_cols > 1 {
        return Err(ExcelError::Value);
    }
    if by_rows == 1 && by_cols == 1 {
        return if array_rows == 1 && array_cols == 1 {
            Ok(Axis::Rows)
        } else {
            Err(ExcelError::Value)
        };
    }
    if by_cols == 1 {
        if by_rows == array_rows {
            return Ok(Axis::Rows);
        }
        if array_rows == 1 && by_rows == array_cols {
            return Ok(Axis::Cols);
        }
        return Err(ExcelError::Value);
    }
    if by_rows == 1 {
        if by_cols == array_cols {
            return Ok(Axis::Cols);
        }
        if array_cols == 1 && by_cols == array_rows {
            return Ok(Axis::Rows);
        }
    }
    Err(ExcelError::Value)
}

fn cmp_keys(extracted: &[(Vec<SortKey>, bool)], a: usize, b: usize) -> Ordering {
    for (keys, descending) in extracted {
        let ord = keys[a].cmp_excel(&keys[b], *descending);
        if ord != Ordering::Equal {
            return ord;
        }
    }
    Ordering::Equal
}

fn permute_fast(
    grid: &[Vec<ExcelValue>],
    axis: Axis,
    extracted: &[(Vec<SortKey>, bool)],
) -> ExcelValue {
    match axis {
        Axis::Cols => {
            let cols = grid[0].len();
            let mut order: Vec<usize> = (0..cols).collect();
            order.sort_by(|&a, &b| cmp_keys(extracted, a, b));
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
        }
        Axis::Rows => {
            let mut order: Vec<usize> = (0..grid.len()).collect();
            order.sort_by(|&a, &b| cmp_keys(extracted, a, b));
            let mut out = Vec::with_capacity(order.len());
            for i in order {
                out.push(grid[i].clone());
            }
            ExcelValue::Array(out)
        }
    }
}

/// Clone every item, insertion-sort with on-the-fly compares, transpose
/// twice for a column sort. Same answers as [`permute_fast`].
fn permute_naive(
    grid: &[Vec<ExcelValue>],
    axis: Axis,
    extracted: &[(Vec<SortKey>, bool)],
) -> ExcelValue {
    match axis {
        Axis::Cols => {
            let mut items: Vec<(usize, Vec<ExcelValue>)> =
                transpose(grid).into_iter().enumerate().collect();
            insertion_sort(&mut items, |a, b| cmp_keys(extracted, a, b));
            let cols: Vec<Vec<ExcelValue>> = items.into_iter().map(|(_, col)| col).collect();
            ExcelValue::Array(transpose(&cols))
        }
        Axis::Rows => {
            let mut items: Vec<(usize, Vec<ExcelValue>)> =
                grid.iter().cloned().enumerate().collect();
            insertion_sort(&mut items, |a, b| cmp_keys(extracted, a, b));
            ExcelValue::Array(items.into_iter().map(|(_, row)| row).collect())
        }
    }
}

fn insertion_sort(
    items: &mut [(usize, Vec<ExcelValue>)],
    mut cmp_orig: impl FnMut(usize, usize) -> Ordering,
) {
    for i in 1..items.len() {
        let mut j = i;
        while j > 0 && cmp_orig(items[j - 1].0, items[j].0) == Ordering::Greater {
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

    fn group(&self) -> u8 {
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
    fn row(vals: &[ExcelValue]) -> ExcelValue {
        ExcelValue::Array(vec![vals.to_vec()])
    }

    fn both_eq(array: &ExcelValue, keys: &[SortByPair<'_>]) {
        assert_eq!(sortby_apply(array, keys), sortby_apply_naive(array, keys));
    }

    #[test]
    fn column_asc_default() {
        let array = col(&[n(3.0), n(1.0), n(2.0)]);
        let by = col(&[n(30.0), n(10.0), n(20.0)]);
        let keys = [(&by, None)];
        assert_eq!(sortby_apply(&array, &keys), col(&[n(1.0), n(2.0), n(3.0)]));
        both_eq(&array, &keys);
    }

    #[test]
    fn column_desc() {
        let array = col(&[n(3.0), n(1.0), n(2.0)]);
        let by = col(&[n(30.0), n(10.0), n(20.0)]);
        let order = n(-1.0);
        let keys = [(&by, Some(&order))];
        assert_eq!(sortby_apply(&array, &keys), col(&[n(3.0), n(2.0), n(1.0)]));
        both_eq(&array, &keys);
    }

    #[test]
    fn row_by_row() {
        let array = row(&[n(3.0), n(1.0), n(2.0)]);
        let by = row(&[n(30.0), n(10.0), n(20.0)]);
        let keys = [(&by, None)];
        assert_eq!(sortby_apply(&array, &keys), row(&[n(1.0), n(2.0), n(3.0)]));
        both_eq(&array, &keys);
    }

    #[test]
    fn one_d_transpose_row_keys_on_column() {
        let array = col(&[n(3.0), n(1.0), n(2.0)]);
        let by = row(&[n(30.0), n(10.0), n(20.0)]);
        let keys = [(&by, None)];
        assert_eq!(sortby_apply(&array, &keys), col(&[n(1.0), n(2.0), n(3.0)]));
        both_eq(&array, &keys);
    }

    #[test]
    fn two_d_sort_rows() {
        let array = ExcelValue::Array(vec![
            vec![n(1.0), t("c")],
            vec![n(2.0), t("a")],
            vec![n(3.0), t("b")],
        ]);
        let by = col(&[n(30.0), n(10.0), n(20.0)]);
        let keys = [(&by, None)];
        assert_eq!(
            sortby_apply(&array, &keys),
            ExcelValue::Array(vec![
                vec![n(2.0), t("a")],
                vec![n(3.0), t("b")],
                vec![n(1.0), t("c")],
            ])
        );
        both_eq(&array, &keys);
    }

    #[test]
    fn two_d_sort_cols() {
        let array = ExcelValue::Array(vec![
            vec![n(3.0), n(1.0), n(2.0)],
            vec![t("c"), t("a"), t("b")],
        ]);
        let by = row(&[n(30.0), n(10.0), n(20.0)]);
        let keys = [(&by, None)];
        assert_eq!(
            sortby_apply(&array, &keys),
            ExcelValue::Array(vec![
                vec![n(1.0), n(2.0), n(3.0)],
                vec![t("a"), t("b"), t("c")],
            ])
        );
        both_eq(&array, &keys);
    }

    #[test]
    fn multi_key_tie_break() {
        let array = col(&[t("a"), t("b"), t("c"), t("d")]);
        let k1 = col(&[n(1.0), n(1.0), n(2.0), n(2.0)]);
        let k2 = col(&[n(20.0), n(10.0), n(40.0), n(30.0)]);
        let keys = [(&k1, None), (&k2, None)];
        assert_eq!(
            sortby_apply(&array, &keys),
            col(&[t("b"), t("a"), t("d"), t("c")])
        );
        both_eq(&array, &keys);
    }

    #[test]
    fn type_groups_and_blanks_last() {
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
        let keys = [(&array, None)];
        assert_eq!(
            sortby_apply(&array, &keys),
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
        both_eq(&array, &keys);
    }

    #[test]
    fn blanks_last_when_descending() {
        let array = col(&[n(2.0), ExcelValue::Empty, n(5.0), n(1.0)]);
        let order = n(-1.0);
        let keys = [(&array, Some(&order))];
        assert_eq!(
            sortby_apply(&array, &keys),
            col(&[n(5.0), n(2.0), n(1.0), ExcelValue::Empty])
        );
        both_eq(&array, &keys);
    }

    #[test]
    fn casefold_is_stable() {
        let array = col(&[t("b"), t("A"), t("a")]);
        let keys = [(&array, None)];
        assert_eq!(sortby_apply(&array, &keys), col(&[t("A"), t("a"), t("b")]));
        both_eq(&array, &keys);
    }

    #[test]
    fn dim_mismatch_is_value() {
        let array = col(&[n(1.0), n(2.0), n(3.0)]);
        let by = col(&[n(1.0), n(2.0)]);
        let keys = [(&by, None)];
        assert_eq!(
            sortby_apply(&array, &keys),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn matrix_by_array_is_value() {
        let array = col(&[n(1.0), n(2.0)]);
        let by = ExcelValue::Array(vec![vec![n(1.0), n(2.0)], vec![n(3.0), n(4.0)]]);
        let keys = [(&by, None)];
        assert_eq!(
            sortby_apply(&array, &keys),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn sort_order_zero_is_value() {
        let array = col(&[n(1.0)]);
        let by = col(&[n(1.0)]);
        let order = n(0.0);
        let keys = [(&by, Some(&order))];
        assert_eq!(
            sortby_apply(&array, &keys),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn scalar_is_one_by_one() {
        let array = n(5.0);
        let by = n(1.0);
        let keys = [(&by, None)];
        assert_eq!(
            sortby_apply(&array, &keys),
            ExcelValue::Array(vec![vec![n(5.0)]])
        );
    }

    #[test]
    fn scalar_error_array_wins() {
        let array = ExcelValue::Error(ExcelError::Na);
        let by = ExcelValue::Error(ExcelError::Div0);
        let keys = [(&by, None)];
        assert_eq!(
            sortby_apply(&array, &keys),
            ExcelValue::Error(ExcelError::Na)
        );
    }

    #[test]
    fn empty_string_is_text_not_blank() {
        let array = col(&[n(1.0), t(""), ExcelValue::Empty]);
        let keys = [(&array, None)];
        assert_eq!(
            sortby_apply(&array, &keys),
            col(&[n(1.0), t(""), ExcelValue::Empty])
        );
        both_eq(&array, &keys);
    }

    #[test]
    fn fifteen_digit_numbers_tie() {
        let a = 0.1 + 0.2;
        let array = col(&[n(a), n(0.3), n(0.2)]);
        let keys = [(&array, None)];
        assert_eq!(sortby_apply(&array, &keys), col(&[n(0.2), n(a), n(0.3)]));
        both_eq(&array, &keys);
    }

    #[test]
    fn large_row_sort_matches_naive() {
        let n_rows = 256usize;
        let mut rows = Vec::with_capacity(n_rows);
        let mut keys_col = Vec::with_capacity(n_rows);
        for i in 0..n_rows {
            rows.push(vec![n((n_rows - 1 - i) as f64), t("x")]);
            keys_col.push(vec![n((n_rows - 1 - i) as f64)]);
        }
        let array = ExcelValue::Array(rows);
        let by = ExcelValue::Array(keys_col);
        let keys = [(&by, None)];
        both_eq(&array, &keys);
        match sortby_apply(&array, &keys) {
            ExcelValue::Array(out) => {
                assert_eq!(out[0][0], n(0.0));
                assert_eq!(out[n_rows - 1][0], n((n_rows - 1) as f64));
            }
            other => panic!("{other:?}"),
        }
    }
}
