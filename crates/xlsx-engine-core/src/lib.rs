//! Real Excel-compatible calculation core.
//!
//! Architecture:
//! - [`parse`] — tokenizer + recursive-descent AST
//! - [`eval`] — workbook-backed walker
//! - [`eval::coerce`] / [`eval::compare`] / [`eval::empty`] — quirk modules
//! - [`eval::functions`] — worksheet functions used by the expanded corpus
//! - [`dates::weekday`] — O(1) Excel `WEEKDAY` on the date serial
//!
//! This crate depends only on [`xlsx_types`]. It never reads fixture expected
//! values; the verification gate (`xlsx-verify`) is the only judge.

pub mod ast;
pub mod dates;
pub mod eval;
pub mod parse;

pub use ast::{BinOp, Expr, UnaryOp};
pub use dates::{weekday as excel_weekday, weekday_naive as excel_weekday_naive};
pub use eval::{eval_formula_in, Evaluator};
pub use parse::parse;

use xlsx_types::{Candidate, EvalError, EvalSpec, ExcelValue};

/// Production calculation candidate (`calc-core`).
#[derive(Clone, Debug, Default)]
pub struct CalcCoreEngine;

impl CalcCoreEngine {
    pub fn new() -> Self {
        Self
    }
}

impl Candidate for CalcCoreEngine {
    fn id(&self) -> &str {
        "calc-core"
    }

    fn evaluate(&self, spec: &EvalSpec) -> Result<ExcelValue, EvalError> {
        Evaluator::new().eval_spec(spec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xlsx_types::{Cell, EvalTarget, ExcelValue, Sheet, Workbook};

    #[test]
    fn candidate_id() {
        assert_eq!(CalcCoreEngine::new().id(), "calc-core");
    }

    #[test]
    fn evaluates_stored_formula_cell() {
        let mut sheet = Sheet::new("Sheet1");
        sheet
            .cells
            .insert("A1".into(), Cell::formula("=A2+1", None));
        sheet
            .cells
            .insert("A2".into(), Cell::value(ExcelValue::Number(4.0)));
        let spec = EvalSpec {
            case_id: "cell.formula".into(),
            workbook: Workbook {
                sheets: vec![sheet],
                names: vec![],
            },
            target: EvalTarget::Cell {
                cell: xlsx_types::CellRef::parse("Sheet1!A1").unwrap(),
            },
            options: Default::default(),
        };
        let v = CalcCoreEngine::new().evaluate(&spec).unwrap();
        assert_eq!(v, ExcelValue::Number(5.0));
    }
}
