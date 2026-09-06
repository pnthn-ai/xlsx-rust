//! Excel `SCAN([initial_value], array, LAMBDA(acc, value, body))`.
//!
//! Walks `array` in **row-major** order (left-to-right, then down) and
//! returns an array of the same shape whose every cell is the accumulator
//! after that step.
//!
//! Documented Excel behavior this module implements:
//!
//! - `initial_value` is optional. Omitted (`SCAN(array, lambda)` or
//!   `SCAN(, array, lambda)`): the first output cell is the first array
//!   element and the LAMBDA is applied from the second element. A supplied
//!   initial (including a blank cell, which is [`ExcelValue::Empty`], not
//!   “omitted”) is bound as `acc` on the first call.
//! - The LAMBDA must have **exactly two** name parameters (inline or a
//!   defined name that refers to one). Anything else is `#VALUE!`.
//! - A body error stays in that cell and becomes the next `acc`. A body
//!   that returns an array is `#CALC!` in that cell (same rule as
//!   `MAKEARRAY` / `BYROW`).
//! - A scalar `array` argument is a 1×1 result. An empty array is `#CALC!`.
//! - `initial_value` is a **scalar** (top-left of an array via
//!   [`super::coerce::scalarize`]). Array-valued accumulators are out of
//!   scope.
//!
//! ## Spill / model limits
//!
//! - The engine returns an [`ExcelValue::Array`]; it does **not** write a
//!   spill range. Occupied neighbors never produce `#SPILL!`.
//! - Bare `LAMBDA(...)` (not consumed by `SCAN` / `MAKEARRAY`) is `#CALC!`.
//! - Immediately-invoked `LAMBDA(...)(args)` is not parsed.
//! - Native functions used *as* the LAMBDA (`SCAN(0, A1:A3, SUM)`) are
//!   `#VALUE!` — only an inline / named `LAMBDA` is bound.
//! - Parameter names that tokenize as A1 refs are not supported.
//! - Optional LAMBDA parameters and `LET` helpers are out of scope.
//!
//! [`scan_fast`] specializes running `acc+value` / `acc*value` / `acc&value`
//! so a prefix sum does not walk the AST per cell. [`scan_naive`] evaluates
//! the same [`FastScan`] through a fresh HashMap binding on every step —
//! same answers, more allocation. Used as the bench "before".

use super::makearray::{names_eq, FastOp, Local};
use super::{excel_pow, Ctx, Evaluator};
use crate::ast::{BinOp, Expr, UnaryOp};
use std::collections::HashMap;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

/// A LAMBDA body that depends only on the two SCAN parameters and literals.
#[derive(Clone, Debug, PartialEq)]
pub enum FastScan {
    Const(ExcelValue),
    Acc,
    Val,
    Neg(Box<FastScan>),
    Op {
        op: FastOp,
        left: Box<FastScan>,
        right: Box<FastScan>,
    },
    Concat(Box<FastScan>, Box<FastScan>),
}

/// Turn a scanned argument into a rectangular row-major grid.
///
/// Scalars become 1×1. A scalar error is returned as `Err` so the whole
/// `SCAN` fails. An empty / zero-width array is `#CALC!`.
pub fn matrix(v: ExcelValue) -> Result<Vec<Vec<ExcelValue>>, ExcelError> {
    match v {
        ExcelValue::Error(e) => Err(e),
        ExcelValue::Array(rows) => {
            if rows.is_empty() || rows.iter().any(|r| r.is_empty()) {
                return Err(ExcelError::Calc);
            }
            let width = rows[0].len();
            if rows.iter().any(|r| r.len() != width) {
                return Err(ExcelError::Value);
            }
            Ok(rows)
        }
        other => Ok(vec![vec![other]]),
    }
}

