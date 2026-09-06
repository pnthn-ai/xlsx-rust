//! Shared Excel-compatible types for the xlsx-rust verification stack.
//!
//! Crate boundaries:
//! - **xlsx-types** — values, errors, COUNTIF / COUNTIFS criteria, workbook snippets, [`Candidate`] trait
//! - **xlsx-oracle** — trusted expected-result source
//! - **xlsx-verify** — corpus + compare + report + CLI
//! - **xlsx-engine-core** — real `calc-core` formula engine
//! - **xlsx-engine** — stub candidates (`seed-compliant`, `naive`) that demonstrate the gate
//! - **xlsx-bench** — Criterion harness + large-snippet builders (advisory timings only)

pub mod cell;
pub mod criterion;
pub mod error;
pub mod eval;
pub mod excel_ceiling;
pub mod excel_int;
pub mod financial;
pub mod floor_ceiling;
pub mod quirk;
pub mod value;
pub mod workbook;

pub use cell::{AddrError, CellAddr, CellRef, RangeRef};
pub use criterion::{count_matches, excel_wildcard, looks_like_wildcard_pat, Criterion};
pub use error::ExcelError;
pub use eval::{
    ArrayMode, Candidate, DateSystem, EvalError, EvalOptions, EvalSpec, EvalTarget, Locale,
};
pub use excel_ceiling::{
    excel_ceiling, excel_ceiling_ieee, excel_ceiling_naive, excel_ceiling_slice,
    excel_ceiling_slice_naive,
};
pub use excel_int::{
    excel_int, excel_int_ieee, excel_int_naive, excel_int_slice, excel_int_slice_naive,
};
pub use financial::{
    cumipmt as excel_cumipmt, cumipmt_naive as excel_cumipmt_naive, cumprinc as excel_cumprinc,
    cumprinc_naive as excel_cumprinc_naive, effect as excel_effect,
    effect_naive as excel_effect_naive, fv as excel_fv, fv_naive as excel_fv_naive,
    ipmt as excel_ipmt, ipmt_naive as excel_ipmt_naive, nominal as excel_nominal,
    nominal_naive as excel_nominal_naive, nper as excel_nper, nper_naive as excel_nper_naive,
    pduration as excel_pduration, pduration_naive as excel_pduration_naive, pmt as excel_pmt,
    ppmt as excel_ppmt, ppmt_naive as excel_ppmt_naive, pv as excel_pv, pv_naive as excel_pv_naive,
    rate as excel_rate, rate_naive as excel_rate_naive, rri as excel_rri,
    rri_naive as excel_rri_naive, RATE_MAX_ITERS, RATE_TOL,
};
pub use floor_ceiling::{
    excel_ceiling_math, excel_floor, excel_floor_math, excel_floor_naive, excel_floor_slice,
    excel_floor_slice_naive,
};
pub use quirk::QuirkCategory;
pub use value::{excel_num_eq, excel_round_15, ExcelType, ExcelValue};
pub use workbook::{Cell, DefinedName, Sheet, Workbook, WorkbookError};
