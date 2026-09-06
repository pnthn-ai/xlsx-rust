//! Excel `MAKEARRAY(rows, cols, LAMBDA(r, c, body))`.
//!
//! Builds a calculated array by calling a two-parameter LAMBDA once per cell.
//! Row and column indexes are **1-based** and relative to the result array
//! (not `ROW()` / `COLUMN()` sheet coordinates).
//!
//! Documented Excel quirks this module implements:
//!
//! - `rows` / `cols` coerce like other numeric args (`TRUE` → 1, `"3"` → 3,
//!   empty → 0). Values are truncated toward zero. After truncation, a
//!   dimension `< 1` or a non-number is `#VALUE!`.
//! - Non-finite dimensions (`#DIV/0!` already surfaced by the caller) and
//!   sizes above the worksheet grid (`1,048,576` rows / `16,384` columns)
//!   are `#NUM!`.
//! - The third argument must be an inline `LAMBDA` (or a defined name that
//!   refers to one) with **exactly two** name parameters. Anything else is
//!   `#VALUE!` ("Incorrect Parameters").
//! - A LAMBDA body that returns an array (not a single value) is `#CALC!`
//!   in that cell — same rule as `BYROW` / `BYCOL`.
//! - An error produced by the body stays in that cell; the rest of the
//!   array still computes. The engine does **not** collapse the whole
//!   spill to one error.
//!
//! ## Spill / model limits
//!
//! - The engine returns an [`ExcelValue::Array`]; it does **not** write a
//!   spill range. Occupied neighbors never produce `#SPILL!`.
//! - Bare `LAMBDA(...)` (not consumed by `MAKEARRAY`) is `#CALC!` — this
//!   engine has no first-class function value.
//! - Immediately-invoked `LAMBDA(...)(args)` is not parsed.
//! - Parameter names that tokenize as A1 refs (`A1`, `R1C1`-looking cells)
//!   are not supported.
//! - Optional LAMBDA parameters and `LET` helpers are out of scope.
//!
//! [`fill_fast`] specializes index-only arithmetic (`r*c`, `r+c`, constants)
//! so a multiplication table does not walk the AST per cell.
//! [`fill_naive`] evaluates the same [`FastBody`] through a fresh HashMap
//! binding on every cell — same answers, more allocation. Used as the bench
//! "before".

use super::{excel_pow, Ctx, Evaluator};
use crate::ast::{BinOp, Expr, UnaryOp};
use crate::parse::parse;
use std::collections::HashMap;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

/// Excel worksheet row cap (dynamic-array / spill limit).
pub const MAX_ROWS: usize = 1_048_576;
/// Excel worksheet column cap (`XFD`).
pub const MAX_COLS: usize = 16_384;

/// A LAMBDA body that depends only on the two index parameters and literals.
#[derive(Clone, Debug, PartialEq)]
pub enum FastBody {
    Const(ExcelValue),
    Row,
    Col,
    Neg(Box<FastBody>),
    Op {
        op: FastOp,
        left: Box<FastBody>,
        right: Box<FastBody>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FastOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
}

/// Coerce one MAKEARRAY dimension. Truncates toward zero.
pub fn dim(v: &ExcelValue, max: usize) -> Result<usize, ExcelError> {
    let n = super::coerce::to_number(v)?;
    if !n.is_finite() {
        return Err(ExcelError::Num);
    }
    let t = n.trunc();
    if t < 1.0 {
        return Err(ExcelError::Value);
    }
    if t > max as f64 {
        return Err(ExcelError::Num);
    }
    Ok(t as usize)
}

/// Validate and truncate `rows` / `cols`.
pub fn dims(rows: &ExcelValue, cols: &ExcelValue) -> Result<(usize, usize), ExcelError> {
    let r = dim(rows, MAX_ROWS)?;
    let c = dim(cols, MAX_COLS)?;
    r.checked_mul(c).ok_or(ExcelError::Num)?;
    Ok((r, c))
}

pub fn is_lambda_name(name: &str) -> bool {
    fn_key(name) == "LAMBDA"
}

/// Uppercase function id with a leading `_XLFN.` prefix stripped.
pub fn fn_key(name: &str) -> String {
    let u = name.to_ascii_uppercase();
    u.strip_prefix("_XLFN.").unwrap_or(&u).to_string()
}

pub fn strip_xlpm(name: &str) -> &str {
    if name.len() >= 6 && name.as_bytes()[..6].eq_ignore_ascii_case(b"_xlpm.") {
        &name[6..]
    } else {
        name
    }
}

pub fn names_eq(a: &str, b: &str) -> bool {
    strip_xlpm(a).eq_ignore_ascii_case(strip_xlpm(b))
}

pub fn lookup_binding(locals: &[(String, ExcelValue)], name: &str) -> Option<ExcelValue> {
    locals
        .iter()
        .rev()
        .find(|(n, _)| names_eq(n, name))
        .map(|(_, v)| v.clone())
}

/// Extract `LAMBDA(row, col, body)` from an inline call or a defined name.
pub(crate) fn resolve_lambda(
    expr: &Expr,
    ctx: &Ctx<'_>,
) -> Result<(String, String, Expr), ExcelError> {
    resolve_lambda_depth(expr, ctx, 0)
}

fn resolve_lambda_depth(
    expr: &Expr,
    ctx: &Ctx<'_>,
    depth: usize,
) -> Result<(String, String, Expr), ExcelError> {
    if depth > 16 {
        return Err(ExcelError::Value);
    }
    match expr {
        Expr::Call { name, args } if is_lambda_name(name) => lambda_params(args),
        Expr::Name(n) => {
            let def = ctx
                .spec
                .workbook
                .defined_name(n)
                .map_err(|_| ExcelError::Value)?;
            let ast = parse(&def.refers_to).map_err(|_| ExcelError::Value)?;
            resolve_lambda_depth(&ast, ctx, depth + 1)
        }
        _ => Err(ExcelError::Value),
    }
}

fn lambda_params(args: &[Expr]) -> Result<(String, String, Expr), ExcelError> {
    if args.len() != 3 {
        return Err(ExcelError::Value);
    }
    let row = param_name(&args[0]).ok_or(ExcelError::Value)?;
    let col = param_name(&args[1]).ok_or(ExcelError::Value)?;
    Ok((row, col, args[2].clone()))
}

fn param_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Name(n) => Some(strip_xlpm(n).to_string()),
        _ => None,
    }
}

