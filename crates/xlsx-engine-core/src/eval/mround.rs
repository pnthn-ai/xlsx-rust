//! Excel `MROUND(number, multiple)` — nearest multiple, half away from zero.
//!
//! Thin calc-core wrapper around the shared [`xlsx_types::excel_mround`]
//! kernel. Does not read fixture goldens.

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{excel_mround, EvalError, ExcelError, ExcelValue};

/// `MROUND(number, multiple)` — exactly two arguments.
pub(crate) fn fn_mround(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() != 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let n = match coerce::to_number(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let m = match coerce::to_number(&ev.eval_scalar(&args[1], ctx)?) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    Ok(match excel_mround(n, m) {
        Ok(v) => ExcelValue::Number(v),
        Err(e) => ExcelValue::Error(e),
    })
}

#[cfg(test)]
mod tests {
    use super::super::eval_formula_in;
    use xlsx_types::{ExcelError, ExcelValue, Workbook};

    #[test]
    fn microsoft_and_arity() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=MROUND(10, 3)").unwrap(),
            ExcelValue::Number(9.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=MROUND(-10, -3)").unwrap(),
            ExcelValue::Number(-9.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=MROUND(5, -2)").unwrap(),
            ExcelValue::Error(ExcelError::Num)
        );
        assert_eq!(
            eval_formula_in(&wb, "=MROUND()").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=MROUND(1)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=MROUND(1, 2, 3)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn zero_multiple_and_coerce() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=MROUND(10, 0)").unwrap(),
            ExcelValue::Number(0.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=MROUND(TRUE, 1)").unwrap(),
            ExcelValue::Number(1.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=MROUND(\"10\", 3)").unwrap(),
            ExcelValue::Number(9.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=MROUND(1/0, NA())").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
    }
}
