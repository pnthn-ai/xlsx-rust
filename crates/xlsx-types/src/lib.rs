//! Shared Excel-compatible types for the xlsx-rust verification stack.
//!
//! Crate boundaries:
//! - **xlsx-types** — values, errors, COUNTIF criteria, workbook snippets, [`Candidate`] trait
//! - **xlsx-oracle** — trusted expected-result source
//! - **xlsx-verify** — corpus + compare + report + CLI
//! - **xlsx-engine-core** — real `calc-core` formula engine
//! - **xlsx-engine** — stub candidates (`seed-compliant`, `naive`) that demonstrate the gate
//! - **xlsx-bench** — Criterion harness + large-snippet builders (advisory timings only)

pub mod cell;
pub mod criterion;
pub mod error;
pub mod eval;
pub mod floor_ceiling;
pub mod quirk;
pub mod value;
pub mod workbook;

pub use criterion::{count_matches, excel_wildcard, looks_like_wildcard_pat, Criterion};
pub use cell::{AddrError, CellAddr, CellRef, RangeRef};
pub use error::ExcelError;
pub use eval::{
    ArrayMode, Candidate, DateSystem, EvalError, EvalOptions, EvalSpec, EvalTarget, Locale,
};
pub use floor_ceiling::{
    excel_ceiling, excel_ceiling_math, excel_ceiling_naive, excel_ceiling_slice,
    excel_ceiling_slice_naive, excel_floor, excel_floor_math, excel_floor_naive, excel_floor_slice,
    excel_floor_slice_naive,
};
pub use quirk::QuirkCategory;
pub use value::{excel_num_eq, excel_round_15, ExcelType, ExcelValue};
pub use workbook::{Cell, DefinedName, Sheet, Workbook, WorkbookError};
