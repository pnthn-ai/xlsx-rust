//! Excel `REDUCE([initial_value], array, LAMBDA(acc, value, body))`.
//!
//! Folds `array` in **row-major** order (left-to-right, then down). The
//! LAMBDA is applied once per element after the accumulator is seeded.
//!
//! Documented Excel quirks this module implements:
//!
//! - `initial_value` is optional. Omit it with an empty first slot
//!   (`REDUCE(, array, LAMBDA(...))`) or the 2-arg form
//!   (`REDUCE(array, LAMBDA(...))`). The first array element becomes the
//!   accumulator and the LAMBDA starts at the second element.
//! - A blank cell used as `initial_value` is **Empty**, not omitted
//!   (`Empty+1` is `1`). A missing slot is omitted.
//! - An empty array with a provided initial returns that initial (LAMBDA
//!   is never called). An empty array with no initial is `#CALC!`.
//! - A one-element array with no initial returns that element; a LAMBDA
//!   that would error is not run.
//! - The LAMBDA must have **exactly two** name parameters (inline or a
//!   defined name that refers to one). Anything else is `#VALUE!`.
//! - A body that returns an array is a valid accumulator (unlike
//!   `MAKEARRAY`, which maps that to `#CALC!` per cell).
//! - An error that is the whole `initial_value` / `array` argument
//!   surfaces left-to-right. An error *inside* the array is a `value`
//!   binding and surfaces only if the body uses it.
//!
//! ## Spill / model limits
//!
//! - A scalar accumulator is returned as a scalar; an array accumulator
//!   is an [`ExcelValue::Array`]. The engine does **not** write a spill
//!   range. Occupied neighbors never produce `#SPILL!`.
//! - Bare `LAMBDA(...)` (not consumed by `REDUCE` / `MAKEARRAY`) is
//!   `#CALC!` — this engine has no first-class function value.
//! - Immediately-invoked `LAMBDA(...)(args)` is not parsed.
//! - Parameter names that tokenize as A1 refs are not supported.
//! - Optional LAMBDA parameters and `LET` helpers are out of scope.
//!
//! [`fold_fast`] specializes `acc+val` / `acc*val` / `acc&val` (and the
//! `acc` / `val` / constant plans) so a running sum does not walk the
//! AST per element. [`fold_naive`] evaluates the same [`ReducePlan`]
//! through a fresh HashMap binding on every step — same answers, more
//! allocation. Used as the bench "before".

use super::{concat, excel_pow, Ctx, Evaluator};
use crate::ast::{BinOp, Expr, UnaryOp};
use std::collections::HashMap;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

use super::makearray::{names_eq, resolve_lambda_n};

