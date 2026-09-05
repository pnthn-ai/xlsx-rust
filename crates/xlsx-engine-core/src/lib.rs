//! Real Excel-compatible calculation core.
//!
//! Architecture:
//! - [`parse`] — tokenizer + recursive-descent AST
//! - [`eval`] — workbook-backed walker
//! - [`eval::coerce`] / [`eval::compare`] / [`eval::empty`] — quirk modules
//! - [`eval::functions`] — worksheet functions used by the expanded corpus
//! - [`text_format`] — Excel `TEXT` for a documented number/date format subset
//! - [`eval::functions`] also dispatches `SUMIF` / `COUNTIF` / `SUMPRODUCT` / `SUBSTITUTE`
//! - [`eval::concat`] — Excel `CONCAT` (range/array flatten + 32767 cap)
//! - [`dates::weekday`] — O(1) Excel `WEEKDAY` on the date serial
//! - [`eval::switch`] — Excel `SWITCH` (exact `=` match, short-circuit vs `IF`)
//! - [`eval::ifs`] — `IFS` pair-selection kernel (eager; no-match `#N/A`)
//! - [`eval::unique`] — `UNIQUE` dynamic-array kernel (hash distinctness)
//! - [`eval::filter`] — `FILTER` mask/select kernel (`#CALC!` / `if_empty`)
//!
//! This crate depends only on [`xlsx_types`]. It never reads fixture expected
//! values; the verification gate (`xlsx-verify`) is the only judge.

pub mod ast;
pub mod dates;
pub mod eval;
pub mod parse;
pub mod text_format;

pub use dates::workday_serial;

pub use ast::{BinOp, Expr, UnaryOp};
pub use eval::substitute::{
    substitute as excel_substitute, substitute_naive as excel_substitute_naive,
};
pub use eval::sumproduct::{product_sum, product_sum_naive, product_sum_packed};
pub use eval::{eval_formula_in, eval_sumif_materialized, Evaluator};
pub use eval::replace::{replace as excel_replace, replace_naive as excel_replace_naive};
pub use eval::find::{find as excel_find, find_naive as excel_find_naive};
pub use eval::textjoin::{
    eval_textjoin_formula, textjoin_naive_join, TextJoinBuilder, TEXTJOIN_MAX_CHARS,
pub use eval::round::{
    rounddown as excel_rounddown, rounddown_naive as excel_rounddown_naive,
    roundup as excel_roundup, roundup_naive as excel_roundup_naive,
};
pub use eval::concat::{concat_naive_join, eval_concat_formula, ConcatBuilder, CONCAT_MAX_CHARS};
pub use eval::search::{search as excel_search, search_naive as excel_search_naive};
pub use dates::{weekday as excel_weekday, weekday_naive as excel_weekday_naive};
pub use eval::npv::{npv as excel_npv, npv_naive as excel_npv_naive};
pub use eval::switch::{
    first_match as excel_switch_first_match, first_match_naive as excel_switch_first_match_naive,
    pick_evaluated as excel_switch_pick_evaluated,
};
pub use eval::ifs::{select as excel_ifs, select_naive as excel_ifs_naive};
pub use eval::unique::{unique_apply, unique_apply_naive, unique_eq};
pub use eval::filter::{select as excel_filter, select_naive as excel_filter_naive};
pub use eval::{eval_formula_in, Evaluator};
pub use eval::{eval_formula_in, eval_sumifs_materialized, Evaluator};
pub use eval::{eval_averageif_materialized, eval_formula_in, Evaluator};
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
