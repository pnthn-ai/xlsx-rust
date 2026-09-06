//! Excel `LET(name1, value1, [name2, value2, …], calculation)`.
//!
//! Binds formula-scoped names onto the same [`Ctx::locals`](super::Ctx)
//! stack that MAKEARRAY / LAMBDA parameters use. Each value is evaluated
//! **once**, in order, so later names can see earlier ones. The last
//! argument is the calculation.
//!
//! Documented Excel quirks this module implements:
//!
//! - Arity must be odd and at least 3 (one name/value pair + calculation).
//!   Even / too-few / more than [`MAX_PAIRS`] pairs → `#VALUE!`.
//! - Name arguments are **not** evaluated. They must parse as a name
//!   (not a cell, bool, number, or call). Invalid names, including `R` /
//!   `C` (R1C1 conflict), are `#NAME?`.
//! - Names are case-insensitive and accept the `_xlpm.` prefix Excel
//!   writes into workbook XML.
//! - Values are eager: an unused name is still computed. An error in a
//!   value is **bound** (so `IFERROR` / `ISERROR` can see it). The
//!   calculation decides whether that error surfaces.
//! - A later pair with the same name shadows the earlier one (last wins).
//!   Nested `LET` and LAMBDA parameters use the same last-wins lookup.
//! - `LET` names are values, not worksheet references: `SUM(x)` of a
//!   bound logical uses scalar `SUM` rules (`SUM(TRUE)` is 1).
//!
//! [`eval_fast`] evaluates an arithmetic calculation from already-bound
//! values by index. [`eval_naive`] does the same through a HashMap clone
//! on every name leaf — same answers, more allocation. Used as the bench
//! "before".
//!
//! ## Limits
//!
//! - Names that tokenize as A1 refs (`A1`, `X1`) cannot be bound — the
//!   parser emits a cell, and that is `#NAME?`.
//! - There is no first-class function value. `LET(f, LAMBDA(...), f)`
//!   binds `#CALC!`. Immediately-invoked `LAMBDA(...)(args)` is not parsed.
//! - Array results are values; occupied neighbors never yield `#SPILL!`.

use super::makearray::{lookup_binding, names_eq, strip_xlpm, Local};
use super::{excel_pow, Ctx, Evaluator};
use crate::ast::{BinOp, Expr, UnaryOp};
use std::collections::HashMap;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

/// Excel's documented cap on name/value pairs inside one `LET`.
pub const MAX_PAIRS: usize = 126;