/// A LAMBDA body that depends only on the two REDUCE parameters and literals.
#[derive(Clone, Debug, PartialEq)]
pub enum ReducePlan {
    Const(ExcelValue),
    Acc,
    Val,
    Neg(Box<ReducePlan>),
    Op {
        op: ReduceOp,
        left: Box<ReducePlan>,
        right: Box<ReducePlan>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReduceOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Concat,
}

/// Row-major walk. A scalar is one cell; nested arrays are **not** unnested.
pub fn for_each_row_major(v: &ExcelValue, mut f: impl FnMut(&ExcelValue)) {
    match v {
        ExcelValue::Array(rows) => {
            for row in rows {
                for c in row {
                    f(c);
                }
            }
        }
        other => f(other),
    }
}

/// Flatten to a row-major `Vec` (bench / tests).
pub fn flatten_row_major(v: &ExcelValue) -> Vec<ExcelValue> {
    let mut out = Vec::new();
    for_each_row_major(v, |c| out.push(c.clone()));
    out
}

pub fn cell_count(v: &ExcelValue) -> usize {
    match v {
        ExcelValue::Array(rows) => rows.iter().map(|r| r.len()).sum(),
        _ => 1,
    }
}

/// Classify a body that only names the two REDUCE parameters.
pub fn classify(body: &Expr, acc_param: &str, val_param: &str) -> Option<ReducePlan> {
    match body {
        Expr::Number(n) => Some(ReducePlan::Const(ExcelValue::Number(*n))),
        Expr::Text(s) => Some(ReducePlan::Const(ExcelValue::Text(s.clone()))),
        Expr::Bool(b) => Some(ReducePlan::Const(ExcelValue::Bool(*b))),
        Expr::Error(e) => Some(ReducePlan::Const(ExcelValue::Error(*e))),
        Expr::Name(n) if names_eq(n, acc_param) => Some(ReducePlan::Acc),
        Expr::Name(n) if names_eq(n, val_param) => Some(ReducePlan::Val),
        Expr::Unary {
            op: UnaryOp::Minus,
            expr,
        } => classify(expr, acc_param, val_param).map(|e| ReducePlan::Neg(Box::new(e))),
        Expr::Unary {
            op: UnaryOp::Plus,
            expr,
        } => classify(expr, acc_param, val_param),
        Expr::Binary { op, left, right } => {
            let fop = match op {
                BinOp::Add => ReduceOp::Add,
                BinOp::Sub => ReduceOp::Sub,
                BinOp::Mul => ReduceOp::Mul,
                BinOp::Div => ReduceOp::Div,
                BinOp::Pow => ReduceOp::Pow,
                BinOp::Concat => ReduceOp::Concat,
                _ => return None,
            };
            Some(ReducePlan::Op {
                op: fop,
                left: Box::new(classify(left, acc_param, val_param)?),
                right: Box::new(classify(right, acc_param, val_param)?),
            })
        }
        _ => None,
    }
}

fn is_acc_val(left: &ReducePlan, right: &ReducePlan) -> bool {
    matches!(
        (left, right),
        (ReducePlan::Acc, ReducePlan::Val) | (ReducePlan::Val, ReducePlan::Acc)
    )
}

/// Production fold: specialized loops for the common acc/value kernels.
pub fn fold_fast(
    initial: Option<&ExcelValue>,
    array: &ExcelValue,
    plan: &ReducePlan,
) -> ExcelValue {
    match plan {
        ReducePlan::Acc => seed_only(initial, array),
        ReducePlan::Val => last_or_seed(initial, array),
        ReducePlan::Const(v) => const_after_calls(initial, array, v),
        ReducePlan::Op {
            op: ReduceOp::Add,
            left,
            right,
        } if is_acc_val(left, right) => fold_add(initial, array),
        ReducePlan::Op {
            op: ReduceOp::Mul,
            left,
            right,
        } if is_acc_val(left, right) => fold_mul(initial, array),
        ReducePlan::Op {
            op: ReduceOp::Concat,
            left,
            right,
        } if matches!(**left, ReducePlan::Acc) && matches!(**right, ReducePlan::Val) => {
            fold_concat(initial, array)
        }
        ReducePlan::Op {
            op: ReduceOp::Sub,
            left,
            right,
        } if matches!(**left, ReducePlan::Acc) && matches!(**right, ReducePlan::Val) => {
            fold_sub(initial, array)
        }
        other => fold_walk(initial, array, other),
    }
}

fn seed_only(initial: Option<&ExcelValue>, array: &ExcelValue) -> ExcelValue {
    if let Some(v) = initial {
        return v.clone();
    }
    let mut first = None;
    for_each_row_major(array, |c| {
        if first.is_none() {
            first = Some(c.clone());
        }
    });
    first.unwrap_or(ExcelValue::Error(ExcelError::Calc))
}

fn last_or_seed(initial: Option<&ExcelValue>, array: &ExcelValue) -> ExcelValue {
    let mut last = initial.cloned();
    let mut any = false;
    for_each_row_major(array, |c| {
        last = Some(c.clone());
        any = true;
    });
    match (any, last, initial) {
        (true, Some(v), _) => v,
        (false, _, Some(v)) => v.clone(),
        (false, _, None) => ExcelValue::Error(ExcelError::Calc),
        (true, None, _) => ExcelValue::Error(ExcelError::Calc),
    }
}

fn const_after_calls(
    initial: Option<&ExcelValue>,
    array: &ExcelValue,
    konst: &ExcelValue,
) -> ExcelValue {
    let n = cell_count(array);
    let calls = if initial.is_some() {
        n
    } else {
        n.saturating_sub(1)
    };
    if calls == 0 {
        return match initial {
            Some(v) => v.clone(),
            None if n == 1 => flatten_row_major(array)
                .into_iter()
                .next()
                .unwrap_or(ExcelValue::Error(ExcelError::Calc)),
            None => ExcelValue::Error(ExcelError::Calc),
        };
    }
    konst.clone()
}

fn fold_add(initial: Option<&ExcelValue>, array: &ExcelValue) -> ExcelValue {
    fold_num2(initial, array, |a, b| ExcelValue::Number(a + b))
}

fn fold_sub(initial: Option<&ExcelValue>, array: &ExcelValue) -> ExcelValue {
    fold_num2(initial, array, |a, b| ExcelValue::Number(a - b))
}

fn fold_mul(initial: Option<&ExcelValue>, array: &ExcelValue) -> ExcelValue {
    fold_num2(initial, array, |a, b| ExcelValue::Number(a * b))
}

fn fold_num2(
    initial: Option<&ExcelValue>,
    array: &ExcelValue,
    f: impl Fn(f64, f64) -> ExcelValue,
) -> ExcelValue {
    let mut acc: Option<ExcelValue> = initial.cloned();
    let mut started = initial.is_some();
    let mut out_err: Option<ExcelError> = None;
    for_each_row_major(array, |v| {
        if out_err.is_some() {
            return;
        }
        if !started {
            acc = Some(v.clone());
            started = true;
            return;
        }
        let Some(cur) = acc.as_ref() else {
            return;
        };
        match (super::coerce::to_number(cur), super::coerce::to_number(v)) {
            (Err(e), _) | (_, Err(e)) => out_err = Some(e),
            (Ok(a), Ok(b)) => acc = Some(f(a, b)),
        }
    });
    if let Some(e) = out_err {
        return ExcelValue::Error(e);
    }
    acc.unwrap_or(ExcelValue::Error(ExcelError::Calc))
}

fn fold_concat(initial: Option<&ExcelValue>, array: &ExcelValue) -> ExcelValue {
    let mut acc: Option<ExcelValue> = initial.cloned();
    let mut started = initial.is_some();
    let mut out_err: Option<ExcelError> = None;
    for_each_row_major(array, |v| {
        if out_err.is_some() {
            return;
        }
        if !started {
            acc = Some(v.clone());
            started = true;
            return;
        }
        let Some(cur) = acc.as_ref() else {
            return;
        };
        match concat(cur, v) {
            ExcelValue::Error(e) => out_err = Some(e),
            other => acc = Some(other),
        }
    });
    if let Some(e) = out_err {
        return ExcelValue::Error(e);
    }
    acc.unwrap_or(ExcelValue::Error(ExcelError::Calc))
}

fn fold_walk(initial: Option<&ExcelValue>, array: &ExcelValue, plan: &ReducePlan) -> ExcelValue {
    let mut acc: Option<ExcelValue> = initial.cloned();
    let mut started = initial.is_some();
    for_each_row_major(array, |v| {
        if !started {
            acc = Some(v.clone());
            started = true;
            return;
        }
        if let Some(cur) = acc.take() {
            acc = Some(eval_fast(plan, &cur, v));
        }
    });
    acc.unwrap_or(ExcelValue::Error(ExcelError::Calc))
}

/// Allocation-heavy baseline: flatten + HashMap bind + walk on every step.
pub fn fold_naive(
    initial: Option<&ExcelValue>,
    array: &ExcelValue,
    plan: &ReducePlan,
) -> ExcelValue {
    let items = flatten_row_major(array);
    let mut acc = match initial {
        Some(v) => v.clone(),
        None => match items.first() {
            Some(v) => v.clone(),
            None => return ExcelValue::Error(ExcelError::Calc),
        },
    };
    let start = if initial.is_some() { 0 } else { 1 };
    for v in &items[start..] {
        let mut env = HashMap::with_capacity(2);
        env.insert("a", acc);
        env.insert("b", v.clone());
        acc = eval_fast_env(plan, &env);
    }
    acc
}

fn eval_fast(plan: &ReducePlan, acc: &ExcelValue, val: &ExcelValue) -> ExcelValue {
    match plan {
        ReducePlan::Const(v) => v.clone(),
        ReducePlan::Acc => acc.clone(),
        ReducePlan::Val => val.clone(),
        ReducePlan::Neg(inner) => match eval_fast(inner, acc, val) {
            ExcelValue::Error(e) => ExcelValue::Error(e),
            v => match super::coerce::to_number(&v) {
                Ok(n) => ExcelValue::Number(-n),
                Err(e) => ExcelValue::Error(e),
            },
        },
        ReducePlan::Op { op, left, right } => {
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
    }
}

fn eval_fast_env(plan: &ReducePlan, env: &HashMap<&str, ExcelValue>) -> ExcelValue {
    let acc = env.get("a").cloned().unwrap_or(ExcelValue::Empty);
    let val = env.get("b").cloned().unwrap_or(ExcelValue::Empty);
    eval_fast(plan, &acc, &val)
}

fn apply_op(op: ReduceOp, left: &ExcelValue, right: &ExcelValue) -> ExcelValue {
    match op {
        ReduceOp::Add => num2(left, right, |a, b| ExcelValue::Number(a + b)),
        ReduceOp::Sub => num2(left, right, |a, b| ExcelValue::Number(a - b)),
        ReduceOp::Mul => num2(left, right, |a, b| ExcelValue::Number(a * b)),
        ReduceOp::Div => super::div(left, right),
        ReduceOp::Pow => excel_pow(left, right),
        ReduceOp::Concat => concat(left, right),
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

/// Evaluator entry: fast kernel when the body is acc/value-only, else AST.
pub(crate) fn apply(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    initial: Option<ExcelValue>,
    array: &ExcelValue,
    acc_param: &str,
    val_param: &str,
    body: &Expr,
) -> Result<ExcelValue, EvalError> {
    if let Some(plan) = classify(body, acc_param, val_param) {
        return Ok(fold_fast(initial.as_ref(), array, &plan));
    }
    apply_naive(ev, ctx, initial, array, acc_param, val_param, body)
}

/// Always walk the AST (bench baseline / seed-compliant-shaped path).
pub(crate) fn apply_naive(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    initial: Option<ExcelValue>,
    array: &ExcelValue,
    acc_param: &str,
    val_param: &str,
    body: &Expr,
) -> Result<ExcelValue, EvalError> {
    apply_general(ev, ctx, initial, array, acc_param, val_param, body)
}

fn apply_general(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    initial: Option<ExcelValue>,
    array: &ExcelValue,
    acc_param: &str,
    val_param: &str,
    body: &Expr,
) -> Result<ExcelValue, EvalError> {
    let mut acc: Option<ExcelValue> = initial;
    let mut started = acc.is_some();
    let base = ctx.locals.len();
    ctx.locals.push((acc_param.to_string(), ExcelValue::Empty));
    ctx.locals.push((val_param.to_string(), ExcelValue::Empty));
    let mut err: Option<EvalError> = None;
    for_each_row_major(array, |v| {
        if err.is_some() {
            return;
        }
        if !started {
            acc = Some(v.clone());
            started = true;
            return;
        }
        let Some(cur) = acc.take() else {
            return;
        };
        ctx.locals[base].1 = cur;
        ctx.locals[base + 1].1 = v.clone();
        match ev.eval_expr(body, ctx) {
            Ok(next) => acc = Some(next),
            Err(e) => err = Some(e),
        }
    });
    ctx.locals.truncate(base);
    if let Some(e) = err {
        return Err(e);
    }
    Ok(acc.unwrap_or(ExcelValue::Error(ExcelError::Calc)))
}

/// `REDUCE([initial], array, LAMBDA(acc, value, body))`.
pub(crate) fn eval(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    let (initial_expr, array_expr, lambda_expr) = match args.len() {
        2 => (None, &args[0], &args[1]),
        3 => {
            let init = if matches!(args[0], Expr::Missing) {
                None
            } else {
                Some(&args[0])
            };
            (init, &args[1], &args[2])
        }
        _ => return Ok(ExcelValue::Error(ExcelError::Value)),
    };

    let initial = if let Some(expr) = initial_expr {
        let v = ev.eval_expr(expr, ctx)?;
        if let ExcelValue::Error(e) = v {
            return Ok(ExcelValue::Error(e));
        }
        Some(v)
    } else {
        None
    };

    let array = ev.eval_expr(array_expr, ctx)?;
    if let ExcelValue::Error(e) = array {
        return Ok(ExcelValue::Error(e));
    }

    let (names, body) = match resolve_lambda_n(lambda_expr, ctx, 2) {
        Ok(l) => l,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    apply(ev, ctx, initial, &array, &names[0], &names[1], &body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::eval_formula_in;
    use crate::parse::parse;
    use xlsx_types::{DefinedName, Workbook};

    fn n(x: f64) -> ExcelValue {
        ExcelValue::Number(x)
    }

    fn add_plan() -> ReducePlan {
        ReducePlan::Op {
            op: ReduceOp::Add,
            left: Box::new(ReducePlan::Acc),
            right: Box::new(ReducePlan::Val),
        }
    }

    fn arr_row(vals: &[f64]) -> ExcelValue {
        ExcelValue::Array(vec![vals.iter().copied().map(n).collect()])
    }

    #[test]
    fn classify_acc_plus_val() {
        let body = parse("a+b").unwrap();
        assert_eq!(classify(&body, "a", "b"), Some(add_plan()));
        assert_eq!(classify(&parse("A+B").unwrap(), "a", "b"), Some(add_plan()));
    }

    #[test]
    fn fast_matches_naive_sum() {
        let plan = add_plan();
        let array = arr_row(&[1.0, 2.0, 3.0]);
        let init = n(0.0);
        assert_eq!(
            fold_fast(Some(&init), &array, &plan),
            fold_naive(Some(&init), &array, &plan)
        );
        assert_eq!(fold_fast(Some(&init), &array, &plan), n(6.0));
        assert_eq!(fold_fast(None, &array, &plan), n(6.0));
    }

    #[test]
    fn omitted_initial_uses_first() {
        let plan = ReducePlan::Op {
            op: ReduceOp::Sub,
            left: Box::new(ReducePlan::Acc),
            right: Box::new(ReducePlan::Val),
        };
        let array = arr_row(&[10.0, 3.0]);
        assert_eq!(fold_fast(None, &array, &plan), n(7.0));
        assert_eq!(fold_fast(Some(&n(0.0)), &array, &plan), n(-13.0));
    }

    #[test]
    fn empty_array_with_and_without_initial() {
        let plan = add_plan();
        let empty = ExcelValue::Array(vec![]);
        assert_eq!(fold_fast(Some(&n(9.0)), &empty, &plan), n(9.0));
        assert_eq!(
            fold_fast(None, &empty, &plan),
            ExcelValue::Error(ExcelError::Calc)
        );
        assert_eq!(
            fold_naive(Some(&n(9.0)), &empty, &plan),
            fold_fast(Some(&n(9.0)), &empty, &plan)
        );
        assert_eq!(
            fold_naive(None, &empty, &plan),
            ExcelValue::Error(ExcelError::Calc)
        );
    }

    #[test]
    fn unused_error_in_array_does_not_surface() {
        let plan = ReducePlan::Acc;
        let array = ExcelValue::Array(vec![vec![
            n(1.0),
            ExcelValue::Error(ExcelError::Div0),
            n(3.0),
        ]]);
        assert_eq!(fold_fast(Some(&n(0.0)), &array, &plan), n(0.0));
    }

    #[test]
    fn formula_sum_and_omitted() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=REDUCE(0,{1,2,3},LAMBDA(a,b,a+b))").unwrap(),
            n(6.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=REDUCE(,{10,3},LAMBDA(a,b,a-b))").unwrap(),
            n(7.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=REDUCE({10,3},LAMBDA(a,b,a-b))").unwrap(),
            n(7.0)
        );
    }

    #[test]
    fn formula_concat_and_product() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=REDUCE(\"\",{\"A\",\"B\",\"C\"},LAMBDA(a,b,a&b))").unwrap(),
            ExcelValue::Text("ABC".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=REDUCE(1,{2,3,4},LAMBDA(a,b,a*b))").unwrap(),
            n(24.0)
        );
    }

    #[test]
    fn formula_named_lambda() {
        let wb = Workbook {
            sheets: vec![xlsx_types::Sheet::new("Sheet1")],
            names: vec![DefinedName {
                name: "Add".into(),
                refers_to: "=LAMBDA(a,b,a+b)".into(),
            }],
        };
        assert_eq!(
            eval_formula_in(&wb, "=REDUCE(0,{1,2},Add)").unwrap(),
            n(3.0)
        );
    }

    #[test]
    fn single_element_omitted_skips_lambda() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=REDUCE(,{42},LAMBDA(a,b,1/0))").unwrap(),
            n(42.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=REDUCE(0,{42},LAMBDA(a,b,1/0))").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
    }

    #[test]
    fn row_major_order() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=REDUCE(\"\",{1,2;3,4},LAMBDA(a,b,a&b))").unwrap(),
            ExcelValue::Text("1234".into())
        );
    }

    #[test]
    fn array_body_is_kept() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=TYPE(REDUCE(1,{2},LAMBDA(a,b,{a,b})))").unwrap(),
            n(64.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=INDEX(REDUCE(1,{2},LAMBDA(a,b,{a,b})),1,2)").unwrap(),
            n(2.0)
        );
    }

    #[test]
    fn bad_lambda_is_value() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=REDUCE(0,{1},LAMBDA(x,1))").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=REDUCE(0,{1},1)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=REDUCE(0,{1})").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn argument_errors_left_to_right() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=REDUCE(1/0,{1},LAMBDA(a,b,a+b))").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=REDUCE(0,1/0,LAMBDA(a,b,a+b))").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
    }

    #[test]
    fn xlfn_prefix() {
        assert_eq!(
            eval_formula_in(
                &Workbook::default(),
                "=_xlfn.REDUCE(0,{1,2},_xlfn.LAMBDA(a,b,a+b))"
            )
            .unwrap(),
            n(3.0)
        );
    }

    #[test]
    fn if_body_via_naive() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=REDUCE(0,{1,-2,3},LAMBDA(a,b,IF(b>0,a+b,a)))").unwrap(),
            n(4.0)
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
            target: EvalTarget::formula("=REDUCE(0,{1,-2,3},LAMBDA(a,b,IF(b>0,a+b,a)))"),
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
        let body = parse("IF(b>0,a+b,a)").unwrap();
        let array = arr_row(&[1.0, -2.0, 3.0]);
        let via_naive = apply_naive(&ev, &mut ctx, Some(n(0.0)), &array, "a", "b", &body).unwrap();
        assert_eq!(via_naive, n(4.0));
    }
}
