//! Excel `BYROW(array, LAMBDA(row, body))`.
//!
//! Applies a one-parameter LAMBDA to each row and returns an n×1 column of
//! results. The LAMBDA receives the whole row as a 1×w array value.
//!
//! Documented Excel quirks this module implements:
//!
//! - The LAMBDA must have **exactly one** name parameter (inline or a defined
//!   name that refers to one). Wrong arity / non-name parameters → `#VALUE!`
//!   ("Incorrect Parameters").
//! - Not providing a LAMBDA at all is `#CALC!`. A number, unknown name, or
//!   other expression that is not a LAMBDA follows that rule. An error
//!   produced while evaluating a non-LAMBDA second argument still surfaces
//!   (so `BYROW(A1:B2, 1/0)` is `#DIV/0!`).
//! - Eta reduction: a bare aggregator name (`SUM`, `AVERAGE`, `MIN`, `MAX`,
//!   `COUNT`, `COUNTA`, `COUNTBLANK`, `PRODUCT`) is treated as
//!   `LAMBDA(row, AGG(row))` — same as Excel 365's short form.
//! - The LAMBDA must return a **single** value. An array return is `#CALC!`
//!   in that result cell (nested arrays are not supported).
//! - A body error stays in that result cell; the rest of the column still
//!   computes.
//! - A scalar first argument is a 1×1 array. Jagged arrays are `#VALUE!`.
//!   A 0-row / 0-column array is `#CALC!`.
//!
//! ## Spill / model limits
//!
//! - The engine returns an [`ExcelValue::Array`]; it does **not** write a
//!   spill range. Occupied neighbors never produce `#SPILL!`.
//! - Bare `LAMBDA(...)` (not consumed by `BYROW` / `MAKEARRAY`) is `#CALC!`.
//! - Immediately-invoked `LAMBDA(...)(args)` is not parsed.
//! - Optional LAMBDA parameters and `LET` helpers are out of scope.
//! - Parameter names that tokenize as A1 refs are not supported.
//!
//! [`apply_fast`] specializes `SUM`/`AVERAGE`/`MIN`/`MAX`/… of the bound
//! row so a row-sum over a numeric grid does not walk the AST per row.
//! [`apply_naive`] evaluates the same [`RowPlan`] through a HashMap copy of
//! each row — same answers, more allocation. Used as the bench "before".

use super::makearray::{
    fn_key, names_eq, resolve_lambda_arity, LambdaError,
};
use super::{Ctx, Evaluator};
use crate::ast::Expr;
use std::collections::HashMap;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

/// Aggregator recognized as an eta-reduced BYROW function / LAMBDA body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowAgg {
    Sum,
    Product,
    Average,
    Min,
    Max,
    Count,
    CountA,
    CountBlank,
}

/// A LAMBDA body (or eta name) that can run without walking the AST.
#[derive(Clone, Debug, PartialEq)]
pub enum RowPlan {
    Agg(RowAgg),
    Const(ExcelValue),
    /// 1-based `INDEX(row, n)` on a single-row array.
    Index(usize),
}

pub fn eta_agg(name: &str) -> Option<RowAgg> {
    match fn_key(name).as_str() {
        "SUM" => Some(RowAgg::Sum),
        "PRODUCT" => Some(RowAgg::Product),
        "AVERAGE" => Some(RowAgg::Average),
        "MIN" => Some(RowAgg::Min),
        "MAX" => Some(RowAgg::Max),
        "COUNT" => Some(RowAgg::Count),
        "COUNTA" => Some(RowAgg::CountA),
        "COUNTBLANK" => Some(RowAgg::CountBlank),
        _ => None,
    }
}

