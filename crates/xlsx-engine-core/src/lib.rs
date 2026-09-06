//! Real Excel-compatible calculation core.
//!
//! Architecture:
//! - [`parse`] — tokenizer + recursive-descent AST
//! - [`eval`] — workbook-backed walker
//! - [`eval::coerce`] / [`eval::compare`] / [`eval::empty`] — quirk modules
//! - [`eval::functions`] — worksheet functions used by the expanded corpus
//! - [`eval::textbefore`] — Excel `TEXTBEFORE` (nth delimiter, match_end / if_not_found)
//! - [`text_format`] — Excel `TEXT` for a documented number/date format subset
//! - [`eval::functions`] also dispatches `SUMIF` / `COUNTIF` / `COUNTIFS` / `SUMPRODUCT` / `SUBSTITUTE`
//! - [`eval::concat`] — Excel `CONCAT` (range/array flatten + 32767 cap)
//! - [`dates::weekday`] — O(1) Excel `WEEKDAY` on the date serial
//! - [`dates::yearfrac`] — Excel `YEARFRAC` day-count bases 0–4
//! - [`dates::workday_serial_intl`] — O(1) Excel `WORKDAY.INTL` weekend mask
//! - [`dates::networkdays_count_mask`] — O(1) `NETWORKDAYS` / `NETWORKDAYS.INTL`
//! - [`eval::switch`] — Excel `SWITCH` (exact `=` match, short-circuit vs `IF`)
//! - [`eval::ifs`] — `IFS` pair-selection kernel (eager; no-match `#N/A`)
//! - [`eval::unique`] — `UNIQUE` dynamic-array kernel (hash distinctness)
//! - [`eval::filter`] — `FILTER` mask/select kernel (`#CALC!` / `if_empty`)
//! - [`eval::sort`] — `SORT` key-extract / index-permute kernel
//! - [`eval::xlookup`] — `XLOOKUP` match/search kernel (`match_mode` / `search_mode`)
//! - [`eval::sortby`] — `SORTBY` key-extract / index-permute kernel
//! - [`eval::tocol`] — `TOCOL` flatten-to-column kernel (`ignore` / `scan_by_col`)
//! - [`eval::torow`] — `TOROW` flatten-to-row kernel (`ignore` / `scan_by_col`)
//! - [`eval::sequence`] — `SEQUENCE` row-major generator (spill / size caps)
//! - [`eval::vstack`] — `VSTACK` vertical append (`#N/A` width pad)
//! - [`eval::wrapcols`] — `WRAPCOLS` column-wrap kernel (`#N/A` pad / `#NUM!`)
//! - [`eval::wraprows`] — `WRAPROWS` reshape kernel (row wrap + pad)
//! - [`eval::hstack`] — `HSTACK` horizontal stack kernel (`#N/A` height pad)
//! - [`eval::take`] — `TAKE` window/slice kernel (negative counts, `#CALC!` on 0)
//! - [`eval::choosecols`] — `CHOOSECOLS` column-pick kernel (neg index / `#VALUE!`)
//! - [`eval::drop`] — `DROP` rectangle slice (`#CALC!` on empty result)
//! - [`eval::expand`] — `EXPAND` grow/pad kernel (`#N/A` pad, shrink `#VALUE!`)
//! - [`eval::chooserows`] — `CHOOSEROWS` pick kernel (negative index / `#VALUE!`)
//! - [`eval::randarray`] — `RANDARRAY` dynamic-array kernel (xorshift64*; not Excel's RNG)
//! - [`eval::makearray`] — `MAKEARRAY(rows, cols, LAMBDA(r, c, body))`
//! - [`eval::isomitted`] — Excel `ISOMITTED` (omitted LAMBDA parameter)
//! - [`eval::textsplit`] — `TEXTSPLIT` col/row split kernel (pad / `#CALC!`)
//! - [`eval::textafter`] — Excel `TEXTAFTER` kernel (nth delimiter, `match_end`)
//! - [`eval::irr`] — Excel `IRR` Newton / secant kernel
//! - [`xlsx_types::excel_nper`] — Excel `NPER` closed form (`ln1p`)
//! - [`eval::xnpv`] — Excel `XNPV` irregular-date NPV kernel
//! - [`eval::xirr`] — Excel `XIRR` Newton / bisection kernel (365-day serials)
//! - [`eval::mirr`] — Excel `MIRR` (finance / reinvest NPV closed form)
//! - [`xlsx_types::excel_effect`] — Excel `EFFECT` (nominal → effective annual)
//! - [`xlsx_types::excel_nominal`] — Excel `NOMINAL` (effective → nominal annual)
//! - [`xlsx_types::excel_pduration`] — Excel `PDURATION` (lump-sum periods)
//! - Financial TVM: `PMT` / `RRI` via [`xlsx_types::excel_pmt`] / [`xlsx_types::excel_rri`]
//!
//! This crate depends only on [`xlsx_types`]. It never reads fixture expected
//! values; the verification gate (`xlsx-verify`) is the only judge.

pub mod ast;
pub mod dates;
pub mod eval;
pub mod parse;
pub mod text_format;

pub use dates::{workday_serial, workday_serial_intl};