/// Classify a body that only names the two index parameters.
pub fn classify(body: &Expr, row_param: &str, col_param: &str) -> Option<FastBody> {
    match body {
        Expr::Number(n) => Some(FastBody::Const(ExcelValue::Number(*n))),
        Expr::Text(s) => Some(FastBody::Const(ExcelValue::Text(s.clone()))),
        Expr::Bool(b) => Some(FastBody::Const(ExcelValue::Bool(*b))),
        Expr::Error(e) => Some(FastBody::Const(ExcelValue::Error(*e))),
        Expr::Name(n) if names_eq(n, row_param) => Some(FastBody::Row),
        Expr::Name(n) if names_eq(n, col_param) => Some(FastBody::Col),
        Expr::Unary {
            op: UnaryOp::Minus,
            expr,
        } => classify(expr, row_param, col_param).map(|e| FastBody::Neg(Box::new(e))),
        Expr::Unary {
            op: UnaryOp::Plus,
            expr,
        } => classify(expr, row_param, col_param),
        Expr::Binary { op, left, right } => {
            let fop = match op {
                BinOp::Add => FastOp::Add,
                BinOp::Sub => FastOp::Sub,
                BinOp::Mul => FastOp::Mul,
                BinOp::Div => FastOp::Div,
                BinOp::Pow => FastOp::Pow,
                _ => return None,
            };
            Some(FastBody::Op {
                op: fop,
                left: Box::new(classify(left, row_param, col_param)?),
                right: Box::new(classify(right, row_param, col_param)?),
            })
        }
        _ => None,
    }
}

/// Production fill: specialized loops for the common index kernels.
pub fn fill_fast(rows: usize, cols: usize, body: &FastBody) -> ExcelValue {
    match body {
        FastBody::Const(v) => ExcelValue::Array(vec![vec![v.clone(); cols]; rows]),
        FastBody::Row => ExcelValue::Array(
            (1..=rows)
                .map(|r| vec![ExcelValue::Number(r as f64); cols])
                .collect(),
        ),
        FastBody::Col => ExcelValue::Array(
            (0..rows)
                .map(|_| (1..=cols).map(|c| ExcelValue::Number(c as f64)).collect())
                .collect(),
        ),
        FastBody::Op {
            op: FastOp::Mul,
            left,
            right,
        } if matches!(**left, FastBody::Row) && matches!(**right, FastBody::Col) => {
            mul_table(rows, cols)
        }
        FastBody::Op {
            op: FastOp::Mul,
            left,
            right,
        } if matches!(**left, FastBody::Col) && matches!(**right, FastBody::Row) => {
            mul_table(rows, cols)
        }
        FastBody::Op {
            op: FastOp::Add,
            left,
            right,
        } if is_row_col_pair(left, right) => add_table(rows, cols),
        other => fill_walk(rows, cols, other),
    }
}

