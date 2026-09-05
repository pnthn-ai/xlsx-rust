//! Evaluation request shared by candidates and live oracles.

use crate::cell::{CellRef, RangeRef};
use crate::value::ExcelValue;
use crate::workbook::Workbook;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// What the engine should compute.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EvalTarget {
    /// Evaluate `formula` as if entered at `at` (defaults to `Sheet1!A1`).
    Formula {
        formula: String,
        at: Option<CellRef>,
    },
    /// Compute an existing cell (literal or formula already on the sheet).
    Cell { cell: CellRef },
    /// Compute a defined name.
    Named { name: String },
}

impl EvalTarget {
    pub fn formula(formula: impl Into<String>) -> Self {
        Self::Formula {
            formula: formula.into(),
            at: None,
        }
    }
}

/// Locale used for argument separators / decimal punctuation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Locale {
    #[default]
    EnUs,
    /// Reserved: `,` decimal / `;` argument separator.
    DeDe,
}

/// Excel date epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DateSystem {
    #[default]
    Excel1900,
    Excel1904,
}

/// How ranges / array literals should be evaluated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ArrayMode {
    /// Implicit intersection / scalar result expected.
    #[default]
    Scalar,
    /// Dynamic-array (Excel 365) — spill the array.
    DynamicArray,
    /// Legacy CSE `{=…}`.
    Cse,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct EvalOptions {
    #[serde(default)]
    pub locale: Locale,
    #[serde(default)]
    pub date_system: DateSystem,
    #[serde(default)]
    pub array_mode: ArrayMode,
}

/// One evaluation request produced from a fixture (or a live oracle call).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvalSpec {
    pub case_id: String,
    #[serde(default)]
    pub workbook: Workbook,
    pub target: EvalTarget,
    #[serde(default)]
    pub options: EvalOptions,
}

impl EvalSpec {
    pub fn formula(case_id: impl Into<String>, formula: impl Into<String>) -> Self {
        Self {
            case_id: case_id.into(),
            workbook: Workbook::default(),
            target: EvalTarget::formula(formula),
            options: EvalOptions::default(),
        }
    }

    pub fn default_cell(&self) -> CellRef {
        match &self.target {
            EvalTarget::Formula { at, .. } => at.clone().unwrap_or_else(|| CellRef {
                sheet: Some(self.workbook.default_sheet_name().to_string()),
                addr: crate::cell::CellAddr::new(0, 0),
            }),
            EvalTarget::Cell { cell } => cell.clone(),
            EvalTarget::Named { .. } => CellRef {
                sheet: Some(self.workbook.default_sheet_name().to_string()),
                addr: crate::cell::CellAddr::new(0, 0),
            },
        }
    }
}

/// Infrastructure failure from a candidate or live oracle — not an Excel error value.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum EvalError {
    #[error("parse error: {0}")]
    Parse(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("workbook: {0}")]
    Workbook(String),
    #[error("candidate error: {0}")]
    Other(String),
}

/// Something that can evaluate an [`EvalSpec`].
///
/// Implement this on a calculation-engine candidate. A future live Excel /
/// LibreOffice backend implements the same shape (via [`crate::oracle`]
/// adapters in `xlsx-oracle`).
pub trait Candidate: Send + Sync {
    /// Stable id used in reports (`calc-core`, `seed-compliant`, `naive`).
    fn id(&self) -> &str;

    fn evaluate(&self, spec: &EvalSpec) -> Result<ExcelValue, EvalError>;

    /// Optional hook: load / replace the snippet workbook. Default is a no-op
    /// because [`Self::evaluate`] receives the workbook on every call.
    fn load_workbook(&mut self, _workbook: &Workbook) -> Result<(), EvalError> {
        Ok(())
    }

    fn compute_cell(&self, spec: &EvalSpec, cell: &CellRef) -> Result<ExcelValue, EvalError> {
        let mut spec = spec.clone();
        spec.target = EvalTarget::Cell { cell: cell.clone() };
        self.evaluate(&spec)
    }

    fn compute_named(&self, spec: &EvalSpec, name: &str) -> Result<ExcelValue, EvalError> {
        let mut spec = spec.clone();
        spec.target = EvalTarget::Named {
            name: name.to_string(),
        };
        self.evaluate(&spec)
    }

    fn compute_range(&self, spec: &EvalSpec, range: &RangeRef) -> Result<ExcelValue, EvalError> {
        let mut rows: Vec<Vec<ExcelValue>> = Vec::new();
        let mut row: Vec<ExcelValue> = Vec::new();
        let mut last_row = range.start.row;
        for addr in range.cells() {
            if addr.row != last_row {
                rows.push(std::mem::take(&mut row));
                last_row = addr.row;
            }
            let cell = CellRef {
                sheet: range.sheet.clone(),
                addr,
            };
            row.push(self.compute_cell(spec, &cell)?);
        }
        rows.push(row);
        if rows.len() == 1 && rows[0].len() == 1 {
            Ok(rows[0][0].clone())
        } else {
            Ok(ExcelValue::Array(rows))
        }
    }
}

impl<T: Candidate + ?Sized> Candidate for &T {
    fn id(&self) -> &str {
        (**self).id()
    }
    fn evaluate(&self, spec: &EvalSpec) -> Result<ExcelValue, EvalError> {
        (**self).evaluate(spec)
    }
}

impl<T: Candidate + ?Sized> Candidate for Box<T> {
    fn id(&self) -> &str {
        (**self).id()
    }
    fn evaluate(&self, spec: &EvalSpec) -> Result<ExcelValue, EvalError> {
        (**self).evaluate(spec)
    }
}