/// A calculation that depends only on already-bound names and literals.
#[derive(Clone, Debug, PartialEq)]
pub enum FastCalc {
    Const(ExcelValue),
    /// Index into the binding snapshot (last-wins already resolved).
    Name(usize),
    Neg(Box<FastCalc>),
    Op {
        op: FastOp,
        left: Box<FastCalc>,
        right: Box<FastCalc>,
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

/// Odd arity, at least one pair, at most [`MAX_PAIRS`] pairs.
pub fn arity_ok(len: usize) -> bool {
    len >= 3 && len % 2 == 1 && (len - 1) / 2 <= MAX_PAIRS
}

/// True when `name` is currently bound on the locals stack.
pub fn is_bound(locals: &[Local], name: &str) -> bool {
    lookup_binding(locals, name).is_some()
}

/// Extract and validate a LET name argument.
pub fn bind_name(expr: &Expr) -> Result<String, ExcelError> {
    match expr {
        Expr::Name(n) => bind_name_str(n),
        _ => Err(ExcelError::Name),
    }
}

/// Validate a name string (shared with seed-compliant, which has its own AST).
pub fn bind_name_str(name: &str) -> Result<String, ExcelError> {
    let n = strip_xlpm(name);
    if is_valid_let_name(n) {
        Ok(n.to_string())
    } else {
        Err(ExcelError::Name)
    }
}

/// Name-manager rules used by Excel `LET`.
pub fn is_valid_let_name(name: &str) -> bool {
    let n = strip_xlpm(name);
    if n.is_empty() || n.len() > 255 {
        return false;
    }
    if n.eq_ignore_ascii_case("R")
        || n.eq_ignore_ascii_case("C")
        || n.eq_ignore_ascii_case("TRUE")
        || n.eq_ignore_ascii_case("FALSE")
    {
        return false;
    }
    let mut chars = n.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_' || first == '\\') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

fn canonical(name: &str) -> String {
    strip_xlpm(name).to_ascii_uppercase()
}

/// Classify a body that only names the given bindings (plus literals).
pub fn classify(body: &Expr, names: &[String]) -> Option<FastCalc> {
    match body {
        Expr::Number(n) => Some(FastCalc::Const(ExcelValue::Number(*n))),
        Expr::Text(s) => Some(FastCalc::Const(ExcelValue::Text(s.clone()))),
        Expr::Bool(b) => Some(FastCalc::Const(ExcelValue::Bool(*b))),
        Expr::Error(e) => Some(FastCalc::Const(ExcelValue::Error(*e))),
        Expr::Name(n) => names
            .iter()
            .rposition(|b| names_eq(b, n))
            .map(FastCalc::Name),
        Expr::Unary {
            op: UnaryOp::Minus,
            expr,
        } => classify(expr, names).map(|e| FastCalc::Neg(Box::new(e))),
        Expr::Unary {
            op: UnaryOp::Plus,
            expr,
        } => classify(expr, names),
        Expr::Unary {
            op: UnaryOp::Percent,
            expr,
        } => classify(expr, names).map(|inner| FastCalc::Op {
            op: FastOp::Div,
            left: Box::new(inner),
            right: Box::new(FastCalc::Const(ExcelValue::Number(100.0))),
        }),
        Expr::Binary { op, left, right } => {
            let fop = match op {
                BinOp::Add => FastOp::Add,
                BinOp::Sub => FastOp::Sub,
                BinOp::Mul => FastOp::Mul,
                BinOp::Div => FastOp::Div,
                BinOp::Pow => FastOp::Pow,
                _ => return None,
            };
            Some(FastCalc::Op {
                op: fop,
                left: Box::new(classify(left, names)?),
                right: Box::new(classify(right, names)?),
            })
        }
        _ => None,
    }
}

/// Production walk: index into the binding snapshot, no HashMap.
pub fn eval_fast(calc: &FastCalc, values: &[ExcelValue]) -> ExcelValue {
    match calc {
        FastCalc::Const(v) => v.clone(),
        FastCalc::Name(i) => values.get(*i).cloned().unwrap_or(ExcelValue::Empty),
        FastCalc::Neg(inner) => match eval_fast(inner, values) {
            ExcelValue::Error(e) => ExcelValue::Error(e),
            v => match super::coerce::to_number(&v) {
                Ok(n) => ExcelValue::Number(-n),
                Err(e) => ExcelValue::Error(e),
            },
        },
        FastCalc::Op { op, left, right } => {
            let l = eval_fast(left, values);
            if let ExcelValue::Error(e) = l {
                return ExcelValue::Error(e);
            }
            let r = eval_fast(right, values);
            if let ExcelValue::Error(e) = r {
                return ExcelValue::Error(e);
            }
            apply_op(*op, &l, &r)
        }
    }
}

/// Allocation-heavy baseline: HashMap bind + clone on every name leaf.
pub fn eval_naive(calc: &FastCalc, names: &[String], values: &[ExcelValue]) -> ExcelValue {
    let mut env = HashMap::with_capacity(names.len());
    for (n, v) in names.iter().zip(values.iter()) {
        env.insert(canonical(n), v.clone());
    }
    eval_naive_env(calc, names, &env)
}

fn eval_naive_env(
    calc: &FastCalc,
    names: &[String],
    env: &HashMap<String, ExcelValue>,
) -> ExcelValue {
    match calc {
        FastCalc::Const(v) => v.clone(),
        FastCalc::Name(i) => {
            let key = names.get(*i).map(|n| canonical(n)).unwrap_or_default();
            env.get(&key).cloned().unwrap_or(ExcelValue::Empty)
        }
        FastCalc::Neg(inner) => match eval_naive_env(inner, names, env) {
            ExcelValue::Error(e) => ExcelValue::Error(e),
            v => match super::coerce::to_number(&v) {
                Ok(n) => ExcelValue::Number(-n),
                Err(e) => ExcelValue::Error(e),
            },
        },
        FastCalc::Op { op, left, right } => {
            let l = eval_naive_env(left, names, env);
            if let ExcelValue::Error(e) = l {
                return ExcelValue::Error(e);
            }
            let r = eval_naive_env(right, names, env);
            if let ExcelValue::Error(e) = r {
                return ExcelValue::Error(e);
            }
            apply_op(*op, &l, &r)
        }
    }
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

/// Bind pairs onto `ctx.locals`, then evaluate the calculation.
///
/// Arithmetic calculations over bound names take the specialized kernel;
/// anything else walks the AST with the shared locals stack.
pub(crate) fn apply(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if !arity_ok(args.len()) {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let base = ctx.locals.len();
    let mut i = 0;
    while i + 1 < args.len() {
        let name = match bind_name(&args[i]) {
            Ok(n) => n,
            Err(e) => {
                ctx.locals.truncate(base);
                return Ok(ExcelValue::Error(e));
            }
        };
        let value = match ev.eval_expr(&args[i + 1], ctx) {
            Ok(v) => v,
            Err(e) => {
                ctx.locals.truncate(base);
                return Err(e);
            }
        };
        ctx.locals.push(Local::provided(name, value));
        i += 2;
    }
    let names: Vec<String> = ctx.locals.iter().map(|l| l.name.clone()).collect();
    let values: Vec<ExcelValue> = ctx.locals.iter().map(|l| l.value.clone()).collect();
    if let Some(fast) = classify(&args[i], &names) {
        let out = eval_fast(&fast, &values);
        ctx.locals.truncate(base);
        return Ok(out);
    }
    let out = ev.eval_expr(&args[i], ctx);
    ctx.locals.truncate(base);
    out
}

/// Always walk the calculation AST (bench / seed-shaped path).
#[allow(dead_code)]
pub(crate) fn apply_naive(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if !arity_ok(args.len()) {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let base = ctx.locals.len();
    let mut i = 0;
    while i + 1 < args.len() {
        let name = match bind_name(&args[i]) {
            Ok(n) => n,
            Err(e) => {
                ctx.locals.truncate(base);
                return Ok(ExcelValue::Error(e));
            }
        };
        let value = match ev.eval_expr(&args[i + 1], ctx) {
            Ok(v) => v,
            Err(e) => {
                ctx.locals.truncate(base);
                return Err(e);
            }
        };
        ctx.locals.push(Local::provided(name, value));
        i += 2;
    }
    let out = ev.eval_expr(&args[i], ctx);
    ctx.locals.truncate(base);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::eval_formula_in;
    use crate::parse::parse;
    use xlsx_types::{Cell, CellAddr, DefinedName, Sheet, Workbook};

    fn n(x: f64) -> ExcelValue {
        ExcelValue::Number(x)
    }

    fn names(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn arity_and_name_rules() {
        assert!(!arity_ok(0));
        assert!(!arity_ok(2));
        assert!(arity_ok(3));
        assert!(!arity_ok(4));
        assert!(arity_ok(5));
        assert!(arity_ok(1 + MAX_PAIRS * 2));
        assert!(!arity_ok(1 + (MAX_PAIRS + 1) * 2));
        assert!(is_valid_let_name("foo"));
        assert!(is_valid_let_name("_x"));
        assert!(is_valid_let_name("x.y"));
        assert!(is_valid_let_name("_xlpm.total"));
        assert!(!is_valid_let_name("R"));
        assert!(!is_valid_let_name("c"));
        assert!(!is_valid_let_name("TRUE"));
        assert!(!is_valid_let_name(""));
        assert_eq!(bind_name(&parse("foo").unwrap()).unwrap(), "foo");
        assert_eq!(bind_name(&parse("A1").unwrap()), Err(ExcelError::Name));
        assert_eq!(bind_name(&parse("TRUE").unwrap()), Err(ExcelError::Name));
        assert_eq!(bind_name(&parse("1").unwrap()), Err(ExcelError::Name));
    }

    #[test]
    fn fast_matches_naive_mul_add() {
        let names = names(&["x", "y"]);
        let values = vec![n(3.0), n(4.0)];
        let calc = classify(&parse("x*y+x").unwrap(), &names).unwrap();
        assert_eq!(eval_fast(&calc, &values), n(15.0));
        assert_eq!(
            eval_fast(&calc, &values),
            eval_naive(&calc, &names, &values)
        );
    }

    #[test]
    fn classify_last_wins_and_casefold() {
        let names = names(&["x", "X"]);
        let values = vec![n(1.0), n(9.0)];
        let calc = classify(&parse("x").unwrap(), &names).unwrap();
        assert_eq!(eval_fast(&calc, &values), n(9.0));
        assert_eq!(
            eval_fast(&calc, &values),
            eval_naive(&calc, &names, &values)
        );
    }

    #[test]
    fn formula_basic_and_pairs() {
        let wb = Workbook::default();
        assert_eq!(eval_formula_in(&wb, "=LET(x, 1, x+1)").unwrap(), n(2.0));
        assert_eq!(
            eval_formula_in(&wb, "=LET(x, 2, y, x*3, y+1)").unwrap(),
            n(7.0)
        );
        assert_eq!(eval_formula_in(&wb, "=LET(x, 5, 9)").unwrap(), n(9.0));
        assert_eq!(eval_formula_in(&wb, "=LET(Foo, 10, foo)").unwrap(), n(10.0));
    }

    #[test]
    fn formula_errors_and_arity() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=LET()").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LET(x, 1)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LET(c, 1, c)").unwrap(),
            ExcelValue::Error(ExcelError::Name)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LET(A1, 5, 1)").unwrap(),
            ExcelValue::Error(ExcelError::Name)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LET(x, 1, y)").unwrap(),
            ExcelValue::Error(ExcelError::Name)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LET(x, 1/0, x)").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(eval_formula_in(&wb, "=LET(x, 1/0, 1)").unwrap(), n(1.0));
        assert_eq!(
            eval_formula_in(&wb, "=LET(x, 1/0, IFERROR(x, 0))").unwrap(),
            n(0.0)
        );
    }

    #[test]
    fn nested_and_shadow() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=LET(x, 1, LET(x, 2, x)+x)").unwrap(),
            n(3.0)
        );
        assert_eq!(eval_formula_in(&wb, "=LET(x, 1, x, 2, x)").unwrap(), n(2.0));
        assert_eq!(
            eval_formula_in(&wb, "=LET(x, 1, x)+LET(y, 2, y)").unwrap(),
            n(3.0)
        );
    }

    #[test]
    fn shadows_defined_name() {
        let wb = Workbook {
            sheets: vec![Sheet::new("Sheet1")],
            names: vec![DefinedName {
                name: "Total".into(),
                refers_to: "10".into(),
            }],
        };
        assert_eq!(
            eval_formula_in(&wb, "=LET(Total, 3, Total+1)").unwrap(),
            n(4.0)
        );
        assert_eq!(eval_formula_in(&wb, "=Total").unwrap(), n(10.0));
    }

    #[test]
    fn array_bind_and_sum_scalar_logical() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=LET(a, {1,2,3}, SUM(a))").unwrap(),
            n(6.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LET(b, TRUE, SUM(b))").unwrap(),
            n(1.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LET(a, {1,2;3,4}, INDEX(a,2,1))").unwrap(),
            n(3.0)
        );
    }

    #[test]
    fn makearray_composition() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=LET(n, 2, INDEX(MAKEARRAY(n,n,LAMBDA(r,c,r*c)),2,2))").unwrap(),
            n(4.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=INDEX(MAKEARRAY(2,2,LAMBDA(r,c,LET(x,r*c,x+1))),1,2)").unwrap(),
            n(3.0)
        );
    }

    #[test]
    fn xlfn_and_xlpm() {
        assert_eq!(
            eval_formula_in(&Workbook::default(), "=_xlfn.LET(_xlpm.x, 1, _xlpm.x+1)").unwrap(),
            n(2.0)
        );
    }

    #[test]
    fn no_scope_leak_and_self_ref() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=LET(x, 1, x)+x").unwrap(),
            ExcelValue::Error(ExcelError::Name)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LET(x, x+1, x)").unwrap(),
            ExcelValue::Error(ExcelError::Name)
        );
    }

