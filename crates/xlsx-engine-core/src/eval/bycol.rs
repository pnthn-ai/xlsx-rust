//! Excel `BYCOL(array, LAMBDA(col, body))`.
//!
//! Applies a one-parameter LAMBDA to each column of `array` and returns a
//! **1-row** array of results (one cell per source column).
//!
//! The column parameter is a vertical vector (`n×1` [`ExcelValue::Array`]).
//! A single-row source binds each column as a **scalar** so `c*2` on
//! `{1,2,3}` works. A multi-row source binds an `n×1` array; `SUM(c)`
//! then folds that vector.
//!
//! Documented Excel quirks this module implements:
//!
//! - The LAMBDA must have **exactly one** name parameter (inline or a
//!   defined name that refers to one). Anything else is `#VALUE!`.
//! - Eta-reduced helpers (`BYCOL(array, SUM)`) are accepted for
//!   `SUM` / `AVERAGE` / `MIN` / `MAX` / `COUNT` / `COUNTA` / `PRODUCT`
//!   — same answers as `LAMBDA(c, SUM(c))`.
//! - A body that returns a multi-cell array is `#CALC!` **in that
//!   result cell**. Other columns still compute. Excel 365 surfaces a
//!   single `#CALC!` ("Nested arrays are not supported") for the whole
//!   spill when any column returns an array; this engine keeps the
//!   per-column `#CALC!` so a later column is still visible.
//! - A 1×1 array result is unpacked to a scalar (single-value array).
//! - An error produced by the body stays in that cell.
//! - A scalar first argument is a 1×1 array (one column).
//!
//! ## Spill / model limits
//!
//! - The engine returns an [`ExcelValue::Array`]; it does **not** write a
//!   spill range. Occupied neighbors never produce `#SPILL!`.
//! - Bare `LAMBDA(...)` (not consumed by BYCOL / MAKEARRAY) is `#CALC!`.
//! - Immediately-invoked `LAMBDA(...)(args)` is not parsed.
//! - Optional LAMBDA parameters and `LET` helpers are out of scope.
//! - Parameter names that tokenize as A1 refs are not supported.
//!
//! [`reduce_fast`] walks each column in place for the common reducers
//! (`SUM(c)`, `AVERAGE(c)`, …). [`reduce_naive`] clones every column
//! into a fresh vector first — same answers, more allocation. Used as
//! the bench "before".

use super::makearray::{fn_key, names_eq, resolve_lambda_n, MAX_COLS};
use super::{Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

const EMPTY: ExcelValue = ExcelValue::Empty;

/// A LAMBDA body that reduces a column without walking the AST.
#[derive(Clone, Debug, PartialEq)]
pub enum ColOp {
    Sum,
    Average,
    Min,
    Max,
    Count,
    CountA,
    Product,
    Const(ExcelValue),
}

fn grid_dims(rows: &[Vec<ExcelValue>]) -> Result<(usize, usize), ExcelError> {
    if rows.is_empty() {
        return Err(ExcelError::Value);
    }
    let cols = rows[0].len();
    if cols == 0 || rows.iter().any(|r| r.len() != cols) {
        return Err(ExcelError::Value);
    }
    if cols > MAX_COLS {
        return Err(ExcelError::Num);
    }
    Ok((rows.len(), cols))
}

/// Build the 1-row result array.
pub fn row_result(cells: Vec<ExcelValue>) -> ExcelValue {
    ExcelValue::Array(vec![cells])
}

/// A body result: multi-cell array → `#CALC!`; 1×1 unpacks.
pub fn scalar_result(v: ExcelValue) -> ExcelValue {
    match v {
        ExcelValue::Array(rows) => {
            let mut first = None;
            let mut n = 0usize;
            for row in &rows {
                for cell in row {
                    n += 1;
                    if n == 1 {
                        first = Some(cell.clone());
                    }
                }
            }
            if n == 1 {
                first.unwrap_or(ExcelValue::Empty)
            } else {
                ExcelValue::Error(ExcelError::Calc)
            }
        }
        other => other,
    }
}

/// One column as the value bound to the LAMBDA parameter.
pub fn column_arg(rows: &[Vec<ExcelValue>], col: usize) -> ExcelValue {
    if rows.len() == 1 {
        return rows[0].get(col).cloned().unwrap_or(EMPTY);
    }
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(vec![row.get(col).cloned().unwrap_or(EMPTY)]);
    }
    ExcelValue::Array(out)
}