fn is_row_col_pair(left: &FastBody, right: &FastBody) -> bool {
    matches!(
        (left, right),
        (FastBody::Row, FastBody::Col) | (FastBody::Col, FastBody::Row)
    )
}

fn mul_table(rows: usize, cols: usize) -> ExcelValue {
    let mut grid = Vec::with_capacity(rows);
    for r in 1..=rows {
        let rf = r as f64;
        let mut row = Vec::with_capacity(cols);
        for c in 1..=cols {
            row.push(ExcelValue::Number(rf * c as f64));
        }
        grid.push(row);
    }
    ExcelValue::Array(grid)
}

fn add_table(rows: usize, cols: usize) -> ExcelValue {
    let mut grid = Vec::with_capacity(rows);
    for r in 1..=rows {
        let rf = r as f64;
        let mut row = Vec::with_capacity(cols);
        for c in 1..=cols {
            row.push(ExcelValue::Number(rf + c as f64));
        }
        grid.push(row);
    }
    ExcelValue::Array(grid)
}

fn fill_walk(rows: usize, cols: usize, body: &FastBody) -> ExcelValue {
    let mut grid = Vec::with_capacity(rows);
    for r in 1..=rows {
        let rf = r as f64;
        let mut row = Vec::with_capacity(cols);
        for c in 1..=cols {
            row.push(eval_fast(body, rf, c as f64));
        }
        grid.push(row);
    }
    ExcelValue::Array(grid)
}

/// Allocation-heavy baseline: HashMap bind + walk on every cell.
pub fn fill_naive(rows: usize, cols: usize, body: &FastBody) -> ExcelValue {
    let mut grid = Vec::new();
    for r in 1..=rows {
        let mut row = Vec::new();
        for c in 1..=cols {
            let mut env = HashMap::with_capacity(2);
            env.insert("r", r as f64);
            env.insert("c", c as f64);
            row.push(eval_fast_env(body, &env));
        }
        grid.push(row);
    }
    ExcelValue::Array(grid)
}

fn eval_fast(body: &FastBody, r: f64, c: f64) -> ExcelValue {
    match body {
        FastBody::Const(v) => v.clone(),
        FastBody::Row => ExcelValue::Number(r),
        FastBody::Col => ExcelValue::Number(c),
        FastBody::Neg(inner) => match eval_fast(inner, r, c) {
            ExcelValue::Error(e) => ExcelValue::Error(e),
            v => match super::coerce::to_number(&v) {
                Ok(n) => ExcelValue::Number(-n),
                Err(e) => ExcelValue::Error(e),
            },
        },
        FastBody::Op { op, left, right } => {
            let l = eval_fast(left, r, c);
            if let ExcelValue::Error(e) = l {
                return ExcelValue::Error(e);
            }
            let rv = eval_fast(right, r, c);
            if let ExcelValue::Error(e) = rv {
                return ExcelValue::Error(e);
            }
            apply_op(*op, &l, &rv)
        }
    }
}

