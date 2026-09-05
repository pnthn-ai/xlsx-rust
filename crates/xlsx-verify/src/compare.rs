//! Compare a candidate value to an oracle value.

use serde::{Deserialize, Serialize};
use xlsx_types::{excel_num_eq, ExcelType, ExcelValue};

/// How numbers should be compared.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NumberMode {
    /// Bit-identical `f64` (NaN ≠ NaN).
    Exact,
    /// Excel 15-significant-digit crossover (default).
    Excel15,
    /// Absolute / relative tolerance.
    Tolerance { abs: f64, rel: f64 },
}

impl Default for NumberMode {
    fn default() -> Self {
        Self::Excel15
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompareOptions {
    #[serde(default)]
    pub numbers: NumberMode,
    /// Excel text equality is case-insensitive. Default: true.
    #[serde(default = "true_default")]
    pub case_insensitive_text: bool,
}

fn true_default() -> bool {
    true
}

impl Default for CompareOptions {
    fn default() -> Self {
        Self {
            numbers: NumberMode::Excel15,
            case_insensitive_text: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffKind {
    Type,
    Value,
    ErrorCode,
    ArrayShape,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Diff {
    pub kind: DiffKind,
    pub path: String,
    pub expected: String,
    pub actual: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Comparison {
    pub equal: bool,
    pub diffs: Vec<Diff>,
}

pub fn compare(expected: &ExcelValue, actual: &ExcelValue, opts: &CompareOptions) -> Comparison {
    let mut diffs = Vec::new();
    compare_at(expected, actual, opts, "$", &mut diffs);
    Comparison {
        equal: diffs.is_empty(),
        diffs,
    }
}

fn compare_at(
    expected: &ExcelValue,
    actual: &ExcelValue,
    opts: &CompareOptions,
    path: &str,
    diffs: &mut Vec<Diff>,
) {
    if expected.excel_type() != actual.excel_type() {
        diffs.push(Diff {
            kind: DiffKind::Type,
            path: path.to_string(),
            expected: expected.excel_type().to_string(),
            actual: actual.excel_type().to_string(),
            message: format!(
                "type mismatch: expected {} ({}), got {} ({})",
                expected.excel_type(),
                expected.display_compact(),
                actual.excel_type(),
                actual.display_compact()
            ),
        });
        return;
    }

    match (expected, actual) {
        (ExcelValue::Empty, ExcelValue::Empty) => {}
        (ExcelValue::Number(e), ExcelValue::Number(a)) => {
            if !numbers_eq(*e, *a, &opts.numbers) {
                diffs.push(Diff {
                    kind: DiffKind::Value,
                    path: path.to_string(),
                    expected: ExcelValue::Number(*e).display_compact(),
                    actual: ExcelValue::Number(*a).display_compact(),
                    message: format!("number mismatch: expected {e} got {a}"),
                });
            }
        }
        (ExcelValue::Text(e), ExcelValue::Text(a)) => {
            let eq = if opts.case_insensitive_text {
                e.eq_ignore_ascii_case(a)
            } else {
                e == a
            };
            if !eq {
                diffs.push(Diff {
                    kind: DiffKind::Value,
                    path: path.to_string(),
                    expected: ExcelValue::Text(e.clone()).display_compact(),
                    actual: ExcelValue::Text(a.clone()).display_compact(),
                    message: format!("text mismatch: expected {e:?} got {a:?}"),
                });
            }
        }
        (ExcelValue::Bool(e), ExcelValue::Bool(a)) => {
            if e != a {
                diffs.push(Diff {
                    kind: DiffKind::Value,
                    path: path.to_string(),
                    expected: if *e { "TRUE" } else { "FALSE" }.into(),
                    actual: if *a { "TRUE" } else { "FALSE" }.into(),
                    message: format!("bool mismatch: expected {e} got {a}"),
                });
            }
        }
        (ExcelValue::Error(e), ExcelValue::Error(a)) => {
            if e != a {
                diffs.push(Diff {
                    kind: DiffKind::ErrorCode,
                    path: path.to_string(),
                    expected: e.excel_text().to_string(),
                    actual: a.excel_text().to_string(),
                    message: format!("error code mismatch: expected {e} got {a}"),
                });
            }
        }
        (ExcelValue::Array(e), ExcelValue::Array(a)) => {
            if e.len() != a.len() || e.iter().zip(a.iter()).any(|(er, ar)| er.len() != ar.len()) {
                diffs.push(Diff {
                    kind: DiffKind::ArrayShape,
                    path: path.to_string(),
                    expected: shape(e),
                    actual: shape(a),
                    message: format!(
                        "array shape mismatch: expected {} got {}",
                        shape(e),
                        shape(a)
                    ),
                });
                return;
            }
            for (ri, (er, ar)) in e.iter().zip(a.iter()).enumerate() {
                for (ci, (ev, av)) in er.iter().zip(ar.iter()).enumerate() {
                    compare_at(ev, av, opts, &format!("{path}[{ri}][{ci}]"), diffs);
                }
            }
        }
        _ => {}
    }
}

fn numbers_eq(e: f64, a: f64, mode: &NumberMode) -> bool {
    match mode {
        NumberMode::Exact => e == a,
        NumberMode::Excel15 => excel_num_eq(e, a),
        NumberMode::Tolerance { abs, rel } => {
            if e == a {
                return true;
            }
            let diff = (e - a).abs();
            diff <= *abs || diff <= rel * e.abs().max(a.abs())
        }
    }
}

fn shape(rows: &[Vec<ExcelValue>]) -> String {
    let cols = rows.first().map(|r| r.len()).unwrap_or(0);
    format!("{}x{}", rows.len(), cols)
}

/// Used by reports to name the expected type even when the value is missing.
pub fn type_name(v: &ExcelValue) -> ExcelType {
    v.excel_type()
}

#[cfg(test)]
mod tests {
    use super::*;
    use xlsx_types::ExcelError;

    #[test]
    fn type_mismatch_is_actionable() {
        let c = compare(
            &ExcelValue::Number(0.0),
            &ExcelValue::Empty,
            &CompareOptions::default(),
        );
        assert!(!c.equal);
        assert_eq!(c.diffs[0].kind, DiffKind::Type);
    }

    #[test]
    fn error_codes_differ() {
        let c = compare(
            &ExcelValue::Error(ExcelError::Div0),
            &ExcelValue::Error(ExcelError::Value),
            &CompareOptions::default(),
        );
        assert_eq!(c.diffs[0].kind, DiffKind::ErrorCode);
    }

    #[test]
    fn excel15_accepts_ieee_sum() {
        let c = compare(
            &ExcelValue::Number(0.3),
            &ExcelValue::Number(0.1 + 0.2),
            &CompareOptions::default(),
        );
        assert!(c.equal);
    }
}