/// Classify `SUM(col)` / `AVERAGE(col)` / a literal. Anything else is `None`.
pub fn classify(body: &Expr, param: &str) -> Option<ColOp> {
    match body {
        Expr::Number(n) => Some(ColOp::Const(ExcelValue::Number(*n))),
        Expr::Text(s) => Some(ColOp::Const(ExcelValue::Text(s.clone()))),
        Expr::Bool(b) => Some(ColOp::Const(ExcelValue::Bool(*b))),
        Expr::Error(e) => Some(ColOp::Const(ExcelValue::Error(*e))),
        Expr::Call { name, args } if args.len() == 1 && is_param(&args[0], param) => {
            match fn_key(name).as_str() {
                "SUM" => Some(ColOp::Sum),
                "AVERAGE" => Some(ColOp::Average),
                "MIN" => Some(ColOp::Min),
                "MAX" => Some(ColOp::Max),
                "COUNT" => Some(ColOp::Count),
                "COUNTA" => Some(ColOp::CountA),
                "PRODUCT" => Some(ColOp::Product),
                _ => None,
            }
        }
        _ => None,
    }
}

fn is_param(expr: &Expr, param: &str) -> bool {
    matches!(expr, Expr::Name(n) if names_eq(n, param))
}

/// Eta-reduced helper: a bare `SUM` / `AVERAGE` / … name.
pub fn eta_op(name: &str) -> Option<ColOp> {
    match fn_key(name).as_str() {
        "SUM" => Some(ColOp::Sum),
        "AVERAGE" => Some(ColOp::Average),
        "MIN" => Some(ColOp::Min),
        "MAX" => Some(ColOp::Max),
        "COUNT" => Some(ColOp::Count),
        "COUNTA" => Some(ColOp::CountA),
        "PRODUCT" => Some(ColOp::Product),
        _ => None,
    }
}

/// Production reduce: walk cells in place, no per-column allocation.
pub fn reduce_fast(rows: &[Vec<ExcelValue>], op: &ColOp) -> ExcelValue {
    match grid_dims(rows) {
        Ok((nrows, ncols)) => reduce_walk(rows, nrows, ncols, op),
        Err(e) => ExcelValue::Error(e),
    }
}

/// Allocation-heavy baseline: clone each column, then fold.
///
/// Same answers as [`reduce_fast`]. Used as the bench "before".
pub fn reduce_naive(rows: &[Vec<ExcelValue>], op: &ColOp) -> ExcelValue {
    let (nrows, ncols) = match grid_dims(rows) {
        Ok(d) => d,
        Err(e) => return ExcelValue::Error(e),
    };
    if let ColOp::Const(v) = op {
        return row_result(vec![v.clone(); ncols]);
    }
    let mut out = Vec::with_capacity(ncols);
    for c in 0..ncols {
        let mut col = Vec::with_capacity(nrows);
        for row in rows {
            col.push(row.get(c).cloned().unwrap_or(EMPTY));
        }
        out.push(fold_slice(&col, op, nrows == 1));
    }
    row_result(out)
}

fn reduce_walk(rows: &[Vec<ExcelValue>], nrows: usize, ncols: usize, op: &ColOp) -> ExcelValue {
    if let ColOp::Const(v) = op {
        return row_result(vec![v.clone(); ncols]);
    }
    let as_scalar = nrows == 1;
    let mut out = Vec::with_capacity(ncols);
    for c in 0..ncols {
        out.push(fold_column(rows, c, op, as_scalar));
    }
    row_result(out)
}

/// Range-like fold for a column (matches `SUM`/`AVERAGE`/… on an array).
///
/// `as_scalar` is `true` for a 1-row source: the cell is the whole
/// argument (`SUM(TRUE)` → 1). Multi-row columns use range skip rules
/// (`SUM` of a logical/text cell is 0 / skip).
struct Acc {
    sum: f64,
    product: f64,
    min: Option<f64>,
    max: Option<f64>,
    count: usize,
    counta: usize,
}

impl Acc {
    fn new(_op: &ColOp) -> Self {
        Self {
            sum: 0.0,
            product: 1.0,
            min: None,
            max: None,
            count: 0,
            counta: 0,
        }
    }

    fn add_number(&mut self, n: f64) {
        self.sum += n;
        self.product *= n;
        self.count += 1;
        self.min = Some(self.min.map_or(n, |m| m.min(n)));
        self.max = Some(self.max.map_or(n, |m| m.max(n)));
    }

