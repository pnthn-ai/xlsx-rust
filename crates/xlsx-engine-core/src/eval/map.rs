//! Excel `MAP(array1, [array2, …], LAMBDA(p1, …, body))`.
//!
//! Applies a LAMBDA once per output cell, pairing elements at the same
//! `(row, col)` of every input array. The last argument is inspected as a
//! LAMBDA (inline or a defined name that refers to one), not evaluated as a
//! worksheet value.
//!
//! Documented Excel quirks this module implements:
//!
//! - At least one array plus a LAMBDA is required. `MAP()` / `MAP(array)`
//!   and a last argument that is not a LAMBDA are `#VALUE!`.
//! - The LAMBDA must declare **exactly one name parameter per array**.
//!   Wrong arity is `#VALUE!` ("Incorrect Parameters").
//! - A scalar error in an array argument wins left-to-right, before the
//!   LAMBDA is inspected (`MAP(1/0, {1}, LAMBDA(a,b,a+b))` is `#DIV/0!`).
//! - A 1×1 / scalar is a 1×1 array. It does **not** broadcast across a
//!   larger sibling (unlike `FILTER` include or `A1:A3+1`).
//! - Arrays of different sizes are **not** a whole-call `#VALUE!`. The
//!   result shape is the union of dimensions (`max` rows × `max` cols).
//!   A position missing from any input is `#N/A` and the LAMBDA is not
//!   called. Observed Excel: `MAP(B1:C1, A2:A3, LAMBDA(a,b,a+b))` with
//!   both ranges holding `{1,2}` yields `2, #N/A; #N/A, #N/A` — **not**
//!   the arithmetic outer product `2,3; 3,4`.
//! - An error produced by the body stays in that cell; the rest of the
//!   array still computes.
//! - A LAMBDA body that returns an array (not a single value) is `#CALC!`
//!   in that cell — same rule as `MAKEARRAY` / `BYROW` / `BYCOL`.
//! - Result is always an [`ExcelValue::Array`], including `1×1`.
//! - Empty (0-row or 0-col) input is `#CALC!` (Excel cannot return an
//!   empty array). Oversized union (`> 1,048,576` rows / `16,384` cols)
//!   is `#NUM!`.
//!
//! ## Spill / broadcast / model limits
//!
//! - The engine returns an array **value**. The snippet workbook has no
//!   spill grid, so occupied neighbors never produce `#SPILL!`.
//! - Immediately-invoked `LAMBDA(...)(args)` is not parsed. Optional
//!   LAMBDA parameters and `LET` helpers are out of scope. Parameter
//!   names that tokenize as A1 refs are `#VALUE!`.
//! - Bare `LAMBDA(...)` (not consumed by `MAP` / `MAKEARRAY`) is `#CALC!`.
//! - Live Excel has sometimes failed to lift two *bare* scalars
//!   (`MAP(0,1,LAMBDA(x,y,x+y))` → `#VALUE!` on some builds). This
//!   engine treats scalars as 1×1 arrays, matching `{0}` / `{1}` / cell
//!   refs. We do not invent a `#VALUE!` golden for that lift bug.
//!
//! [`fill_fast`] specializes identity / constant / element-wise `+ * - /`
//! so `MAP(A1:A10000, LAMBDA(x, x*2))` does not walk the AST per cell.
//! [`fill_naive`] evaluates the same [`MapFast`] through a fresh Vec bind
//! on every cell — same answers, more allocation. Used as the bench
//! "before".

use super::makearray::{
    names_eq, resolve_lambda_any, MAX_COLS, MAX_ROWS,
};
use super::{excel_pow, Ctx, Evaluator};
use crate::ast::{BinOp, Expr, UnaryOp};
use xlsx_types::{EvalError, ExcelError, ExcelValue};

const NA: ExcelValue = ExcelValue::Error(ExcelError::Na);
const EMPTY: ExcelValue = ExcelValue::Empty;

