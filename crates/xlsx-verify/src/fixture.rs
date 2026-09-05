//! Fixture / corpus case schema (JSON or TOML).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use thiserror::Error;
use xlsx_types::{
    Cell, CellAddr, CellRef, EvalOptions, EvalSpec, EvalTarget, ExcelValue, QuirkCategory, Sheet,
    Workbook,
};

/// One correctness case.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Fixture {
    pub id: String,
    pub description: String,
    pub tags: Vec<String>,
    pub quirks: Vec<QuirkCategory>,
    pub spec: EvalSpec,
    pub expected: Option<ExcelValue>,
    /// When set, the verifier skips the case (reason is shown).
    pub ignore: Option<String>,
    pub notes: Option<String>,
    pub source: PathBuf,
}

#[derive(Debug, Error)]
pub enum FixtureError {
    #[error("io error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parse error in {path}: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("invalid fixture {id}: {message}")]
    Invalid { id: String, message: String },
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum FileDto {
    Many { cases: Vec<CaseDto> },
    List(Vec<CaseDto>),
    One(Box<CaseDto>),
}

#[derive(Debug, Deserialize)]
struct CaseDto {
    id: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    quirks: Vec<String>,
    #[serde(default)]
    workbook: Option<WorkbookDto>,
    #[serde(default)]
    formula: Option<String>,
    #[serde(default)]
    at: Option<String>,
    #[serde(default)]
    cell: Option<String>,
    #[serde(default)]
    named: Option<String>,
    #[serde(default)]
    expected: Option<ExcelValue>,
    #[serde(default)]
    ignore: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    options: Option<EvalOptions>,
}

#[derive(Debug, Deserialize)]
struct WorkbookDto {
    #[serde(default)]
    sheets: Vec<SheetDto>,
    #[serde(default)]
    names: Vec<xlsx_types::DefinedName>,
}

#[derive(Debug, Deserialize)]
struct SheetDto {
    #[serde(default = "default_sheet")]
    name: String,
    #[serde(default)]
    cells: BTreeMap<String, CellDto>,
}

fn default_sheet() -> String {
    "Sheet1".to_string()
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum CellDto {
    /// `{ "number": 1 }` / `{ "text": "x" }` / …
    Value(ExcelValue),
    Full {
        #[serde(default)]
        value: Option<ExcelValue>,
        #[serde(default)]
        formula: Option<String>,
    },
}

impl CellDto {
    fn into_cell(self) -> Cell {
        match self {
            CellDto::Value(v) => Cell::value(v),
            CellDto::Full { value, formula } => Cell { value, formula },
        }
    }
}

pub fn load_path(path: &Path) -> Result<Vec<Fixture>, FixtureError> {
    let data = std::fs::read_to_string(path).map_err(|source| FixtureError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    load_str(path, &data)
}

pub fn load_str(path: &Path, data: &str) -> Result<Vec<Fixture>, FixtureError> {
    let dto: FileDto = serde_json::from_str(data).map_err(|e| FixtureError::Parse {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    let cases = match dto {
        FileDto::Many { cases } => cases,
        FileDto::List(cases) => cases,
        FileDto::One(c) => vec![*c],
    };
    cases.into_iter().map(|c| dto_to_fixture(c, path)).collect()
}

fn dto_to_fixture(dto: CaseDto, path: &Path) -> Result<Fixture, FixtureError> {
    let id = dto.id.clone();
    let target = match (dto.formula, dto.cell, dto.named) {
        (Some(formula), None, None) => {
            let at = match dto.at {
                Some(s) => Some(CellRef::parse(&s).map_err(|e| FixtureError::Invalid {
                    id: id.clone(),
                    message: e.to_string(),
                })?),
                None => None,
            };
            EvalTarget::Formula { formula, at }
        }
        (None, Some(cell), None) => EvalTarget::Cell {
            cell: CellRef::parse(&cell).map_err(|e| FixtureError::Invalid {
                id: id.clone(),
                message: e.to_string(),
            })?,
        },
        (None, None, Some(name)) => EvalTarget::Named { name },
        _ => {
            return Err(FixtureError::Invalid {
                id,
                message: "exactly one of formula, cell, or named is required".into(),
            });
        }
    };

    let workbook = match dto.workbook {
        Some(wb) => {
            let sheets = if wb.sheets.is_empty() {
                vec![Sheet::new("Sheet1")]
            } else {
                wb.sheets
                    .into_iter()
                    .map(|s| {
                        let mut sheet = Sheet::new(s.name);
                        for (a1, cell) in s.cells {
                            CellAddr::parse(&a1).map_err(|e| FixtureError::Invalid {
                                id: id.clone(),
                                message: format!("cell key {a1}: {e}"),
                            })?;
                            sheet.cells.insert(a1, cell.into_cell());
                        }
                        Ok(sheet)
                    })
                    .collect::<Result<Vec<_>, _>>()?
            };
            Workbook {
                sheets,
                names: wb.names,
            }
        }
        None => Workbook::default(),
    };

    let mut quirks = Vec::new();
    for q in &dto.quirks {
        match QuirkCategory::parse(q) {
            Some(cat) => quirks.push(cat),
            None => {
                return Err(FixtureError::Invalid {
                    id: id.clone(),
                    message: format!("unknown quirk category: {q}"),
                });
            }
        }
    }

    Ok(Fixture {
        spec: EvalSpec {
            case_id: dto.id.clone(),
            workbook,
            target,
            options: dto.options.unwrap_or_default(),
        },
        id: dto.id,
        description: dto.description.unwrap_or_default(),
        tags: dto.tags,
        quirks,
        expected: dto.expected,
        ignore: dto.ignore,
        notes: dto.notes,
        source: path.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_json_array() {
        let src = r#"
        [
          {
            "id": "arith.add",
            "description": "1+2",
            "formula": "=1+2",
            "expected": { "number": 3 },
            "tags": ["arithmetic"],
            "quirks": []
          }
        ]
        "#;
        let cases = load_str(Path::new("t.json"), src).unwrap();
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].id, "arith.add");
        assert_eq!(cases[0].expected, Some(ExcelValue::Number(3.0)));
    }

    #[test]
    fn parse_wrapped_cases_object() {
        let src = r#"
        { "cases": [{ "id": "arith.add", "formula": "=1+2", "expected": { "number": 3 } }] }
        "#;
        let cases = load_str(Path::new("t.json"), src).unwrap();
        assert_eq!(cases[0].id, "arith.add");
    }

    #[test]
    fn parse_workbook_cells() {
        let src = r#"
        {
          "id": "sum.range",
          "formula": "=SUM(A1:A2)",
          "expected": { "number": 3 },
          "workbook": {
            "sheets": [
              {
                "name": "Sheet1",
                "cells": {
                  "A1": { "number": 1 },
                  "A2": { "number": 2 }
                }
              }
            ]
          }
        }
        "#;
        let cases = load_str(Path::new("t.json"), src).unwrap();
        assert_eq!(
            cases[0].spec.workbook.sheets[0].cells["A1"].value,
            Some(ExcelValue::Number(1.0))
        );
    }
}