    fn feed(&mut self, v: &ExcelValue, as_scalar: bool, op: &ColOp) -> Option<ExcelError> {
        if matches!(op, ColOp::CountA) {
            return self.feed_counta(v);
        }
        if matches!(op, ColOp::Count) {
            return self.feed_count(v, as_scalar);
        }
        match (v, as_scalar) {
            (ExcelValue::Error(e), _) => Some(*e),
            (ExcelValue::Number(n), _) => {
                self.add_number(*n);
                None
            }
            (ExcelValue::Empty, _) => None,
            (ExcelValue::Bool(b), true) => {
                self.add_number(if *b { 1.0 } else { 0.0 });
                None
            }
            (ExcelValue::Bool(_), false) => None,
            (ExcelValue::Text(s), true) => match super::coerce::parse_numeric_text(s) {
                Ok(n) => {
                    self.add_number(n);
                    None
                }
                Err(e) => Some(e),
            },
            (ExcelValue::Text(_), false) => None,
            (ExcelValue::Array(_), _) => Some(ExcelError::Value),
        }
    }

    fn feed_count(&mut self, v: &ExcelValue, as_scalar: bool) -> Option<ExcelError> {
        match (v, as_scalar) {
            (ExcelValue::Error(_), _) => None,
            (ExcelValue::Number(_), _) => {
                self.count += 1;
                None
            }
            (ExcelValue::Bool(_), true) => {
                self.count += 1;
                None
            }
            (ExcelValue::Text(s), true) => {
                if super::coerce::parse_numeric_text(s).is_ok() {
                    self.count += 1;
                }
                None
            }
            _ => None,
        }
    }

    fn feed_counta(&mut self, v: &ExcelValue) -> Option<ExcelError> {
        match v {
            ExcelValue::Empty => None,
            ExcelValue::Array(_) => Some(ExcelError::Value),
            _ => {
                self.counta += 1;
                None
            }
        }
    }

    fn finish(self, op: &ColOp) -> ExcelValue {
        match op {
            ColOp::Sum => ExcelValue::Number(self.sum),
            ColOp::Product => {
                ExcelValue::Number(if self.count == 0 { 0.0 } else { self.product })
            }
            ColOp::Average => {
                if self.count == 0 {
                    ExcelValue::Error(ExcelError::Div0)
                } else {
                    ExcelValue::Number(self.sum / self.count as f64)
                }
            }
            ColOp::Min => ExcelValue::Number(self.min.unwrap_or(0.0)),
            ColOp::Max => ExcelValue::Number(self.max.unwrap_or(0.0)),
            ColOp::Count => ExcelValue::Number(self.count as f64),
            ColOp::CountA => ExcelValue::Number(self.counta as f64),
            ColOp::Const(v) => v.clone(),
        }
    }
}

fn fold_column(rows: &[Vec<ExcelValue>], col: usize, op: &ColOp, as_scalar: bool) -> ExcelValue {
    let mut acc = Acc::new(op);
    for row in rows {
        let cell = row.get(col).unwrap_or(&EMPTY);
        if let Some(e) = acc.feed(cell, as_scalar, op) {
            return ExcelValue::Error(e);
        }
    }
    acc.finish(op)
}

fn fold_slice(col: &[ExcelValue], op: &ColOp, as_scalar: bool) -> ExcelValue {
    let mut acc = Acc::new(op);
    for cell in col {
        if let Some(e) = acc.feed(cell, as_scalar, op) {
            return ExcelValue::Error(e);
        }
    }
    acc.finish(op)
}