/// A LAMBDA body that only names MAP parameters and literals.
#[derive(Clone, Debug, PartialEq)]
pub enum MapFast {
    Const(ExcelValue),
    Param(usize),
    Neg(Box<MapFast>),
    Percent(Box<MapFast>),
    Op {
        op: MapOp,
        left: Box<MapFast>,
        right: Box<MapFast>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Concat,
}

/// Row/column count of one MAP argument. Scalars are 1×1.
pub fn dims(v: &ExcelValue) -> Result<(usize, usize), ExcelError> {
    match v {
        ExcelValue::Error(e) => Err(*e),
        ExcelValue::Array(rows) => {
            let r = rows.len();
            let c = rows.first().map(|row| row.len()).unwrap_or(0);
            if r == 0 || c == 0 {
                return Err(ExcelError::Calc);
            }
            Ok((r, c))
        }
        _ => Ok((1, 1)),
    }
}

/// Union-of-dimensions result shape. Oversized is `#NUM!`.
pub fn output_shape(arrays: &[ExcelValue]) -> Result<(usize, usize), ExcelError> {
    if arrays.is_empty() {
        return Err(ExcelError::Value);
    }
    let mut rows = 0usize;
    let mut cols = 0usize;
    for a in arrays {
        let (r, c) = dims(a)?;
        rows = rows.max(r);
        cols = cols.max(c);
    }
    if rows > MAX_ROWS || cols > MAX_COLS {
        return Err(ExcelError::Num);
    }
    rows.checked_mul(cols).ok_or(ExcelError::Num)?;
    Ok((rows, cols))
}

/// Value at `(r, c)` if that cell exists in `v`.
pub fn get_cell(v: &ExcelValue, r: usize, c: usize) -> Option<&ExcelValue> {
    match v {
        ExcelValue::Array(rows) => rows.get(r).and_then(|row| row.get(c)),
        other if r == 0 && c == 0 => Some(other),
        _ => None,
    }
}

/// Pair corresponding cells. `None` means at least one input is missing
/// (`#N/A`, LAMBDA not called).
pub fn pair_at<'a>(arrays: &'a [ExcelValue], r: usize, c: usize) -> Option<Vec<&'a ExcelValue>> {
    let mut out = Vec::with_capacity(arrays.len());
    for a in arrays {
        out.push(get_cell(a, r, c)?);
    }
    Some(out)
}

fn in_all(arrays: &[ExcelValue], r: usize, c: usize) -> bool {
    arrays.iter().all(|a| get_cell(a, r, c).is_some())
}

/// Classify a body that only names the MAP parameters and literals.
pub fn classify(body: &Expr, params: &[String]) -> Option<MapFast> {
    match body {
        Expr::Number(n) => Some(MapFast::Const(ExcelValue::Number(*n))),
        Expr::Text(s) => Some(MapFast::Const(ExcelValue::Text(s.clone()))),
        Expr::Bool(b) => Some(MapFast::Const(ExcelValue::Bool(*b))),
        Expr::Error(e) => Some(MapFast::Const(ExcelValue::Error(*e))),
        Expr::Name(n) => params
            .iter()
            .position(|p| names_eq(n, p))
            .map(MapFast::Param),
        Expr::Unary {
            op: UnaryOp::Minus,
            expr,
        } => classify(expr, params).map(|e| MapFast::Neg(Box::new(e))),
        Expr::Unary {
            op: UnaryOp::Plus,
            expr,
        } => classify(expr, params),
        Expr::Unary {
            op: UnaryOp::Percent,
            expr,
        } => classify(expr, params).map(|e| MapFast::Percent(Box::new(e))),
        Expr::Binary { op, left, right } => {
            let fop = match op {
                BinOp::Add => MapOp::Add,
                BinOp::Sub => MapOp::Sub,
                BinOp::Mul => MapOp::Mul,
                BinOp::Div => MapOp::Div,
                BinOp::Pow => MapOp::Pow,
                BinOp::Concat => MapOp::Concat,
                _ => return None,
            };
            Some(MapFast::Op {
                op: fop,
                left: Box::new(classify(left, params)?),
                right: Box::new(classify(right, params)?),
            })
        }
        _ => None,
    }
}

