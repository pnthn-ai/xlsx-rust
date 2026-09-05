//! Built-in worksheet functions implemented for the seed corpus and as a
//! foundation for later library work.
//!
//! Unknown names return `#NAME?` (an Excel value, not [`EvalError`]).

use super::{coerce, compare, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

pub(crate) fn dispatch(
    ev: &Evaluator,
    name: &str,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    match name.to_ascii_uppercase().as_str() {
        "SUM" => fn_sum(ev, args, ctx),
        "IF" => fn_if(ev, args, ctx),
        "IFERROR" => fn_iferror(ev, args, ctx),
        "VLOOKUP" => fn_vlookup(ev, args, ctx),
        "ABS" => fn_abs(ev, args, ctx),
        "N" => fn_n(ev, args, ctx),
        "ISBLANK" => fn_is(ev, args, ctx, |v| matches!(v, ExcelValue::Empty)),
        "ISNUMBER" => fn_is(ev, args, ctx, |v| matches!(v, ExcelValue::Number(_))),
        "ISTEXT" => fn_is(ev, args, ctx, |v| matches!(v, ExcelValue::Text(_))),
        "ISERROR" => fn_is(ev, args, ctx, |v| matches!(v, ExcelValue::Error(_))),
        "TRUE" => Ok(ExcelValue::Bool(true)),
        "FALSE" => Ok(ExcelValue::Bool(false)),
        _ => Ok(ExcelValue::Error(ExcelError::Name)),
    }
}

/// `SUM` skips blanks. Text and logicals are skipped in references / arrays
/// but coerced when passed as scalar literals (`SUM(TRUE)` = 1).
fn fn_sum(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    let mut acc = 0.0;
    for arg in args {
        let from_range = arg.is_reference();
        let v = ev.eval_expr(arg, ctx)?;
        if let Some(err) = add_sum(&mut acc, &v, from_range) {
            return Ok(ExcelValue::Error(err));
        }
    }
    Ok(ExcelValue::Number(acc))
}

fn add_sum(acc: &mut f64, v: &ExcelValue, from_range: bool) -> Option<ExcelError> {
    match (v, from_range) {
        (ExcelValue::Error(e), _) => Some(*e),
        (ExcelValue::Number(n), _) => {
            *acc += *n;
            None
        }
        (ExcelValue::Empty, _) => None,
        (ExcelValue::Bool(b), false) => {
            *acc += if *b { 1.0 } else { 0.0 };
            None
        }
        (ExcelValue::Bool(_), true) => None,
        (ExcelValue::Text(s), false) => match coerce::parse_numeric_text(s) {
            Ok(n) => {
                *acc += n;
                None
            }
            Err(e) => Some(e),
        },
        (ExcelValue::Text(_), true) => None,
        (ExcelValue::Array(rows), _) => {
            for row in rows {
                for c in row {
                    if let Some(e) = add_sum(acc, c, true) {
                        return Some(e);
                    }
                }
            }
            None
        }
    }
}

/// `IF` short-circuits: the unused branch is not evaluated.
fn fn_if(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.is_empty() {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let cond = coerce::scalarize(ev.eval_expr(&args[0], ctx)?);
    let truth = match coerce::to_logical(&cond) {
        Ok(b) => b,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    if args.len() == 1 {
        return Ok(ExcelValue::Bool(truth));
    }
    if truth {
        ev.eval_expr(&args[1], ctx)
    } else if args.len() >= 3 {
        ev.eval_expr(&args[2], ctx)
    } else {
        Ok(ExcelValue::Bool(false))
    }
}

fn fn_iferror(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let v = ev.eval_expr(&args[0], ctx)?;
    if v.is_error() {
        ev.eval_expr(&args[1], ctx)
    } else {
        Ok(v)
    }
}

fn fn_vlookup(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() < 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let lookup = coerce::scalarize(ev.eval_expr(&args[0], ctx)?);
    if let ExcelValue::Error(e) = lookup {
        return Ok(ExcelValue::Error(e));
    }
    let table = ev.eval_expr(&args[1], ctx)?;
    let col = coerce::scalarize(ev.eval_expr(&args[2], ctx)?);
    let approx = if args.len() >= 4 {
        match coerce::to_logical(&coerce::scalarize(ev.eval_expr(&args[3], ctx)?)) {
            Ok(b) => b,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        true
    };
    let col_n = match coerce::to_number(&col) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    if col_n < 1.0 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let col_idx = col_n as usize;
    let rows = match table {
        ExcelValue::Array(rows) => rows,
        other => vec![vec![other]],
    };
    if rows.is_empty() {
        return Ok(ExcelValue::Error(ExcelError::Na));
    }
    let width = rows[0].len();
    if col_idx > width {
        return Ok(ExcelValue::Error(ExcelError::Ref));
    }
    if !approx {
        for row in &rows {
            if compare::equal(&lookup, &row[0]) {
                return Ok(row[col_idx - 1].clone());
            }
        }
        return Ok(ExcelValue::Error(ExcelError::Na));
    }
    // Approximate match: last row whose first column is <= lookup (numeric).
    // Unsorted tables produce Excel's well-known wrong answers.
    let mut found: Option<&Vec<ExcelValue>> = None;
    for row in &rows {
        if let (Ok(lv), Ok(kv)) = (coerce::to_number(&lookup), coerce::to_number(&row[0])) {
            if kv <= lv {
                found = Some(row);
            }
        }
    }
    match found {
        Some(row) => Ok(row[col_idx - 1].clone()),
        None => Ok(ExcelValue::Error(ExcelError::Na)),
    }
}

fn fn_abs(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 1 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let v = coerce::scalarize(ev.eval_expr(&args[0], ctx)?);
    match coerce::to_number(&v) {
        Ok(n) => Ok(ExcelValue::Number(n.abs())),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn fn_n(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 1 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let v = coerce::scalarize(ev.eval_expr(&args[0], ctx)?);
    Ok(match v {
        ExcelValue::Number(n) => ExcelValue::Number(n),
        ExcelValue::Bool(true) => ExcelValue::Number(1.0),
        ExcelValue::Bool(false) | ExcelValue::Empty | ExcelValue::Text(_) => {
            ExcelValue::Number(0.0)
        }
        ExcelValue::Error(e) => ExcelValue::Error(e),
        ExcelValue::Array(_) => ExcelValue::Number(0.0),
    })
}

fn fn_is(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
    pred: impl Fn(&ExcelValue) -> bool,
) -> Result<ExcelValue, EvalError> {
    if args.len() != 1 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let v = coerce::scalarize(ev.eval_expr(&args[0], ctx)?);
    Ok(ExcelValue::Bool(pred(&v)))
}