/// Classify a body that only names the accumulator / value parameters.
pub fn classify(body: &Expr, acc_param: &str, val_param: &str) -> Option<FastScan> {
    match body {
        Expr::Number(n) => Some(FastScan::Const(ExcelValue::Number(*n))),
        Expr::Text(s) => Some(FastScan::Const(ExcelValue::Text(s.clone()))),
        Expr::Bool(b) => Some(FastScan::Const(ExcelValue::Bool(*b))),
        Expr::Error(e) => Some(FastScan::Const(ExcelValue::Error(*e))),
        Expr::Name(n) if names_eq(n, acc_param) => Some(FastScan::Acc),
        Expr::Name(n) if names_eq(n, val_param) => Some(FastScan::Val),
        Expr::Unary {
            op: UnaryOp::Minus,
            expr,
        } => classify(expr, acc_param, val_param).map(|e| FastScan::Neg(Box::new(e))),
        Expr::Unary {
            op: UnaryOp::Plus,
            expr,
        } => classify(expr, acc_param, val_param),
        Expr::Binary {
            op: BinOp::Concat,
            left,
            right,
        } => Some(FastScan::Concat(
            Box::new(classify(left, acc_param, val_param)?),
            Box::new(classify(right, acc_param, val_param)?),
        )),
        Expr::Binary { op, left, right } => {
            let fop = match op {
                BinOp::Add => FastOp::Add,
                BinOp::Sub => FastOp::Sub,
                BinOp::Mul => FastOp::Mul,
                BinOp::Div => FastOp::Div,
                BinOp::Pow => FastOp::Pow,
                _ => return None,
            };
            Some(FastScan::Op {
                op: fop,
                left: Box::new(classify(left, acc_param, val_param)?),
                right: Box::new(classify(right, acc_param, val_param)?),
            })
        }
        _ => None,
    }
}

/// Production fill: specialized loops for running sum / product / concat.
pub fn scan_fast(
    initial: Option<&ExcelValue>,
    grid: &[Vec<ExcelValue>],
    body: &FastScan,
) -> ExcelValue {
    match classify_kernel(body) {
        Some(Kernel::Add) => running_add(initial, grid),
        Some(Kernel::Mul) => running_mul(initial, grid),
        Some(Kernel::Concat) => running_concat(initial, grid),
        None => scan_walk(initial, grid, body),
    }
}

/// Allocation-heavy baseline: HashMap bind + walk on every step.
pub fn scan_naive(
    initial: Option<&ExcelValue>,
    grid: &[Vec<ExcelValue>],
    body: &FastScan,
) -> ExcelValue {
    if grid.is_empty() || grid[0].is_empty() {
        return ExcelValue::Error(ExcelError::Calc);
    }
    let rows = grid.len();
    let cols = grid[0].len();
    let mut out: Vec<Vec<ExcelValue>> = Vec::new();
    let mut acc: Option<ExcelValue> = initial.cloned();
    let omit_first = initial.is_none();
    let mut first = true;
    for r in 0..rows {
        let mut row = Vec::new();
        for c in 0..cols {
            let val = &grid[r][c];
            let next = if omit_first && first {
                val.clone()
            } else {
                let a = acc.clone().unwrap_or(ExcelValue::Empty);
                let mut env = HashMap::with_capacity(2);
                env.insert("a", a);
                env.insert("v", val.clone());
                eval_fast_env(body, &env)
            };
            first = false;
            acc = Some(next.clone());
            row.push(next);
        }
        out.push(row);
    }
    ExcelValue::Array(out)
}

#[derive(Clone, Copy)]
enum Kernel {
    Add,
    Mul,
    Concat,
}

fn classify_kernel(body: &FastScan) -> Option<Kernel> {
    match body {
        FastScan::Op {
            op: FastOp::Add,
            left,
            right,
        } if is_acc_val_pair(left, right) => Some(Kernel::Add),
        FastScan::Op {
            op: FastOp::Mul,
            left,
            right,
        } if is_acc_val_pair(left, right) => Some(Kernel::Mul),
        FastScan::Concat(left, right)
            if matches!(**left, FastScan::Acc) && matches!(**right, FastScan::Val) =>
        {
            Some(Kernel::Concat)
        }
        _ => None,
    }
}

fn is_acc_val_pair(left: &FastScan, right: &FastScan) -> bool {
    matches!(
        (left, right),
        (FastScan::Acc, FastScan::Val) | (FastScan::Val, FastScan::Acc)
    )
}

fn running_add(initial: Option<&ExcelValue>, grid: &[Vec<ExcelValue>]) -> ExcelValue {
    running_num(initial, grid, |a, v| Ok(a + v))
}