pub use ast::{BinOp, Expr, UnaryOp};
pub use dates::{
    weekday as excel_weekday, weekday_naive as excel_weekday_naive, yearfrac as excel_yearfrac,
    yearfrac_naive as excel_yearfrac_naive,
};
pub use eval::choosecols::{select as excel_choosecols, select_naive as excel_choosecols_naive};
pub use eval::chooserows::{select as excel_chooserows, select_naive as excel_chooserows_naive};
pub use eval::concat::{concat_naive_join, eval_concat_formula, ConcatBuilder, CONCAT_MAX_CHARS};
pub use eval::drop::{apply as excel_drop, apply_naive as excel_drop_naive};
pub use eval::expand::{
    dim_from_value as expand_dim_from_value, expand as excel_expand,
    expand_naive as excel_expand_naive, output_shape as expand_output_shape,
    resolve_dim as expand_resolve_dim, EXPAND_MAX_COLS, EXPAND_MAX_ROWS,
};
pub use eval::filter::{select as excel_filter, select_naive as excel_filter_naive};
pub use eval::hstack::{hstack as excel_hstack, hstack_naive as excel_hstack_naive};
pub use eval::find::{find as excel_find, find_naive as excel_find_naive};
pub use eval::ifs::{select as excel_ifs, select_naive as excel_ifs_naive};
pub use eval::irr::{irr as excel_irr, irr_naive as excel_irr_naive, MAX_ITERS as IRR_MAX_ITERS};
pub use eval::mirr::{mirr as excel_mirr, mirr_naive as excel_mirr_naive};
pub use eval::isomitted::{is_omitted as excel_isomitted, is_omitted_naive as excel_isomitted_naive};
pub use eval::makearray::{
    fill_fast as excel_makearray, fill_naive as excel_makearray_naive, FastBody, FastOp, Local,
};
pub use eval::npv::{npv as excel_npv, npv_naive as excel_npv_naive};
pub use eval::randarray::{
    apply as excel_randarray, apply_naive as excel_randarray_naive, fill as excel_randarray_fill,
    fill_naive as excel_randarray_fill_naive, XorShift64,
};
pub use eval::replace::{replace as excel_replace, replace_naive as excel_replace_naive};
pub use eval::round::{
    rounddown as excel_rounddown, rounddown_naive as excel_rounddown_naive,
    roundup as excel_roundup, roundup_naive as excel_roundup_naive,
};
pub use eval::search::{search as excel_search, search_naive as excel_search_naive};
pub use eval::sort::{sort_apply as excel_sort, sort_apply_naive as excel_sort_naive};
pub use eval::sortby::{
    sortby_apply as excel_sortby, sortby_apply_naive as excel_sortby_naive, MAX_SORT_KEYS,
};
pub use eval::sequence::{
    sequence as excel_sequence, sequence_naive as excel_sequence_naive,
    MAX_CELLS as SEQUENCE_MAX_CELLS,
};
pub use eval::substitute::{
    substitute as excel_substitute, substitute_naive as excel_substitute_naive,
};
pub use eval::sumproduct::{product_sum, product_sum_naive, product_sum_packed};
pub use eval::switch::{
    first_match as excel_switch_first_match, first_match_naive as excel_switch_first_match_naive,
    pick_evaluated as excel_switch_pick_evaluated,
};
pub use eval::take::{take as excel_take, take_naive as excel_take_naive};
pub use eval::textafter::{textafter as excel_textafter, textafter_naive as excel_textafter_naive};
pub use eval::textbefore::{
    textbefore as excel_textbefore, textbefore_naive as excel_textbefore_naive,
};
pub use eval::textjoin::{
    eval_textjoin_formula, textjoin_naive_join, TextJoinBuilder, TEXTJOIN_MAX_CHARS,
};
pub use eval::tocol::{
    parse_ignore as parse_tocol_ignore, tocol_apply, tocol_apply_limited, tocol_apply_naive,
    TOCOL_MAX_ROWS,
};
pub use eval::torow::{
    apply as excel_torow_apply, apply_naive as excel_torow_naive, excel_torow,
    parse_ignore as parse_torow_ignore, TorowIgnore,
};
pub use eval::textsplit::{
    apply_values as excel_textsplit_apply, textsplit as excel_textsplit,
    textsplit_naive as excel_textsplit_naive,
};
pub use eval::unique::{unique_apply, unique_apply_naive, unique_eq};
pub use eval::xnpv::{
    collect_series as collect_xnpv_series, date_serial_trunc as xnpv_date_serial_trunc,
    xnpv as excel_xnpv, xnpv_naive as excel_xnpv_naive,
};
pub use eval::xirr::{
    collect_series as collect_xirr_series, date_serial_trunc as xirr_date_serial_trunc,
    xirr as excel_xirr, xirr_naive as excel_xirr_naive, MAX_ITERS as XIRR_MAX_ITERS,
};
pub use eval::xlookup::{xlookup as excel_xlookup, xlookup_naive as excel_xlookup_naive};
pub use eval::vstack::{
    stack as excel_vstack, stack_naive as excel_vstack_naive, stack_owned as excel_vstack_owned,
};
pub use eval::wrapcols::{wrapcols as excel_wrapcols, wrapcols_naive as excel_wrapcols_naive};
pub use eval::wraprows::{
    output_shape as wraprows_output_shape, parse_wrap_count, wraprows as excel_wraprows,
    wraprows_naive as excel_wraprows_naive, WRAPROWS_MAX_COLS, WRAPROWS_MAX_ROWS,
};
pub use eval::{
    eval_averageif_materialized, eval_averageifs_materialized, eval_countifs_materialized,
    eval_formula_in, eval_sumif_materialized, eval_sumifs_materialized, Evaluator,
};
pub use parse::parse;
pub use xlsx_types::{
    excel_cumipmt, excel_cumipmt_naive, excel_cumprinc, excel_cumprinc_naive, excel_effect,
    excel_effect_naive, excel_fv, excel_fv_naive, excel_ipmt, excel_ipmt_naive, excel_nominal,
    excel_nominal_naive, excel_nper, excel_nper_naive, excel_pduration, excel_pduration_naive,
    excel_pmt, excel_ppmt, excel_ppmt_naive, excel_pv, excel_pv_naive, excel_rate, excel_rate_naive,
    excel_rri, excel_rri_naive,
};

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