/// Rectangularize a BYROW source. Scalars become 1×1.
pub fn to_grid(v: ExcelValue) -> Result<Vec<Vec<ExcelValue>>, ExcelError> {
    match v {
        ExcelValue::Array(rows) => {
            if rows.is_empty() || rows[0].is_empty() {
                return Err(ExcelError::Calc);
            }
            let cols = rows[0].len();
            if rows.iter().any(|r| r.is_empty() || r.len() != cols) {
                return Err(ExcelError::Value);
            }
            Ok(rows)
        }
        other => Ok(vec![vec![other]]),
    }
}

fn scalar_cell(v: ExcelValue) -> ExcelValue {
    match v {
        ExcelValue::Array(_) => ExcelValue::Error(ExcelError::Calc),
        other => other,
    }
}

/// Classify a one-parameter body that only names the bound row.
pub fn classify(body: &Expr, row_param: &str) -> Option<RowPlan> {
    match body {
        Expr::Number(n) => Some(RowPlan::Const(ExcelValue::Number(*n))),
        Expr::Text(s) => Some(RowPlan::Const(ExcelValue::Text(s.clone()))),
        Expr::Bool(b) => Some(RowPlan::Const(ExcelValue::Bool(*b))),
        Expr::Error(e) => Some(RowPlan::Const(ExcelValue::Error(*e))),
        Expr::Call { name, args } if args.len() == 1 => {
            let agg = eta_agg(name)?;
            match &args[0] {
                Expr::Name(n) if names_eq(n, row_param) => Some(RowPlan::Agg(agg)),
                _ => None,
            }
        }
        Expr::Call { name, args } if fn_key(name) == "INDEX" && args.len() == 2 => {
            let Expr::Name(n) = &args[0] else {
                return None;
            };
            if !names_eq(n, row_param) {
                return None;
            }
            let Expr::Number(idx) = &args[1] else {
                return None;
            };
            if !idx.is_finite() {
                return None;
            }
            let t = idx.trunc();
            if t < 1.0 {
                return None;
            }
            Some(RowPlan::Index(t as usize))
        }
        _ => None,
    }
}

/// Production fill: specialized aggregators / constants / INDEX.
pub fn apply_fast(grid: &[Vec<ExcelValue>], plan: &RowPlan) -> ExcelValue {
    match plan {
        RowPlan::Const(v) => ExcelValue::Array(vec![vec![v.clone()]; grid.len()]),
        RowPlan::Agg(kind) => {
            let mut out = Vec::with_capacity(grid.len());
            for row in grid {
                out.push(vec![agg_row(row, *kind)]);
            }
            ExcelValue::Array(out)
        }
        RowPlan::Index(i) => {
            let mut out = Vec::with_capacity(grid.len());
            for row in grid {
                out.push(vec![index_row(row, *i)]);
            }
            ExcelValue::Array(out)
        }
    }
}

/// Allocation-heavy baseline: HashMap copy of every row, then the same plan.
pub fn apply_naive(grid: &[Vec<ExcelValue>], plan: &RowPlan) -> ExcelValue {
    let mut out = Vec::new();
    for row in grid {
        let mut env: HashMap<usize, ExcelValue> = HashMap::with_capacity(row.len());
        for (i, c) in row.iter().enumerate() {
            env.insert(i, c.clone());
        }
        let cells: Vec<ExcelValue> = (0..row.len())
            .map(|i| env.get(&i).cloned().unwrap_or(ExcelValue::Empty))
            .collect();
        let v = match plan {
            RowPlan::Const(c) => c.clone(),
            RowPlan::Agg(kind) => agg_row(&cells, *kind),
            RowPlan::Index(i) => index_row(&cells, *i),
        };
        out.push(vec![v]);
    }
    ExcelValue::Array(out)
}

fn index_row(row: &[ExcelValue], idx: usize) -> ExcelValue {
    if idx < 1 || idx > row.len() {
        ExcelValue::Error(ExcelError::Ref)
    } else {
        row[idx - 1].clone()
    }
}

