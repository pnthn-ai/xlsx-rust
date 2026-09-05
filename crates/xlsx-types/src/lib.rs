//! Shared Excel-compatible types for the xlsx-rust verification stack.
//!
//! Crate boundaries:
//! - **xlsx-types** — values, errors, workbook snippets, [`Candidate`] trait
//! - **xlsx-oracle** — trusted expected-result source
//! - **xlsx-verify** — corpus + compare + report + CLI
//! - **xlsx-engine-core** — real `calc-core` formula engine
//! - **xlsx-engine** — stub candidates (`seed-compliant`, `naive`) that demonstrate the gate

pub mod cell;
pub mod error;
pub mod eval;
pub mod financial;
pub mod quirk;
pub mod value;
pub mod workbook;

pub use cell::{AddrError, CellAddr, CellRef, RangeRef};
pub use error::ExcelError;
pub use eval::{
    ArrayMode, Candidate, DateSystem, EvalError, EvalOptions, EvalSpec, EvalTarget, Locale,
};
pub use financial::pmt as excel_pmt;
pub use quirk::QuirkCategory;
pub use value::{excel_num_eq, excel_round_15, ExcelType, ExcelValue};
pub use workbook::{Cell, DefinedName, Sheet, Workbook, WorkbookError};