fn running_mul(initial: Option<&ExcelValue>, grid: &[Vec<ExcelValue>]) -> ExcelValue {
    running_num(initial, grid, |a, v| Ok(a * v))
}

fn running_num(
    initial: Option<&ExcelValue>,
    grid: &[Vec<ExcelValue>],
    f: impl Fn(f64, f64) -> Result<f64, ExcelError>,
) -> ExcelValue {
    if grid.is_empty() || grid[0].is_empty() {
        return ExcelValue::Error(ExcelError::Calc);
    }
    let rows = grid.len();
    let cols = grid[0].len();
    let mut out = Vec::with_capacity(rows);
    let mut acc: Option<Result<f64, ExcelError>> = match initial {
        Some(v) => Some(super::coerce::to_number(v).or_else(|e| {
            if let ExcelValue::Error(err) = v {
                Err(*err)
            } else {
                Err(e)
            }
        })),
        None => None,
    };
    let omit_first = initial.is_none();
    let mut first = true;
    for r in 0..rows {
        let mut row = Vec::with_capacity(cols);
        for c in 0..cols {
            let val = &grid[r][c];
            let next = if omit_first && first {
                match val {
                    ExcelValue::Error(e) => Err(*e),
                    other => super::coerce::to_number(other),
                }
            } else {
                match acc {
                    Some(Ok(a)) => match val {
                        ExcelValue::Error(e) => Err(*e),
                        other => match super::coerce::to_number(other) {
                            Ok(v) => f(a, v),
                            Err(e) => Err(e),
                        },
                    },
                    Some(Err(e)) => Err(e),
                    None => super::coerce::to_number(val),
                }
            };
            first = false;
            acc = Some(next);
            row.push(match next {
                Ok(n) => ExcelValue::Number(n),
                Err(e) => ExcelValue::Error(e),
            });
        }
        out.push(row);
    }
    ExcelValue::Array(out)
}

fn running_concat(initial: Option<&ExcelValue>, grid: &[Vec<ExcelValue>]) -> ExcelValue {
    if grid.is_empty() || grid[0].is_empty() {
        return ExcelValue::Error(ExcelError::Calc);
    }
    let rows = grid.len();
    let cols = grid[0].len();
    let mut out = Vec::with_capacity(rows);
    let mut acc: Option<Result<String, ExcelError>> = match initial {
        Some(v) => Some(super::coerce::to_text(v)),
        None => None,
    };
    let omit_first = initial.is_none();
    let mut first = true;
    for r in 0..rows {
        let mut row = Vec::with_capacity(cols);
        for c in 0..cols {
            let val = &grid[r][c];
            let next = if omit_first && first {
                super::coerce::to_text(val)
            } else {
                match acc {
                    Some(Ok(ref a)) => match super::coerce::to_text(val) {
                        Ok(v) => {
                            let mut s = String::with_capacity(a.len() + v.len());
                            s.push_str(a);
                            s.push_str(&v);
                            Ok(s)
                        }
                        Err(e) => Err(e),
                    },
                    Some(Err(e)) => Err(e),
                    None => super::coerce::to_text(val),
                }
            };
            first = false;
            let cell = match &next {
                Ok(s) => ExcelValue::Text(s.clone()),
                Err(e) => ExcelValue::Error(*e),
            };
            acc = Some(next);
            row.push(cell);
        }
        out.push(row);
    }
    ExcelValue::Array(out)
}

fn scan_walk(
    initial: Option<&ExcelValue>,
    grid: &[Vec<ExcelValue>],
    body: &FastScan,
) -> ExcelValue {
    if grid.is_empty() || grid[0].is_empty() {
        return ExcelValue::Error(ExcelError::Calc);
    }
    let rows = grid.len();
    let cols = grid[0].len();
    let mut out = Vec::with_capacity(rows);
    let mut acc: Option<ExcelValue> = initial.cloned();
    let omit_first = initial.is_none();
    let mut first = true;
    for r in 0..rows {
        let mut row = Vec::with_capacity(cols);
        for c in 0..cols {
            let val = &grid[r][c];
            let next = if omit_first && first {
                val.clone()
            } else {
                eval_fast(body, acc.as_ref().unwrap_or(&ExcelValue::Empty), val)
            };
            first = false;
            acc = Some(next.clone());
            row.push(next);
        }
        out.push(row);
    }
    ExcelValue::Array(out)
}