/// Excel range-style aggregator over one row (skip text/logicals; errors win).
pub fn agg_row(row: &[ExcelValue], kind: RowAgg) -> ExcelValue {
    if kind == RowAgg::Sum {
        if let Some(s) = sum_packed(row) {
            return ExcelValue::Number(s);
        }
    }
    let mut acc = AggAcc::new(kind);
    for c in row {
        if let Some(e) = acc.fold(c) {
            return ExcelValue::Error(e);
        }
    }
    acc.finish()
}

fn sum_packed(row: &[ExcelValue]) -> Option<f64> {
    let mut sum = 0.0;
    for c in row {
        match c {
            ExcelValue::Number(n) => sum += *n,
            ExcelValue::Empty => {}
            _ => return None,
        }
    }
    Some(sum)
}

struct AggAcc {
    kind: RowAgg,
    sum: f64,
    product: f64,
    min: Option<f64>,
    max: Option<f64>,
    count: usize,
    counta: usize,
    countblank: usize,
}

impl AggAcc {
    fn new(kind: RowAgg) -> Self {
        Self {
            kind,
            sum: 0.0,
            product: 1.0,
            min: None,
            max: None,
            count: 0,
            counta: 0,
            countblank: 0,
        }
    }

    fn fold(&mut self, v: &ExcelValue) -> Option<ExcelError> {
        // Cells of a BYROW row are range-like (skip text / logicals).
        match v {
            ExcelValue::Array(rows) => {
                for row in rows {
                    for c in row {
                        if let Some(e) = self.fold(c) {
                            return Some(e);
                        }
                    }
                }
                None
            }
            ExcelValue::Error(e) => match self.kind {
                RowAgg::CountA => {
                    self.counta += 1;
                    None
                }
                RowAgg::Count | RowAgg::CountBlank => None,
                _ => Some(*e),
            },
            ExcelValue::Number(n) => {
                self.add_number(*n);
                self.counta += 1;
                None
            }
            ExcelValue::Empty => {
                self.countblank += 1;
                None
            }
            ExcelValue::Bool(_) => {
                if matches!(self.kind, RowAgg::CountA) {
                    self.counta += 1;
                }
                None
            }
            ExcelValue::Text(s) => {
                match self.kind {
                    RowAgg::CountA => self.counta += 1,
                    RowAgg::CountBlank if s.is_empty() => self.countblank += 1,
                    _ => {}
                }
                None
            }
        }
    }

    fn add_number(&mut self, n: f64) {
        self.sum += n;
        self.product *= n;
        self.count += 1;
        self.min = Some(self.min.map_or(n, |m| m.min(n)));
        self.max = Some(self.max.map_or(n, |m| m.max(n)));
    }

    fn finish(self) -> ExcelValue {
        match self.kind {
            RowAgg::Sum => ExcelValue::Number(self.sum),
            RowAgg::Product => ExcelValue::Number(if self.count == 0 { 0.0 } else { self.product }),
            RowAgg::Average => {
                if self.count == 0 {
                    ExcelValue::Error(ExcelError::Div0)
                } else {
                    ExcelValue::Number(self.sum / self.count as f64)
                }
            }
            RowAgg::Min => ExcelValue::Number(self.min.unwrap_or(0.0)),
            RowAgg::Max => ExcelValue::Number(self.max.unwrap_or(0.0)),
            RowAgg::Count => ExcelValue::Number(self.count as f64),
            RowAgg::CountA => ExcelValue::Number(self.counta as f64),
            RowAgg::CountBlank => ExcelValue::Number(self.countblank as f64),
        }
    }
}

/// Evaluator entry: fast kernel when the body is a row aggregator / const /
/// INDEX, else AST + locals.
pub(crate) fn apply(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    grid: &[Vec<ExcelValue>],
    row_param: &str,
    body: &Expr,
) -> Result<ExcelValue, EvalError> {
    if let Some(plan) = classify(body, row_param) {
        return Ok(apply_fast(grid, &plan));
    }
    apply_naive_eval(ev, ctx, grid, row_param, body)
}

