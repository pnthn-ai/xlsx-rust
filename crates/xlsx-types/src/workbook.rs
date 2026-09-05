//! Minimal workbook snippet used by fixtures (no full OOXML).

use crate::cell::{AddrError, CellAddr, CellRef};
use crate::value::ExcelValue;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

/// A cell in a snippet: optional cached value and optional formula text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Cell {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<ExcelValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
}

impl Cell {
    pub fn value(v: ExcelValue) -> Self {
        Self {
            value: Some(v),
            formula: None,
        }
    }

    pub fn formula(formula: impl Into<String>, cached: Option<ExcelValue>) -> Self {
        Self {
            value: cached,
            formula: Some(formula.into()),
        }
    }

    pub fn is_blank(&self) -> bool {
        self.formula.is_none() && matches!(self.value, None | Some(ExcelValue::Empty))
    }
}

/// One sheet of a snippet workbook.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sheet {
    pub name: String,
    /// Cells keyed by A1 (`A1`, `B2`, …). Missing keys are blank.
    #[serde(default)]
    pub cells: BTreeMap<String, Cell>,
}

impl Sheet {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            cells: BTreeMap::new(),
        }
    }

    pub fn get(&self, addr: CellAddr) -> Option<&Cell> {
        self.cells.get(&addr.a1())
    }

    pub fn insert(&mut self, addr: CellAddr, cell: Cell) {
        self.cells.insert(addr.a1(), cell);
    }

    pub fn value_or_empty(&self, addr: CellAddr) -> ExcelValue {
        match self.get(addr) {
            Some(c) => c.value.clone().unwrap_or(ExcelValue::Empty),
            None => ExcelValue::Empty,
        }
    }
}

/// Defined name pointing at a cell, range, or formula.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DefinedName {
    pub name: String,
    /// A1-style ref (`Sheet1!B2` or `Sheet1!A1:A3`) or a formula (`=SUM(A1:A3)`).
    pub refers_to: String,
}

/// Tiny workbook used as fixture context. Not an `.xlsx` parser.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Workbook {
    #[serde(default)]
    pub sheets: Vec<Sheet>,
    #[serde(default)]
    pub names: Vec<DefinedName>,
}

impl Default for Workbook {
    fn default() -> Self {
        Self {
            sheets: vec![Sheet::new("Sheet1")],
            names: Vec::new(),
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum WorkbookError {
    #[error("sheet not found: {0}")]
    MissingSheet(String),
    #[error("defined name not found: {0}")]
    MissingName(String),
    #[error(transparent)]
    Addr(#[from] AddrError),
}

impl Workbook {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn default_sheet_name(&self) -> &str {
        self.sheets
            .first()
            .map(|s| s.name.as_str())
            .unwrap_or("Sheet1")
    }

    pub fn sheet(&self, name: Option<&str>) -> Result<&Sheet, WorkbookError> {
        let want = name.unwrap_or_else(|| self.default_sheet_name());
        self.sheets
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(want))
            .ok_or_else(|| WorkbookError::MissingSheet(want.to_string()))
    }

    pub fn sheet_mut(&mut self, name: Option<&str>) -> Result<&mut Sheet, WorkbookError> {
        let want = name
            .map(|s| s.to_string())
            .unwrap_or_else(|| self.default_sheet_name().to_string());
        self.sheets
            .iter_mut()
            .find(|s| s.name.eq_ignore_ascii_case(&want))
            .ok_or(WorkbookError::MissingSheet(want))
    }

    pub fn ensure_sheet(&mut self, name: &str) -> &mut Sheet {
        if let Some(idx) = self
            .sheets
            .iter()
            .position(|s| s.name.eq_ignore_ascii_case(name))
        {
            return &mut self.sheets[idx];
        }
        self.sheets.push(Sheet::new(name));
        self.sheets.last_mut().unwrap()
    }

    pub fn cell(&self, r: &CellRef) -> Result<Option<&Cell>, WorkbookError> {
        Ok(self.sheet(r.sheet.as_deref())?.get(r.addr))
    }

    pub fn defined_name(&self, name: &str) -> Result<&DefinedName, WorkbookError> {
        self.names
            .iter()
            .find(|n| n.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| WorkbookError::MissingName(name.to_string()))
    }

    pub fn set_value(
        &mut self,
        sheet: &str,
        a1: &str,
        value: ExcelValue,
    ) -> Result<(), WorkbookError> {
        let addr = CellAddr::parse(a1)?;
        self.ensure_sheet(sheet).insert(addr, Cell::value(value));
        Ok(())
    }
}