fn eval_fast(body: &FastScan, acc: &ExcelValue, val: &ExcelValue) -> ExcelValue {
    match body {
        FastScan::Const(v) => v.clone(),
        FastScan::Acc => acc.clone(),
        FastScan::Val => val.clone(),
        FastScan::Neg(inner) => match eval_fast(inner, acc, val) {
            ExcelValue::Error(e) => ExcelValue::Error(e),
            v => match super::coerce::to_number(&v) {
                Ok(n) => ExcelValue::Number(-n),
                Err(e) => ExcelValue::Error(e),
            },
        },
        FastScan::Op { op, left, right } => {
            let l = eval_fast(left, acc, val);
            if let ExcelValue::Error(e) = l {
                return ExcelValue::Error(e);
            }
            let r = eval_fast(right, acc, val);
            if let ExcelValue::Error(e) = r {
                return ExcelValue::Error(e);
            }
            apply_op(*op, &l, &r)
        }
        FastScan::Concat(left, right) => {
            let l = eval_fast(left, acc, val);
            if let ExcelValue::Error(e) = l {
                return ExcelValue::Error(e);
            }
            let r = eval_fast(right, acc, val);
            if let ExcelValue::Error(e) = r {
                return ExcelValue::Error(e);
            }
            match (super::coerce::to_text(&l), super::coerce::to_text(&r)) {
                (Ok(a), Ok(b)) => ExcelValue::Text(a + &b),
                (Err(e), _) | (_, Err(e)) => ExcelValue::Error(e),
            }
        }
    }
}

fn eval_fast_env(body: &FastScan, env: &HashMap<&str, ExcelValue>) -> ExcelValue {
    let a = env.get("a").cloned().unwrap_or(ExcelValue::Empty);
    let v = env.get("v").cloned().unwrap_or(ExcelValue::Empty);
    eval_fast(body, &a, &v)
}

fn apply_op(op: FastOp, left: &ExcelValue, right: &ExcelValue) -> ExcelValue {
    match op {
        FastOp::Add => num2(left, right, |a, b| ExcelValue::Number(a + b)),
        FastOp::Sub => num2(left, right, |a, b| ExcelValue::Number(a - b)),
        FastOp::Mul => num2(left, right, |a, b| ExcelValue::Number(a * b)),
        FastOp::Div => match (
            super::coerce::to_number(left),
            super::coerce::to_number(right),
        ) {
            (Err(e), _) | (_, Err(e)) => ExcelValue::Error(e),
            (Ok(_), Ok(d)) if d == 0.0 => ExcelValue::Error(ExcelError::Div0),
            (Ok(n), Ok(d)) => ExcelValue::Number(n / d),
        },
        FastOp::Pow => excel_pow(left, right),
    }
}

fn num2(left: &ExcelValue, right: &ExcelValue, f: impl Fn(f64, f64) -> ExcelValue) -> ExcelValue {
    match (
        super::coerce::to_number(left),
        super::coerce::to_number(right),
    ) {
        (Ok(a), Ok(b)) => f(a, b),
        (Err(e), _) | (_, Err(e)) => ExcelValue::Error(e),
    }
}

fn scalar_cell(v: ExcelValue) -> ExcelValue {
    match v {
        ExcelValue::Array(_) => ExcelValue::Error(ExcelError::Calc),
        other => other,
    }
}

/// Evaluator entry: fast kernel when the body is acc/value-only, else AST.
pub(crate) fn apply(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    initial: Option<&ExcelValue>,
    grid: &[Vec<ExcelValue>],
    acc_param: &str,
    val_param: &str,
    body: &Expr,
) -> Result<ExcelValue, EvalError> {
    if let Some(fast) = classify(body, acc_param, val_param) {
        return Ok(scan_fast(initial, grid, &fast));
    }
    apply_naive(ev, ctx, initial, grid, acc_param, val_param, body)
}