/// Always walk the AST (bench baseline / seed-compliant-shaped path).
pub(crate) fn apply_naive_eval(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    grid: &[Vec<ExcelValue>],
    row_param: &str,
    body: &Expr,
) -> Result<ExcelValue, EvalError> {
    apply_general(ev, ctx, grid, row_param, body)
}

fn apply_general(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    grid: &[Vec<ExcelValue>],
    row_param: &str,
    body: &Expr,
) -> Result<ExcelValue, EvalError> {
    let base = ctx.locals.len();
    ctx.locals
        .push((row_param.to_string(), ExcelValue::Array(vec![grid[0].clone()])));
    let mut out = Vec::with_capacity(grid.len());
    for row in grid {
        ctx.locals[base].1 = ExcelValue::Array(vec![row.clone()]);
        out.push(vec![scalar_cell(ev.eval_expr(body, ctx)?)]);
    }
    ctx.locals.truncate(base);
    Ok(ExcelValue::Array(out))
}

/// Bind a LAMBDA (or eta name) and evaluate BYROW.
pub(crate) fn eval(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() != 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let array = ev.eval_expr(&args[0], ctx)?;
    if let ExcelValue::Error(e) = array {
        return Ok(ExcelValue::Error(e));
    }
    let grid = match to_grid(array) {
        Ok(g) => g,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };

    match resolve_lambda_arity(&args[1], ctx, 1) {
        Ok((params, body)) => apply(ev, ctx, &grid, &params[0], &body),
        Err(LambdaError::WrongArity) => Ok(ExcelValue::Error(ExcelError::Value)),
        Err(LambdaError::NotLambda) => {
            if let Expr::Name(n) = &args[1] {
                if let Some(kind) = eta_agg(n) {
                    return Ok(apply_fast(&grid, &RowPlan::Agg(kind)));
                }
            }
            let second = ev.eval_expr(&args[1], ctx)?;
            if let ExcelValue::Error(e) = second {
                return Ok(ExcelValue::Error(e));
            }
            Ok(ExcelValue::Error(ExcelError::Calc))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::eval_formula_in;
    use crate::parse::parse;
    use xlsx_types::{Cell, DefinedName, Sheet, Workbook};

    fn n(x: f64) -> ExcelValue {
        ExcelValue::Number(x)
    }

    fn grid2() -> Vec<Vec<ExcelValue>> {
        vec![vec![n(1.0), n(2.0), n(3.0)], vec![n(4.0), n(5.0), n(6.0)]]
    }

    fn sheet_a1_c2() -> Workbook {
        let mut sheet = Sheet::new("Sheet1");
        sheet.cells.insert("A1".into(), Cell::value(n(1.0)));
        sheet.cells.insert("B1".into(), Cell::value(n(2.0)));
        sheet.cells.insert("C1".into(), Cell::value(n(3.0)));
        sheet.cells.insert("A2".into(), Cell::value(n(4.0)));
        sheet.cells.insert("B2".into(), Cell::value(n(5.0)));
        sheet.cells.insert("C2".into(), Cell::value(n(6.0)));
        Workbook {
            sheets: vec![sheet],
            names: vec![],
        }
    }

    #[test]
    fn fast_matches_naive_sum() {
        let plan = RowPlan::Agg(RowAgg::Sum);
        let g = grid2();
        assert_eq!(apply_fast(&g, &plan), apply_naive(&g, &plan));
        assert_eq!(
            apply_fast(&g, &plan),
            ExcelValue::Array(vec![vec![n(6.0)], vec![n(15.0)]])
        );
    }

    #[test]
    fn classify_sum_and_index() {
        assert_eq!(
            classify(&parse("SUM(row)").unwrap(), "row"),
            Some(RowPlan::Agg(RowAgg::Sum))
        );
        assert_eq!(
            classify(&parse("INDEX(r,2)").unwrap(), "r"),
            Some(RowPlan::Index(2))
        );
        assert_eq!(classify(&parse("7").unwrap(), "r"), Some(RowPlan::Const(n(7.0))));
        assert_eq!(classify(&parse("SUM(r)+1").unwrap(), "r"), None);
    }

    #[test]
    fn formula_sum_range() {
        let wb = sheet_a1_c2();
        assert_eq!(
            eval_formula_in(&wb, "=BYROW(A1:C2,LAMBDA(row,SUM(row)))").unwrap(),
            ExcelValue::Array(vec![vec![n(6.0)], vec![n(15.0)]])
        );
    }

    #[test]
    fn formula_eta_sum() {
        let wb = sheet_a1_c2();
        assert_eq!(
            eval_formula_in(&wb, "=BYROW(A1:C2,SUM)").unwrap(),
            ExcelValue::Array(vec![vec![n(6.0)], vec![n(15.0)]])
        );
    }

    #[test]
    fn formula_max_microsoft() {
        let wb = sheet_a1_c2();
        assert_eq!(
            eval_formula_in(&wb, "=BYROW(A1:C2,LAMBDA(array,MAX(array)))").unwrap(),
            ExcelValue::Array(vec![vec![n(3.0)], vec![n(6.0)]])
        );
    }

    #[test]
    fn formula_named_lambda() {
        let mut wb = sheet_a1_c2();
        wb.names.push(DefinedName {
            name: "RowSum".into(),
            refers_to: "=LAMBDA(r,SUM(r))".into(),
        });
        assert_eq!(
            eval_formula_in(&wb, "=BYROW(A1:C2,RowSum)").unwrap(),
            ExcelValue::Array(vec![vec![n(6.0)], vec![n(15.0)]])
        );
    }

    #[test]
    fn body_array_is_calc() {
        assert_eq!(
            eval_formula_in(&Workbook::default(), "=BYROW({1,2;3,4},LAMBDA(r,r))").unwrap(),
            ExcelValue::Array(vec![
                vec![ExcelValue::Error(ExcelError::Calc)],
                vec![ExcelValue::Error(ExcelError::Calc)],
            ])
        );
    }

    #[test]
    fn bad_lambda_is_value_or_calc() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=BYROW({1,2},LAMBDA(a,b,a))").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=BYROW({1,2},1)").unwrap(),
            ExcelValue::Error(ExcelError::Calc)
        );
        assert_eq!(
            eval_formula_in(&wb, "=BYROW({1,2})").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn xlfn_prefix() {
        let wb = sheet_a1_c2();
        assert_eq!(
            eval_formula_in(&wb, "=_xlfn.BYROW(A1:C2,_xlfn.LAMBDA(r,SUM(r)))").unwrap(),
            ExcelValue::Array(vec![vec![n(6.0)], vec![n(15.0)]])
        );
    }

    #[test]
    fn apply_naive_eval_matches_if_body() {
        use crate::eval::Ctx;
        use crate::eval::Evaluator;
        use std::collections::HashSet;
        use xlsx_types::{CellAddr, EvalSpec, EvalTarget};

        let spec = EvalSpec {
            case_id: "naive".into(),
            workbook: Workbook::default(),
            target: EvalTarget::formula("=BYROW({1,2;10,1},LAMBDA(r,IF(SUM(r)>5,1,0)))"),
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
        let body = parse("IF(SUM(r)>5,1,0)").unwrap();
        let grid = vec![vec![n(1.0), n(2.0)], vec![n(10.0), n(1.0)]];
        let via_naive = apply_naive_eval(&ev, &mut ctx, &grid, "r", &body).unwrap();
        assert_eq!(
            via_naive,
            ExcelValue::Array(vec![vec![n(0.0)], vec![n(1.0)]])
        );
    }
}