/// Production fill: specialized loops for identity / constant / zip-arith.
pub fn fill_fast(arrays: &[ExcelValue], body: &MapFast) -> ExcelValue {
    match (arrays, body) {
        ([only], MapFast::Param(0)) => wrap_array(only.clone()),
        ([only], MapFast::Const(v)) => match dims(only) {
            Ok((r, c)) => ExcelValue::Array(vec![vec![v.clone(); c]; r]),
            Err(e) => ExcelValue::Error(e),
        },
        ([only], MapFast::Op { op, left, right }) => {
            if let Some(plan) = unary_arith(*op, left, right) {
                return map_unary(only, plan);
            }
            fill_walk(arrays, body)
        }
        ([a, b], MapFast::Op { op, left, right })
            if matches!((&**left, &**right), (MapFast::Param(0), MapFast::Param(1))) =>
        {
            zip_op(a, b, *op)
        }
        ([a, b], MapFast::Op { op, left, right })
            if matches!((&**left, &**right), (MapFast::Param(1), MapFast::Param(0))) =>
        {
            zip_op(b, a, *op)
        }
        _ => fill_walk(arrays, body),
    }
}

struct UnaryArith {
    op: MapOp,
    k: ExcelValue,
    param_left: bool,
}

fn unary_arith(op: MapOp, left: &MapFast, right: &MapFast) -> Option<UnaryArith> {
    match (left, right) {
        (MapFast::Param(0), MapFast::Const(k)) => Some(UnaryArith {
            op,
            k: k.clone(),
            param_left: true,
        }),
        (MapFast::Const(k), MapFast::Param(0)) => Some(UnaryArith {
            op,
            k: k.clone(),
            param_left: false,
        }),
        _ => None,
    }
}

fn map_unary(array: &ExcelValue, plan: UnaryArith) -> ExcelValue {
    match output_shape(std::slice::from_ref(array)) {
        Ok((rows, cols)) => {
            let mut grid = Vec::with_capacity(rows);
            for r in 0..rows {
                let mut row = Vec::with_capacity(cols);
                for c in 0..cols {
                    match get_cell(array, r, c) {
                        None => row.push(NA.clone()),
                        Some(v) => {
                            let cell = if plan.param_left {
                                apply_op(plan.op, v, &plan.k)
                            } else {
                                apply_op(plan.op, &plan.k, v)
                            };
                            row.push(cell);
                        }
                    }
                }
                grid.push(row);
            }
            ExcelValue::Array(grid)
        }
        Err(e) => ExcelValue::Error(e),
    }
}

fn zip_op(left: &ExcelValue, right: &ExcelValue, op: MapOp) -> ExcelValue {
    let arrays = [left.clone(), right.clone()];
    match output_shape(&arrays) {
        Ok((rows, cols)) => {
            let mut grid = Vec::with_capacity(rows);
            for r in 0..rows {
                let mut row = Vec::with_capacity(cols);
                for c in 0..cols {
                    match (get_cell(left, r, c), get_cell(right, r, c)) {
                        (Some(a), Some(b)) => row.push(apply_op(op, a, b)),
                        _ => row.push(NA.clone()),
                    }
                }
                grid.push(row);
            }
            ExcelValue::Array(grid)
        }
        Err(e) => ExcelValue::Error(e),
    }
}

fn fill_walk(arrays: &[ExcelValue], body: &MapFast) -> ExcelValue {
    match output_shape(arrays) {
        Ok((rows, cols)) => {
            let n = arrays.len();
            let mut scratch = vec![EMPTY; n];
            let mut grid = Vec::with_capacity(rows);
            for r in 0..rows {
                let mut row = Vec::with_capacity(cols);
                for c in 0..cols {
                    if !in_all(arrays, r, c) {
                        row.push(NA.clone());
                        continue;
                    }
                    for (i, a) in arrays.iter().enumerate() {
                        scratch[i] = get_cell(a, r, c).cloned().unwrap_or(EMPTY);
                    }
                    row.push(eval_fast(body, &scratch));
                }
                grid.push(row);
            }
            ExcelValue::Array(grid)
        }
        Err(e) => ExcelValue::Error(e),
    }
}