/// Always walk the AST (bench baseline / seed-compliant-shaped path).
pub(crate) fn apply_naive(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    initial: Option<&ExcelValue>,
    grid: &[Vec<ExcelValue>],
    acc_param: &str,
    val_param: &str,
    body: &Expr,
) -> Result<ExcelValue, EvalError> {
    if grid.is_empty() || grid[0].is_empty() {
        return Ok(ExcelValue::Error(ExcelError::Calc));
    }
    let rows = grid.len();
    let cols = grid[0].len();
    let base = ctx.locals.len();
    ctx.locals
        .push(Local::provided(acc_param.to_string(), ExcelValue::Empty));
    ctx.locals
        .push(Local::provided(val_param.to_string(), ExcelValue::Empty));
    let mut out = Vec::with_capacity(rows);
    let mut acc: Option<ExcelValue> = initial.cloned();
    let omit_first = initial.is_none();
    let mut first = true;
    for r in 0..rows {
        let mut row = Vec::with_capacity(cols);
        for c in 0..cols {
            let val = grid[r][c].clone();
            let next = if omit_first && first {
                val
            } else {
                ctx.locals[base].value = acc.clone().unwrap_or(ExcelValue::Empty);
                ctx.locals[base + 1].value = val;
                scalar_cell(ev.eval_expr(body, ctx)?)
            };
            first = false;
            acc = Some(next.clone());
            row.push(next);
        }
        out.push(row);
    }
    ctx.locals.truncate(base);
    Ok(ExcelValue::Array(out))
}

