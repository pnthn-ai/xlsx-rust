//! Excel `UNIQUE(array, [by_col], [exactly_once])`.
//!
//! Dynamic-array result is an [`ExcelValue::Array`] (including 1×1). This
//! engine does **not** write a spill range into the sheet, so occupied
//! neighbors never produce `#SPILL!` — evaluate returns the array that
//! *would* spill.
//!
//! Distinctness is type-aware and case-insensitive for text:
//! - `"A"` and `"a"` are the same (first spelling is kept)
//! - `1` and `"1"` are different; `TRUE` and `1` are different
//! - blanks are a real value (several blanks collapse to one empty)
//! - omitted `by_col` / `exactly_once` are FALSE
//! - no survivors under `exactly_once` → `#CALC!`

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use xlsx_types::{excel_round_15, EvalError, ExcelError, ExcelValue};

pub(crate) fn eval(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.is_empty() || args.len() > 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }

    let array = ev.eval_expr(&args[0], ctx)?;
    if let ExcelValue::Error(e) = array {
        return Ok(ExcelValue::Error(e));
    }

    let by_col = if args.len() >= 2 {
        match logical_flag(ev, &args[1], ctx)? {
            Ok(b) => b,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        false
    };
    let exactly_once = if args.len() >= 3 {
        match logical_flag(ev, &args[2], ctx)? {
            Ok(b) => b,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        false
    };

    let grid = match to_grid(array) {
        Ok(g) => g,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    Ok(unique_apply(&grid, by_col, exactly_once))
}

fn logical_flag(
    ev: &Evaluator,
    expr: &Expr,
    ctx: &mut Ctx<'_>,
) -> Result<Result<bool, ExcelError>, EvalError> {
    let v = ev.eval_scalar(expr, ctx)?;
    Ok(coerce::to_logical(&v))
}

fn to_grid(v: ExcelValue) -> Result<Vec<Vec<ExcelValue>>, ExcelError> {
    match v {
        ExcelValue::Array(rows) => {
            if rows.is_empty() {
                return Ok(rows);
            }
            let cols = rows[0].len();
            if rows.iter().any(|r| r.len() != cols) {
                return Err(ExcelError::Value);
            }
            Ok(rows)
        }
        other => Ok(vec![vec![other]]),
    }
}

/// Production hash kernel. Used by the evaluator and by benches.
pub fn unique_apply(grid: &[Vec<ExcelValue>], by_col: bool, exactly_once: bool) -> ExcelValue {
    unique_kernel(grid, by_col, exactly_once, Dedup::Hash)
}

/// O(n²) pairwise walk — bench baseline only.
pub fn unique_apply_naive(
    grid: &[Vec<ExcelValue>],
    by_col: bool,
    exactly_once: bool,
) -> ExcelValue {
    unique_kernel(grid, by_col, exactly_once, Dedup::Naive)
}

#[derive(Clone, Copy)]
enum Dedup {
    Hash,
    Naive,
}

fn unique_kernel(
    grid: &[Vec<ExcelValue>],
    by_col: bool,
    exactly_once: bool,
    mode: Dedup,
) -> ExcelValue {
    if grid.is_empty() {
        return ExcelValue::Error(ExcelError::Calc);
    }
    let cols = grid[0].len();
    if cols == 0 {
        return ExcelValue::Error(ExcelError::Calc);
    }
    if grid.iter().any(|r| r.len() != cols) {
        return ExcelValue::Error(ExcelError::Value);
    }

    let items: Vec<Vec<ExcelValue>> = if by_col {
        (0..cols)
            .map(|c| grid.iter().map(|row| row[c].clone()).collect())
            .collect()
    } else {
        grid.to_vec()
    };

    let kept = match mode {
        Dedup::Hash => dedup_hash(&items, exactly_once),
        Dedup::Naive => dedup_naive(&items, exactly_once),
    };
    if kept.is_empty() {
        return ExcelValue::Error(ExcelError::Calc);
    }
    let out = if by_col { columns_to_rows(&kept) } else { kept };
    ExcelValue::Array(out)
}

fn columns_to_rows(cols: &[Vec<ExcelValue>]) -> Vec<Vec<ExcelValue>> {
    if cols.is_empty() {
        return Vec::new();
    }
    let height = cols[0].len();
    (0..height)
        .map(|r| cols.iter().map(|col| col[r].clone()).collect())
        .collect()
}

fn dedup_hash(items: &[Vec<ExcelValue>], exactly_once: bool) -> Vec<Vec<ExcelValue>> {
    let mut first: HashMap<ItemKey, usize> = HashMap::with_capacity(items.len());
    let mut counts: Vec<usize> = Vec::with_capacity(items.len());
    let mut order: Vec<Vec<ExcelValue>> = Vec::with_capacity(items.len());
    for item in items {
        let key = ItemKey::from_cells(item);
        if let Some(&idx) = first.get(&key) {
            counts[idx] += 1;
        } else {
            first.insert(key, order.len());
            counts.push(1);
            order.push(item.clone());
        }
    }
    filter_once(order, &counts, exactly_once)
}

fn dedup_naive(items: &[Vec<ExcelValue>], exactly_once: bool) -> Vec<Vec<ExcelValue>> {
    let mut order: Vec<Vec<ExcelValue>> = Vec::new();
    let mut counts: Vec<usize> = Vec::new();
    for item in items {
        if let Some(i) = order.iter().position(|seen| items_eq(seen, item)) {
            counts[i] += 1;
        } else {
            counts.push(1);
            order.push(item.clone());
        }
    }
    filter_once(order, &counts, exactly_once)
}

fn filter_once(
    order: Vec<Vec<ExcelValue>>,
    counts: &[usize],
    exactly_once: bool,
) -> Vec<Vec<ExcelValue>> {
    if !exactly_once {
        return order;
    }
    order
        .into_iter()
        .enumerate()
        .filter_map(|(i, row)| if counts[i] == 1 { Some(row) } else { None })
        .collect()
}

fn items_eq(a: &[ExcelValue], b: &[ExcelValue]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| unique_eq(x, y))
}

/// Type-strict UNIQUE equality (not Excel `=`).
pub fn unique_eq(a: &ExcelValue, b: &ExcelValue) -> bool {
    match (a, b) {
        (ExcelValue::Empty, ExcelValue::Empty) => true,
        (ExcelValue::Number(x), ExcelValue::Number(y)) => number_key(*x) == number_key(*y),
        (ExcelValue::Text(x), ExcelValue::Text(y)) => x.eq_ignore_ascii_case(y),
        (ExcelValue::Bool(x), ExcelValue::Bool(y)) => x == y,
        (ExcelValue::Error(x), ExcelValue::Error(y)) => x == y,
        _ => false,
    }
}

fn number_key(n: f64) -> u64 {
    if n == 0.0 {
        0.0f64.to_bits()
    } else {
        excel_round_15(n).to_bits()
    }
}

#[derive(Clone, Eq)]
struct ItemKey(Vec<CellKey>);

impl ItemKey {
    fn from_cells(cells: &[ExcelValue]) -> Self {
        Self(cells.iter().map(CellKey::from_value).collect())
    }
}

impl PartialEq for ItemKey {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Hash for ItemKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum CellKey {
    Empty,
    Number(u64),
    Text(String),
    Bool(bool),
    Error(ExcelError),
    Array,
}

impl CellKey {
    fn from_value(v: &ExcelValue) -> Self {
        match v {
            ExcelValue::Empty => Self::Empty,
            ExcelValue::Number(n) => Self::Number(number_key(*n)),
            ExcelValue::Text(s) => Self::Text(s.to_ascii_lowercase()),
            ExcelValue::Bool(b) => Self::Bool(*b),
            ExcelValue::Error(e) => Self::Error(*e),
            ExcelValue::Array(_) => Self::Array,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(vals: &[ExcelValue]) -> Vec<Vec<ExcelValue>> {
        vals.iter().cloned().map(|v| vec![v]).collect()
    }

    fn n(x: f64) -> ExcelValue {
        ExcelValue::Number(x)
    }

    fn t(s: &str) -> ExcelValue {
        ExcelValue::Text(s.into())
    }

    #[test]
    fn first_occurrence_order() {
        let g = col(&[n(1.0), n(2.0), n(1.0), n(3.0), n(2.0)]);
        assert_eq!(
            unique_apply(&g, false, false),
            ExcelValue::Array(col(&[n(1.0), n(2.0), n(3.0)]))
        );
    }

    #[test]
    fn exactly_once_drops_repeats() {
        let g = col(&[n(1.0), n(2.0), n(2.0), n(3.0)]);
        assert_eq!(
            unique_apply(&g, false, true),
            ExcelValue::Array(col(&[n(1.0), n(3.0)]))
        );
    }

    #[test]
    fn exactly_once_empty_is_calc() {
        let g = col(&[n(1.0), n(1.0)]);
        assert_eq!(
            unique_apply(&g, false, true),
            ExcelValue::Error(ExcelError::Calc)
        );
    }

    #[test]
    fn casefold_keeps_first_spelling() {
        let g = col(&[t("Apple"), t("apple"), t("APPLE")]);
        assert_eq!(
            unique_apply(&g, false, false),
            ExcelValue::Array(col(&[t("Apple")]))
        );
    }

    #[test]
    fn types_stay_distinct() {
        let g = col(&[n(1.0), t("1"), ExcelValue::Bool(true)]);
        assert_eq!(
            unique_apply(&g, false, false),
            ExcelValue::Array(col(&[n(1.0), t("1"), ExcelValue::Bool(true)]))
        );
        assert!(!unique_eq(&n(1.0), &ExcelValue::Bool(true)));
        assert!(!unique_eq(&n(0.0), &ExcelValue::Empty));
        assert!(!unique_eq(&t(""), &ExcelValue::Empty));
    }

    #[test]
    fn blanks_collapse() {
        let g = col(&[n(1.0), ExcelValue::Empty, n(1.0), ExcelValue::Empty]);
        assert_eq!(
            unique_apply(&g, false, false),
            ExcelValue::Array(col(&[n(1.0), ExcelValue::Empty]))
        );
    }

    #[test]
    fn by_col_on_row() {
        let g = vec![vec![n(1.0), n(2.0), n(1.0)]];
        assert_eq!(
            unique_apply(&g, true, false),
            ExcelValue::Array(vec![vec![n(1.0), n(2.0)]])
        );
        // by_col omitted (rows): a single row is already unique
        assert_eq!(
            unique_apply(&g, false, false),
            ExcelValue::Array(vec![vec![n(1.0), n(2.0), n(1.0)]])
        );
    }

    #[test]
    fn hash_matches_naive() {
        let g = vec![
            vec![n(1.0), t("A")],
            vec![n(1.0), t("a")],
            vec![n(2.0), t("B")],
            vec![ExcelValue::Empty, n(0.0)],
            vec![n(2.0), t("b")],
        ];
        for by_col in [false, true] {
            for once in [false, true] {
                assert_eq!(
                    unique_apply(&g, by_col, once),
                    unique_apply_naive(&g, by_col, once),
                    "by_col={by_col} exactly_once={once}"
                );
            }
        }
    }

    #[test]
    fn fifteen_digit_numbers_match() {
        let a = 0.1 + 0.2;
        let g = col(&[n(a), n(0.3)]);
        assert_eq!(
            unique_apply(&g, false, false),
            ExcelValue::Array(col(&[n(a)]))
        );
    }
}