fn eval_fast_env(body: &FastBody, env: &HashMap<&str, f64>) -> ExcelValue {
    let r = *env.get("r").unwrap_or(&0.0);
    let c = *env.get("c").unwrap_or(&0.0);
    eval_fast(body, r, c)
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

/// Evaluator entry: fast kernel when the body is index-only, else AST + locals.
pub(crate) fn apply(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    rows: usize,
    cols: usize,
    row_param: &str,
    col_param: &str,
    body: &Expr,
) -> Result<ExcelValue, EvalError> {
    if let Some(fast) = classify(body, row_param, col_param) {
        return Ok(fill_fast(rows, cols, &fast));
    }
    apply_naive(ev, ctx, rows, cols, row_param, col_param, body)
}

/// Always walk the AST (bench baseline / seed-compliant-shaped path).
pub(crate) fn apply_naive(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    rows: usize,
    cols: usize,
    row_param: &str,
    col_param: &str,
    body: &Expr,
) -> Result<ExcelValue, EvalError> {
    apply_general(ev, ctx, rows, cols, row_param, col_param, body)
}

fn apply_general(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    rows: usize,
    cols: usize,
    row_param: &str,
    col_param: &str,
    body: &Expr,
) -> Result<ExcelValue, EvalError> {
    let base = ctx.locals.len();
    ctx.locals
        .push((row_param.to_string(), ExcelValue::Number(1.0)));
    ctx.locals
        .push((col_param.to_string(), ExcelValue::Number(1.0)));
    let mut grid = Vec::with_capacity(rows);
    for r in 1..=rows {
        ctx.locals[base].1 = ExcelValue::Number(r as f64);
        let mut row = Vec::with_capacity(cols);
        for c in 1..=cols {
            ctx.locals[base + 1].1 = ExcelValue::Number(c as f64);
            row.push(scalar_cell(ev.eval_expr(body, ctx)?));
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
    use xlsx_types::{DefinedName, Workbook};

    fn n(x: f64) -> ExcelValue {
        ExcelValue::Number(x)
    }

    fn mul_plan() -> FastBody {
        FastBody::Op {
            op: FastOp::Mul,
            left: Box::new(FastBody::Row),
            right: Box::new(FastBody::Col),
        }
    }

    #[test]
    fn dims_truncate_and_reject() {
        assert_eq!(dims(&n(2.9), &n(1.1)).unwrap(), (2, 1));
        assert_eq!(dims(&ExcelValue::Bool(true), &n(1.0)).unwrap(), (1, 1));
        assert_eq!(dims(&n(0.0), &n(1.0)), Err(ExcelError::Value));
        assert_eq!(dims(&n(-2.0), &n(1.0)), Err(ExcelError::Value));
        assert_eq!(
            dims(&ExcelValue::Text("x".into()), &n(1.0)),
            Err(ExcelError::Value)
        );
        assert_eq!(
            dims(&n((MAX_ROWS + 1) as f64), &n(1.0)),
            Err(ExcelError::Num)
        );
        assert_eq!(
            dims(&n(1.0), &n((MAX_COLS + 1) as f64)),
            Err(ExcelError::Num)
        );
    }

    #[test]
    fn fast_matches_naive_mul() {
        let plan = mul_plan();
        assert_eq!(fill_fast(3, 3, &plan), fill_naive(3, 3, &plan));
        assert_eq!(
            fill_fast(3, 3, &plan),
            ExcelValue::Array(vec![
                vec![n(1.0), n(2.0), n(3.0)],
                vec![n(2.0), n(4.0), n(6.0)],
                vec![n(3.0), n(6.0), n(9.0)],
            ])
        );
    }

    #[test]
    fn classify_r_times_c() {
        let body = parse("r*c").unwrap();
        assert_eq!(classify(&body, "r", "c"), Some(mul_plan()));
        assert_eq!(classify(&parse("R*C").unwrap(), "r", "c"), Some(mul_plan()));
    }

    #[test]
    fn formula_mul_table() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=MAKEARRAY(2,3,LAMBDA(r,c,r*c))").unwrap(),
            ExcelValue::Array(vec![
                vec![n(1.0), n(2.0), n(3.0)],
                vec![n(2.0), n(4.0), n(6.0)],
            ])
        );
    }

    #[test]
    fn formula_named_lambda() {
        let wb = Workbook {
            sheets: vec![xlsx_types::Sheet::new("Sheet1")],
            names: vec![DefinedName {
                name: "Mul".into(),
                refers_to: "=LAMBDA(r,c,r*c)".into(),
            }],
        };
        assert_eq!(
            eval_formula_in(&wb, "=MAKEARRAY(2,2,Mul)").unwrap(),
            ExcelValue::Array(vec![vec![n(1.0), n(2.0)], vec![n(2.0), n(4.0)]])
        );
    }

    #[test]
    fn body_error_stays_in_cell() {
        let v =
            eval_formula_in(&Workbook::default(), "=MAKEARRAY(1,2,LAMBDA(r,c,1/(c-1)))").unwrap();
        assert_eq!(
            v,
            ExcelValue::Array(vec![vec![ExcelValue::Error(ExcelError::Div0), n(1.0)]])
        );
    }

    #[test]
    fn bad_lambda_is_value() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=MAKEARRAY(1,1,LAMBDA(r,1))").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=MAKEARRAY(1,1,1)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=MAKEARRAY(0,1,LAMBDA(r,c,1))").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn bare_lambda_is_calc() {
        assert_eq!(
            eval_formula_in(&Workbook::default(), "=LAMBDA(x,x+1)").unwrap(),
            ExcelValue::Error(ExcelError::Calc)
        );
    }

    #[test]
    fn xlfn_prefix() {
        assert_eq!(
            eval_formula_in(
                &Workbook::default(),
                "=_xlfn.MAKEARRAY(1,1,_xlfn.LAMBDA(r,c,7))"
            )
            .unwrap(),
            ExcelValue::Array(vec![vec![n(7.0)]])
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
            target: EvalTarget::formula("=MAKEARRAY(2,2,LAMBDA(r,c,IF(r=c,1,0)))"),
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
        let body = parse("IF(r=c,1,0)").unwrap();
        let via_naive = apply_naive(&ev, &mut ctx, 2, 2, "r", "c", &body).unwrap();
        assert_eq!(
            via_naive,
            ExcelValue::Array(vec![vec![n(1.0), n(0.0)], vec![n(0.0), n(1.0)],])
        );
    }
}
