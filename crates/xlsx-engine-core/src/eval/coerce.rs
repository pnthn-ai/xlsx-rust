//! Excel type coercion for arithmetic, concatenation, and `IF`.
//!
//! Localized here so the evaluator does not grow a second copy of
//! `"2"+1 = 3` vs `"2"=2` is `FALSE`. Equality lives in [`super::compare`].

use super::empty;
use xlsx_types::{ExcelError, ExcelValue};

/// Coerce a value to a number (arithmetic, unary `+/-/%`, `ABS`, …).
///
/// Empty → 0, TRUE → 1, FALSE → 0, numeric text → parsed, other text → `#VALUE!`.
pub fn to_number(v: &ExcelValue) -> Result<f64, ExcelError> {
    match v {
        ExcelValue::Error(e) => Err(*e),
        ExcelValue::Number(n) => Ok(*n),
        ExcelValue::Empty => Ok(empty::as_number()),
        ExcelValue::Bool(true) => Ok(1.0),
        ExcelValue::Bool(false) => Ok(0.0),
        ExcelValue::Text(s) => parse_numeric_text(s),
        ExcelValue::Array(_) => Err(ExcelError::Value),
    }
}

/// Coerce a value to text (`&`).
pub fn to_text(v: &ExcelValue) -> Result<String, ExcelError> {
    match v {
        ExcelValue::Error(e) => Err(*e),
        ExcelValue::Text(s) => Ok(s.clone()),
        ExcelValue::Empty => Ok(empty::as_text()),
        ExcelValue::Bool(true) => Ok("TRUE".into()),
        ExcelValue::Bool(false) => Ok("FALSE".into()),
        ExcelValue::Number(n) => Ok(format_plain(*n)),
        ExcelValue::Array(_) => Err(ExcelError::Value),
    }
}

/// Coerce a value to a logical used by `IF` / `FILTER` include (and VLOOKUP's range_lookup).
///
/// Numbers: nonzero is TRUE. Empty is FALSE. Text is `#VALUE!` (Excel does
/// not treat `"TRUE"` as a logical here unless it is the boolean literal).
pub fn to_logical(v: &ExcelValue) -> Result<bool, ExcelError> {
    match v {
        ExcelValue::Error(e) => Err(*e),
        ExcelValue::Bool(b) => Ok(*b),
        ExcelValue::Number(n) => Ok(*n != 0.0),
        ExcelValue::Empty => Ok(empty::as_logical()),
        ExcelValue::Text(_) | ExcelValue::Array(_) => Err(ExcelError::Value),
    }
}

pub fn parse_numeric_text(s: &str) -> Result<f64, ExcelError> {
    let t = s.trim();
    if t.is_empty() {
        return Err(ExcelError::Value);
    }
    t.parse::<f64>().map_err(|_| ExcelError::Value)
}

pub fn format_plain(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{n:.0}")
    } else {
        format!("{n}")
    }
}

/// Implicit intersection / first-element unwrap for a scalar operator.
///
/// Real Excel picks the cell on the same row/column as the host. This core
/// takes the top-left element, which is enough for the seed corpus and for
/// array-literal operands.
pub fn scalarize(v: ExcelValue) -> ExcelValue {
    match v {
        ExcelValue::Array(rows) => rows
            .first()
            .and_then(|r| r.first())
            .cloned()
            .unwrap_or(ExcelValue::Empty),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_coercion() {
        assert_eq!(to_number(&ExcelValue::Empty).unwrap(), 0.0);
        assert_eq!(to_number(&ExcelValue::Bool(true)).unwrap(), 1.0);
        assert_eq!(to_number(&ExcelValue::Text("2".into())).unwrap(), 2.0);
        assert_eq!(
            to_number(&ExcelValue::Text("x".into())),
            Err(ExcelError::Value)
        );
    }

    #[test]
    fn if_coercion() {
        assert_eq!(to_logical(&ExcelValue::Number(1.0)).unwrap(), true);
        assert_eq!(to_logical(&ExcelValue::Number(0.0)).unwrap(), false);
        assert_eq!(to_logical(&ExcelValue::Empty).unwrap(), false);
        assert_eq!(
            to_logical(&ExcelValue::Text("x".into())),
            Err(ExcelError::Value)
        );
    }
}