/// Allocation-heavy baseline: clone arrays, Vec-bind every cell.
pub fn fill_naive(arrays: &[ExcelValue], body: &MapFast) -> ExcelValue {
    let owned: Vec<ExcelValue> = arrays.to_vec();
    match output_shape(&owned) {
        Ok((rows, cols)) => {
            let mut grid = Vec::new();
            for r in 0..rows {
                let mut row = Vec::new();
                for c in 0..cols {
                    let mut env = Vec::new();
                    let mut missing = false;
                    for a in &owned {
                        match get_cell(a, r, c) {
                            Some(v) => env.push(v.clone()),
                            None => {
                                missing = true;
                                break;
                            }
                        }
                    }
                    if missing {
                        row.push(NA.clone());
                    } else {
                        row.push(eval_fast(body, &env));
                    }
                }
                grid.push(row);
            }
            ExcelValue::Array(grid)
        }
        Err(e) => ExcelValue::Error(e),
    }
}

fn wrap_array(v: ExcelValue) -> ExcelValue {
    match v {
        ExcelValue::Error(e) => ExcelValue::Error(e),
        ExcelValue::Array(rows) if rows.is_empty() || rows[0].is_empty() => {
            ExcelValue::Error(ExcelError::Calc)
        }
        ExcelValue::Array(rows) => ExcelValue::Array(rows),
        other => ExcelValue::Array(vec![vec![other]]),
    }
}

fn eval_fast(body: &MapFast, env: &[ExcelValue]) -> ExcelValue {
    match body {
        MapFast::Const(v) => v.clone(),
        MapFast::Param(i) => env.get(*i).cloned().unwrap_or(EMPTY),
        MapFast::Neg(inner) => match eval_fast(inner, env) {
            ExcelValue::Error(e) => ExcelValue::Error(e),
            v => match super::coerce::to_number(&v) {
                Ok(n) => ExcelValue::Number(-n),
                Err(e) => ExcelValue::Error(e),
            },
        },
        MapFast::Percent(inner) => match eval_fast(inner, env) {
            ExcelValue::Error(e) => ExcelValue::Error(e),
            v => match super::coerce::to_number(&v) {
                Ok(n) => ExcelValue::Number(n / 100.0),
                Err(e) => ExcelValue::Error(e),
            },
        },
        MapFast::Op { op, left, right } => {
            let l = eval_fast(left, env);
            if let ExcelValue::Error(e) = l {
                return ExcelValue::Error(e);
            }
            let rv = eval_fast(right, env);
            if let ExcelValue::Error(e) = rv {
                return ExcelValue::Error(e);
            }
            apply_op(*op, &l, &rv)
        }
    }
}

