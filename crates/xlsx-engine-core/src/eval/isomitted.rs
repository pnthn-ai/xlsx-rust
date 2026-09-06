//! Excel `ISOMITTED(argument)`.
//!
//! Detects a **missing LAMBDA parameter** (or an omitted call slot). This is
//! not `ISBLANK`: a provided blank cell, `0`, `""`, or error is still
//! present, so the result is FALSE.
//!
//! Documented Excel quirks this module implements:
//!
//! - Arity is exactly one. `ISOMITTED()` / `ISOMITTED(a,b)` → `#VALUE!`.
//! - The argument is **not evaluated**. An omitted name is TRUE even though
//!   ordinary evaluation would treat it as empty/`0`. A provided `#DIV/0!`
//!   is FALSE (the slot was filled).
//! - `ISOMITTED` of a non-name expression (literal, call, cell ref) is
//!   FALSE — the argument text was supplied.
//! - Used with immediately-invoked `LAMBDA(...)(args)` or a defined-name
//!   LAMBDA. Microsoft's documented form is
//!   `LAMBDA(x,y,IF(ISOMITTED(y),"Missing second argument",x+y))(1,)`.
//!
//! [`is_omitted`] is a reverse scan of the local bind stack (no allocation).
//! [`is_omitted_naive`] builds a HashMap of omitted flags on every call —
//! same answers, more allocation. Used as the bench "before".

use super::makearray::{lookup_omitted, strip_xlpm, Local};
use crate::ast::Expr;
use std::collections::HashMap;
use xlsx_types::ExcelError;

/// TRUE when `arg` is an omitted LAMBDA parameter or a missing call slot.
pub fn is_omitted(arg: &Expr, locals: &[Local]) -> bool {
    match arg {
        Expr::Missing => true,
        Expr::Name(n) => lookup_omitted(locals, n).unwrap_or(false),
        _ => false,
    }
}

/// Allocation-heavy baseline: HashMap bind + lookup on every call.
pub fn is_omitted_naive(arg: &Expr, locals: &[Local]) -> bool {
    match arg {
        Expr::Missing => true,
        Expr::Name(n) => {
            let mut map = HashMap::with_capacity(locals.len());
            for loc in locals {
                map.insert(strip_xlpm(&loc.name).to_ascii_uppercase(), loc.omitted);
            }
            map.get(&strip_xlpm(n).to_ascii_uppercase())
                .copied()
                .unwrap_or(false)
        }
        _ => false,
    }
}

/// Worksheet-function wrapper: arity then [`is_omitted`].
pub fn eval(args: &[Expr], locals: &[Local]) -> Result<bool, ExcelError> {
    if args.len() != 1 {
        return Err(ExcelError::Value);
    }
    Ok(is_omitted(&args[0], locals))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::eval_formula_in;
    use crate::parse::parse;
    use xlsx_types::{DefinedName, ExcelError, ExcelValue, Workbook};

    fn n(x: f64) -> ExcelValue {
        ExcelValue::Number(x)
    }

    #[test]
    fn kernel_missing_slot_and_name() {
        let missing = Expr::Missing;
        assert!(is_omitted(&missing, &[]));
        assert!(is_omitted_naive(&missing, &[]));
        assert!(!is_omitted(&Expr::Number(1.0), &[]));

        let locals = vec![Local::provided("x", n(1.0)), Local::missing("y")];
        let y = parse("y").unwrap();
        let x = parse("x").unwrap();
        assert!(is_omitted(&y, &locals));
        assert!(is_omitted_naive(&y, &locals));
        assert!(!is_omitted(&x, &locals));
        assert!(!is_omitted_naive(&x, &locals));
    }

    #[test]
    fn iife_omitted_trailing_is_true() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=LAMBDA(x,y,ISOMITTED(y))(1,)").unwrap(),
            ExcelValue::Bool(true)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LAMBDA(x,y,ISOMITTED(y))(1)").unwrap(),
            ExcelValue::Bool(true)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LAMBDA(x,y,ISOMITTED(y))(1,2)").unwrap(),
            ExcelValue::Bool(false)
        );
    }

    #[test]
    fn microsoft_missing_second_argument() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(
                &wb,
                r#"=LAMBDA(x,y,IF(ISOMITTED(y),"Missing second argument",x+y))(1,)"#
            )
            .unwrap(),
            ExcelValue::Text("Missing second argument".into())
        );
        assert_eq!(
            eval_formula_in(
                &wb,
                r#"=LAMBDA(x,y,IF(ISOMITTED(y),"Missing second argument",x+y))(1,2)"#
            )
            .unwrap(),
            n(3.0)
        );
    }

    #[test]
    fn provided_blank_is_not_omitted() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=LAMBDA(x,ISOMITTED(x))(A1)").unwrap(),
            ExcelValue::Bool(false)
        );
        assert_eq!(
            eval_formula_in(&wb, "=ISOMITTED(A1)").unwrap(),
            ExcelValue::Bool(false)
        );
    }

    #[test]
    fn named_lambda_omitted_optional() {
        let wb = Workbook {
            sheets: vec![xlsx_types::Sheet::new("Sheet1")],
            names: vec![DefinedName {
                name: "HasOpt".into(),
                refers_to: "=LAMBDA(a,b,ISOMITTED(b))".into(),
            }],
        };
        assert_eq!(
            eval_formula_in(&wb, "=HasOpt(1)").unwrap(),
            ExcelValue::Bool(true)
        );
        assert_eq!(
            eval_formula_in(&wb, "=HasOpt(1,0)").unwrap(),
            ExcelValue::Bool(false)
        );
    }

    #[test]
    fn arity_and_outside_lambda() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=ISOMITTED()").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=ISOMITTED(1,2)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=ISOMITTED(1)").unwrap(),
            ExcelValue::Bool(false)
        );
        assert_eq!(
            eval_formula_in(&wb, "=ISOMITTED(#N/A)").unwrap(),
            ExcelValue::Bool(false)
        );
    }
}