    #[test]
    fn blank_cell_value() {
        let mut sheet = Sheet::new("Sheet1");
        sheet.insert(CellAddr::new(0, 0), Cell::value(ExcelValue::Empty));
        let wb = Workbook {
            sheets: vec![sheet],
            names: vec![],
        };
        assert_eq!(eval_formula_in(&wb, "=LET(x, A1, x+1)").unwrap(), n(1.0));
    }

    #[test]
    fn apply_naive_matches_apply_for_if() {
        use crate::eval::randarray::XorShift64;
        use crate::eval::{Ctx, Evaluator};
        use std::collections::HashSet;
        use xlsx_types::{EvalSpec, EvalTarget};

        let spec = EvalSpec {
            case_id: "naive".into(),
            workbook: Workbook::default(),
            target: EvalTarget::formula("=LET(x, 2, IF(x>1, x*3, 0))"),
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
            rng: XorShift64::from_eval_options(&spec.options),
        };
        let ast = parse("=LET(x, 2, IF(x>1, x*3, 0))").unwrap();
        let Expr::Call { args, .. } = ast else {
            panic!("expected LET call");
        };
        let via_naive = apply_naive(&ev, &args, &mut ctx).unwrap();
        assert_eq!(via_naive, n(6.0));
        assert_eq!(apply(&ev, &args, &mut ctx).unwrap(), n(6.0));
    }
}