/// Evaluator entry.
pub(crate) fn eval(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() != 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let plan = match resolve_plan(&args[1], ctx) {
        Ok(p) => p,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    if let Expr::Range(range) = &args[0] {
        return eval_range(ev, range, &plan, ctx);
    }
    let array = ev.eval_expr(&args[0], ctx)?;
    if let ExcelValue::Error(e) = array {
        return Ok(ExcelValue::Error(e));
    }
    apply(ev, ctx, &array, &plan)
}

enum Plan {
    Fast(ColOp),
    Ast { param: String, body: Expr },
}

fn resolve_plan(expr: &Expr, ctx: &Ctx<'_>) -> Result<Plan, ExcelError> {
    if let Ok((params, body)) = resolve_lambda_n(expr, ctx, 1) {
        let param = params.into_iter().next().ok_or(ExcelError::Value)?;
        if let Some(op) = classify(&body, &param) {
            return Ok(Plan::Fast(op));
        }
        return Ok(Plan::Ast { param, body });
    }
    if let Expr::Name(n) = expr {
        if let Some(op) = eta_op(n) {
            return Ok(Plan::Fast(op));
        }
    }
    Err(ExcelError::Value)
}

fn eval_range(
    ev: &Evaluator,
    range: &xlsx_types::RangeRef,
    plan: &Plan,
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    let sheet_name = range
        .sheet
        .clone()
        .unwrap_or_else(|| ctx.current_sheet.clone());
    if ctx.spec.workbook.sheet(Some(&sheet_name)).is_err() {
        return Ok(ExcelValue::Error(ExcelError::Ref));
    }
    let nrows = range.row_count() as usize;
    let ncols = range.col_count() as usize;
    if nrows == 0 || ncols == 0 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    if ncols > MAX_COLS {
        return Ok(ExcelValue::Error(ExcelError::Num));
    }
    match plan {
        Plan::Fast(op) => {
            if let ColOp::Const(v) = op {
                return Ok(row_result(vec![v.clone(); ncols]));
            }
            let as_scalar = nrows == 1;
            let mut out = Vec::with_capacity(ncols);
            for c in 0..ncols {
                let mut acc = Acc::new(op);
                let mut col_err = None;
                for r in 0..nrows {
                    let addr = xlsx_types::CellAddr::new(
                        range.start.col + c as u32,
                        range.start.row + r as u32,
                    );
                    let v = ev.eval_cell(
                        &xlsx_types::CellRef {
                            sheet: Some(sheet_name.clone()),
                            addr,
                        },
                        ctx,
                    )?;
                    if let Some(e) = acc.feed(&v, as_scalar, op) {
                        col_err = Some(e);
                        break;
                    }
                }
                out.push(match col_err {
                    Some(e) => ExcelValue::Error(e),
                    None => acc.finish(op),
                });
            }
            Ok(row_result(out))
        }
        Plan::Ast { param, body } => {
            let array = ev.eval_range(range, ctx)?;
            apply_ast(ev, ctx, &array, param, body)
        }
    }
}

fn apply(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    array: &ExcelValue,
    plan: &Plan,
) -> Result<ExcelValue, EvalError> {
    match plan {
        Plan::Fast(op) => Ok(apply_fast(array, op)),
        Plan::Ast { param, body } => apply_ast(ev, ctx, array, param, body),
    }
}

fn apply_fast(array: &ExcelValue, op: &ColOp) -> ExcelValue {
    match array {
        ExcelValue::Error(e) => ExcelValue::Error(*e),
        ExcelValue::Array(rows) => reduce_fast(rows, op),
        other => {
            let rows = [vec![other.clone()]];
            reduce_fast(&rows, op)
        }
    }
}

fn apply_ast(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    array: &ExcelValue,
    param: &str,
    body: &Expr,
) -> Result<ExcelValue, EvalError> {
    let owned;
    let rows: &[Vec<ExcelValue>] = match array {
        ExcelValue::Error(e) => return Ok(ExcelValue::Error(*e)),
        ExcelValue::Array(rows) => {
            if let Err(e) = grid_dims(rows) {
                return Ok(ExcelValue::Error(e));
            }
            rows
        }
        other => {
            owned = vec![vec![other.clone()]];
            &owned
        }
    };
    let ncols = rows[0].len();
    let base = ctx.locals.len();
    ctx.locals.push((param.to_string(), EMPTY));
    let mut out = Vec::with_capacity(ncols);
    for c in 0..ncols {
        ctx.locals[base].1 = column_arg(rows, c);
        out.push(scalar_result(ev.eval_expr(body, ctx)?));
    }
    ctx.locals.truncate(base);
    Ok(row_result(out))
}

/// Always walk the AST (unit-test baseline / seed-shaped path).
#[cfg(test)]
pub(crate) fn apply_naive(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    array: &ExcelValue,
    param: &str,
    body: &Expr,
) -> Result<ExcelValue, EvalError> {
    apply_ast(ev, ctx, array, param, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::eval_formula_in;
    use crate::parse::parse;
    use xlsx_types::{DefinedName, Sheet, Workbook};

    fn n(x: f64) -> ExcelValue {
        ExcelValue::Number(x)
    }

    fn grid(v: Vec<Vec<f64>>) -> Vec<Vec<ExcelValue>> {
        v.into_iter()
            .map(|row| row.into_iter().map(n).collect())
            .collect()
    }

    #[test]
    fn classify_sum_of_param() {
        let body = parse("SUM(c)").unwrap();
        assert_eq!(classify(&body, "c"), Some(ColOp::Sum));
        assert_eq!(classify(&parse("sum(C)").unwrap(), "c"), Some(ColOp::Sum));
        assert_eq!(classify(&parse("SUM(x)").unwrap(), "c"), None);
        assert_eq!(classify(&parse("7").unwrap(), "c"), Some(ColOp::Const(n(7.0))));
    }

    #[test]
    fn fast_matches_naive_sum() {
        let rows = grid(vec![vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]]);
        let fast = reduce_fast(&rows, &ColOp::Sum);
        let naive = reduce_naive(&rows, &ColOp::Sum);
        assert_eq!(fast, naive);
        assert_eq!(fast, row_result(vec![n(5.0), n(7.0), n(9.0)]));
    }

    #[test]
    fn formula_sum_2x3() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=BYCOL({1,2,3;4,5,6},LAMBDA(c,SUM(c)))").unwrap(),
            row_result(vec![n(5.0), n(7.0), n(9.0)])
        );
    }

    #[test]
    fn formula_eta_sum() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=BYCOL({1,2,3;4,5,6},SUM)").unwrap(),
            row_result(vec![n(5.0), n(7.0), n(9.0)])
        );
    }

    #[test]
    fn one_row_identity_and_scale() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=BYCOL({1,2,3},LAMBDA(c,c))").unwrap(),
            row_result(vec![n(1.0), n(2.0), n(3.0)])
        );
        assert_eq!(
            eval_formula_in(&wb, "=BYCOL({1,2,3},LAMBDA(c,c*2))").unwrap(),
            row_result(vec![n(2.0), n(4.0), n(6.0)])
        );
    }

    #[test]
    fn multi_row_identity_is_calc() {
        let v = eval_formula_in(&Workbook::default(), "=BYCOL({1,2;3,4},LAMBDA(c,c))").unwrap();
        assert_eq!(
            v,
            row_result(vec![
                ExcelValue::Error(ExcelError::Calc),
                ExcelValue::Error(ExcelError::Calc)
            ])
        );
    }

    #[test]
    fn named_lambda() {
        let wb = Workbook {
            sheets: vec![Sheet::new("Sheet1")],
            names: vec![DefinedName {
                name: "ColSum".into(),
                refers_to: "=LAMBDA(c,SUM(c))".into(),
            }],
        };
        assert_eq!(
            eval_formula_in(&wb, "=BYCOL({1,2;3,4},ColSum)").unwrap(),
            row_result(vec![n(4.0), n(6.0)])
        );
    }

    #[test]
    fn body_error_stays_in_cell() {
        let v = eval_formula_in(
            &Workbook::default(),
            "=BYCOL({1,0,2},LAMBDA(c,1/c))",
        )
        .unwrap();
        assert_eq!(
            v,
            row_result(vec![n(1.0), ExcelValue::Error(ExcelError::Div0), n(0.5)])
        );
    }

    #[test]
    fn bad_lambda_is_value() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=BYCOL({1},LAMBDA(r,c,r))").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=BYCOL({1},1)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=BYCOL({1})").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn xlfn_prefix() {
        assert_eq!(
            eval_formula_in(
                &Workbook::default(),
                "=_xlfn.BYCOL({1,2},_xlfn.LAMBDA(c,c*10))"
            )
            .unwrap(),
            row_result(vec![n(10.0), n(20.0)])
        );
    }

    #[test]
    fn apply_naive_matches_if_body() {
        use crate::eval::Ctx;
        use crate::eval::Evaluator;
        use std::collections::HashSet;
        use xlsx_types::{CellAddr, EvalSpec, EvalTarget};

        let spec = EvalSpec {
            case_id: "naive".into(),
            workbook: Workbook::default(),
            target: EvalTarget::formula("=BYCOL({1,2;3,4},LAMBDA(c,IF(SUM(c)>4,1,0)))"),
            options: Default::default(),
        };
        let ev = Evaluator::new();
        let mut ctx = Ctx {
            spec: &spec,
            current_sheet: "Sheet1".into(),
            depth: 0,
            visiting: HashSet::new(),
            host: CellAddr::new(0, 0),
            locals: Vec::new(),
            rng: crate::eval::randarray::XorShift64::from_eval_options(&spec.options),
        };
        let array = ExcelValue::Array(grid(vec![vec![1.0, 2.0], vec![3.0, 4.0]]));
        let body = parse("IF(SUM(c)>4,1,0)").unwrap();
        let via_naive = apply_naive(&ev, &mut ctx, &array, "c", &body).unwrap();
        assert_eq!(via_naive, row_result(vec![n(0.0), n(1.0)]));
    }
}