fn apply_op(op: MapOp, left: &ExcelValue, right: &ExcelValue) -> ExcelValue {
    match op {
        MapOp::Add => num2(left, right, |a, b| ExcelValue::Number(a + b)),
        MapOp::Sub => num2(left, right, |a, b| ExcelValue::Number(a - b)),
        MapOp::Mul => num2(left, right, |a, b| ExcelValue::Number(a * b)),
        MapOp::Div => match (
            super::coerce::to_number(left),
            super::coerce::to_number(right),
        ) {
            (Err(e), _) | (_, Err(e)) => ExcelValue::Error(e),
            (Ok(_), Ok(d)) if d == 0.0 => ExcelValue::Error(ExcelError::Div0),
            (Ok(n), Ok(d)) => ExcelValue::Number(n / d),
        },
        MapOp::Pow => excel_pow(left, right),
        MapOp::Concat => match (super::coerce::to_text(left), super::coerce::to_text(right)) {
            (Ok(a), Ok(b)) => ExcelValue::Text(a + &b),
            (Err(e), _) | (_, Err(e)) => ExcelValue::Error(e),
        },
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

/// Evaluator entry: evaluate arrays LTR, then bind the LAMBDA.
pub(crate) fn eval(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let (lambda_expr, array_exprs) = args.split_last().unwrap();
    let mut arrays = Vec::with_capacity(array_exprs.len());
    for expr in array_exprs {
        let v = ev.eval_expr(expr, ctx)?;
        if let ExcelValue::Error(e) = v {
            return Ok(ExcelValue::Error(e));
        }
        arrays.push(v);
    }
    let (params, body) = match resolve_lambda_any(lambda_expr, ctx) {
        Ok(l) => l,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    if params.len() != arrays.len() {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    apply(ev, ctx, &arrays, &params, &body)
}

/// Fast kernel when the body is param-only arithmetic, else AST + locals.
pub(crate) fn apply(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    arrays: &[ExcelValue],
    params: &[String],
    body: &Expr,
) -> Result<ExcelValue, EvalError> {
    if let Some(fast) = classify(body, params) {
        return Ok(fill_fast(arrays, &fast));
    }
    apply_naive(ev, ctx, arrays, params, body)
}

/// Always walk the AST (bench baseline / IF / cell-ref bodies).
pub(crate) fn apply_naive(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    arrays: &[ExcelValue],
    params: &[String],
    body: &Expr,
) -> Result<ExcelValue, EvalError> {
    let (rows, cols) = match output_shape(arrays) {
        Ok(s) => s,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let base = ctx.locals.len();
    for p in params {
        ctx.locals.push((p.clone(), EMPTY));
    }
    let mut grid = Vec::with_capacity(rows);
    for r in 0..rows {
        let mut row = Vec::with_capacity(cols);
        for c in 0..cols {
            match pair_at(arrays, r, c) {
                None => row.push(NA.clone()),
                Some(vals) => {
                    for (i, v) in vals.into_iter().enumerate() {
                        ctx.locals[base + i].1 = v.clone();
                    }
                    row.push(scalar_cell(ev.eval_expr(body, ctx)?));
                }
            }
        }
        grid.push(row);
    }
    ctx.locals.truncate(base);
    Ok(ExcelValue::Array(grid))
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

    fn arr(rows: Vec<Vec<ExcelValue>>) -> ExcelValue {
        ExcelValue::Array(rows)
    }

    fn times2() -> MapFast {
        MapFast::Op {
            op: MapOp::Mul,
            left: Box::new(MapFast::Param(0)),
            right: Box::new(MapFast::Const(n(2.0))),
        }
    }

    fn add_xy() -> MapFast {
        MapFast::Op {
            op: MapOp::Add,
            left: Box::new(MapFast::Param(0)),
            right: Box::new(MapFast::Param(1)),
        }
    }

    #[test]
    fn classify_x_times_2() {
        let body = parse("x*2").unwrap();
        assert_eq!(classify(&body, &["x".into()]), Some(times2()));
        assert_eq!(classify(&parse("X*2").unwrap(), &["x".into()]), Some(times2()));
    }

    #[test]
    fn fast_matches_naive_times2() {
        let src = arr(vec![vec![n(1.0)], vec![n(2.0)], vec![n(3.0)]]);
        let plan = times2();
        let arrays = [src];
        assert_eq!(fill_fast(&arrays, &plan), fill_naive(&arrays, &plan));
        assert_eq!(
            fill_fast(&arrays, &plan),
            arr(vec![vec![n(2.0)], vec![n(4.0)], vec![n(6.0)]])
        );
    }

    #[test]
    fn identity_wraps_scalar() {
        let plan = MapFast::Param(0);
        assert_eq!(
            fill_fast(&[n(7.0)], &plan),
            arr(vec![vec![n(7.0)]])
        );
    }

    #[test]
    fn union_pads_na() {
        let a = arr(vec![vec![n(1.0), n(2.0)]]);
        let b = arr(vec![vec![n(1.0)], vec![n(2.0)]]);
        let got = fill_fast(&[a, b], &add_xy());
        assert_eq!(
            got,
            arr(vec![
                vec![n(2.0), NA.clone()],
                vec![NA.clone(), NA.clone()],
            ])
        );
        assert_eq!(got, fill_naive(&[
            arr(vec![vec![n(1.0), n(2.0)]]),
            arr(vec![vec![n(1.0)], vec![n(2.0)]]),
        ], &add_xy()));
    }

    #[test]
    fn formula_times2() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=MAP({1,2,3},LAMBDA(x,x*2))").unwrap(),
            arr(vec![vec![n(2.0), n(4.0), n(6.0)]])
        );
    }

    #[test]
    fn formula_two_arrays() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=MAP({1,2},{10,20},LAMBDA(a,b,a+b))").unwrap(),
            arr(vec![vec![n(11.0), n(22.0)]])
        );
    }

    #[test]
    fn formula_named_lambda() {
        let wb = Workbook {
            sheets: vec![Sheet::new("Sheet1")],
            names: vec![DefinedName {
                name: "Dbl".into(),
                refers_to: "=LAMBDA(x,x*2)".into(),
            }],
        };
        assert_eq!(
            eval_formula_in(&wb, "=MAP({1;2},Dbl)").unwrap(),
            arr(vec![vec![n(2.0)], vec![n(4.0)]])
        );
    }

    #[test]
    fn body_error_stays_in_cell() {
        let v = eval_formula_in(&Workbook::default(), "=MAP({1,0},LAMBDA(x,1/x))").unwrap();
        assert_eq!(
            v,
            arr(vec![vec![n(1.0), ExcelValue::Error(ExcelError::Div0)]])
        );
    }

    #[test]
    fn perp_ranges_pad_na() {
        let mut sheet = Sheet::new("Sheet1");
        sheet
            .cells
            .insert("B1".into(), xlsx_types::Cell::value(n(1.0)));
        sheet
            .cells
            .insert("C1".into(), xlsx_types::Cell::value(n(2.0)));
        sheet
            .cells
            .insert("A2".into(), xlsx_types::Cell::value(n(1.0)));
        sheet
            .cells
            .insert("A3".into(), xlsx_types::Cell::value(n(2.0)));
        let wb = Workbook {
            sheets: vec![sheet],
            names: vec![],
        };
        assert_eq!(
            eval_formula_in(&wb, "=MAP(B1:C1,A2:A3,LAMBDA(a,b,a+b))").unwrap(),
            arr(vec![
                vec![n(2.0), NA.clone()],
                vec![NA.clone(), NA.clone()],
            ])
        );
    }

    #[test]
    fn bad_lambda_is_value() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=MAP({1},LAMBDA(a,b,a+b))").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=MAP({1},1)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=MAP({1})").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn array_arg_error_wins() {
        assert_eq!(
            eval_formula_in(&Workbook::default(), "=MAP(1/0,{1},LAMBDA(a,b,a+b))").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
    }

    #[test]
    fn xlfn_prefix() {
        assert_eq!(
            eval_formula_in(
                &Workbook::default(),
                "=_xlfn.MAP({1,2},_xlfn.LAMBDA(x,x+1))"
            )
            .unwrap(),
            arr(vec![vec![n(2.0), n(3.0)]])
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
            target: EvalTarget::formula("=MAP({1,5},LAMBDA(a,IF(a>4,a*a,a)))"),
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
        let body = parse("IF(a>4,a*a,a)").unwrap();
        let src = arr(vec![vec![n(1.0), n(5.0)]]);
        let via_naive = apply_naive(&ev, &mut ctx, &[src], &["a".into()], &body).unwrap();
        assert_eq!(via_naive, arr(vec![vec![n(1.0), n(25.0)]]));
    }
}