/// `SCAN([initial], array, LAMBDA(acc, value, body))` evaluator entry.
pub(crate) fn eval(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    let (initial_expr, array_expr, lambda_expr) = match args.len() {
        2 => (None, &args[0], &args[1]),
        3 if args[0].is_omitted() => (None, &args[1], &args[2]),
        3 => (Some(&args[0]), &args[1], &args[2]),
        _ => return Ok(ExcelValue::Error(ExcelError::Value)),
    };

    let initial = if let Some(expr) = initial_expr {
        let v = ev.eval_scalar(expr, ctx)?;
        if let ExcelValue::Error(e) = v {
            return Ok(ExcelValue::Error(e));
        }
        Some(v)
    } else {
        None
    };

    let array = ev.eval_expr(array_expr, ctx)?;
    let grid = match matrix(array) {
        Ok(g) => g,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };

    let (acc_p, val_p, body) = match super::makearray::resolve_lambda(lambda_expr, ctx) {
        Ok(l) => l,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    apply(ev, ctx, initial.as_ref(), &grid, &acc_p, &val_p, &body)
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

    fn add_plan() -> FastScan {
        FastScan::Op {
            op: FastOp::Add,
            left: Box::new(FastScan::Acc),
            right: Box::new(FastScan::Val),
        }
    }

    fn grid_row(vals: &[f64]) -> Vec<Vec<ExcelValue>> {
        vec![vals.iter().copied().map(n).collect()]
    }

    #[test]
    fn matrix_scalar_and_empty() {
        assert_eq!(matrix(n(5.0)).unwrap(), vec![vec![n(5.0)]]);
        assert_eq!(
            matrix(ExcelValue::Error(ExcelError::Div0)),
            Err(ExcelError::Div0)
        );
        assert_eq!(matrix(ExcelValue::Array(vec![])), Err(ExcelError::Calc));
        assert_eq!(
            matrix(ExcelValue::Array(vec![vec![]])),
            Err(ExcelError::Calc)
        );
    }

    #[test]
    fn fast_matches_naive_sum() {
        let plan = add_plan();
        let grid = grid_row(&[1.0, 2.0, 3.0]);
        let init = n(0.0);
        assert_eq!(
            scan_fast(Some(&init), &grid, &plan),
            scan_naive(Some(&init), &grid, &plan)
        );
        assert_eq!(
            scan_fast(Some(&init), &grid, &plan),
            ExcelValue::Array(vec![vec![n(1.0), n(3.0), n(6.0)]])
        );
    }

    #[test]
    fn omitted_initial_skips_first_lambda() {
        let plan = add_plan();
        let grid = grid_row(&[1.0, 2.0, 3.0]);
        assert_eq!(
            scan_fast(None, &grid, &plan),
            ExcelValue::Array(vec![vec![n(1.0), n(3.0), n(6.0)]])
        );
    }

    #[test]
    fn classify_acc_plus_val() {
        let body = parse("a+v").unwrap();
        assert_eq!(classify(&body, "a", "v"), Some(add_plan()));
        assert_eq!(classify(&parse("A+V").unwrap(), "a", "v"), Some(add_plan()));
    }

    #[test]
    fn formula_running_sum() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=SCAN(0,{1,2,3},LAMBDA(a,v,a+v))").unwrap(),
            ExcelValue::Array(vec![vec![n(1.0), n(3.0), n(6.0)]])
        );
    }

    #[test]
    fn formula_omitted_and_two_arg() {
        let wb = Workbook::default();
        let want = ExcelValue::Array(vec![vec![n(1.0), n(3.0), n(6.0)]]);
        assert_eq!(
            eval_formula_in(&wb, "=SCAN(,{1,2,3},LAMBDA(a,v,a+v))").unwrap(),
            want
        );
        assert_eq!(
            eval_formula_in(&wb, "=SCAN({1,2,3},LAMBDA(a,v,a+v))").unwrap(),
            want
        );
    }

    #[test]
    fn formula_row_major_2d() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=SCAN(0,{1,2;3,4},LAMBDA(a,v,a+v))").unwrap(),
            ExcelValue::Array(vec![vec![n(1.0), n(3.0)], vec![n(6.0), n(10.0)]])
        );
    }

    #[test]
    fn formula_named_lambda() {
        let wb = Workbook {
            sheets: vec![Sheet::new("Sheet1")],
            names: vec![DefinedName {
                name: "Add".into(),
                refers_to: "=LAMBDA(a,v,a+v)".into(),
            }],
        };
        assert_eq!(
            eval_formula_in(&wb, "=SCAN(0,{1,2},Add)").unwrap(),
            ExcelValue::Array(vec![vec![n(1.0), n(3.0)]])
        );
    }

    #[test]
    fn body_error_stays_then_propagates_when_used() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=SCAN(0,{1,0,2},LAMBDA(a,v,1/v))").unwrap(),
            ExcelValue::Array(vec![vec![
                n(1.0),
                ExcelValue::Error(ExcelError::Div0),
                n(0.5)
            ]])
        );
        assert_eq!(
            eval_formula_in(&wb, "=SCAN(0,{1,0,2},LAMBDA(a,v,a+1/v))").unwrap(),
            ExcelValue::Array(vec![vec![
                n(1.0),
                ExcelValue::Error(ExcelError::Div0),
                ExcelValue::Error(ExcelError::Div0)
            ]])
        );
    }

    #[test]
    fn bad_lambda_is_value() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=SCAN(0,{1},LAMBDA(a,1))").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=SCAN(0,{1},1)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=SCAN({1,2,3})").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn xlfn_prefix() {
        assert_eq!(
            eval_formula_in(
                &Workbook::default(),
                "=_xlfn.SCAN(0,{1,2},_xlfn.LAMBDA(a,v,a+v))"
            )
            .unwrap(),
            ExcelValue::Array(vec![vec![n(1.0), n(3.0)]])
        );
    }

    #[test]
    fn apply_naive_matches_eval_for_if_body() {
        use crate::eval::Ctx;
        use crate::eval::Evaluator;
        use std::collections::HashSet;
        use xlsx_types::{CellAddr, EvalSpec, EvalTarget};

        let spec = EvalSpec {
            case_id: "naive".into(),
            workbook: Workbook::default(),
            target: EvalTarget::formula("=SCAN(,{3,1,4},LAMBDA(a,v,IF(a>v,a,v)))"),
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
        let body = parse("IF(a>v,a,v)").unwrap();
        let grid = vec![vec![n(3.0), n(1.0), n(4.0)]];
        let via_naive = apply_naive(&ev, &mut ctx, None, &grid, "a", "v", &body).unwrap();
        assert_eq!(
            via_naive,
            ExcelValue::Array(vec![vec![n(3.0), n(3.0), n(4.0)]])
        );
    }
}
