//! Empty-cell duality.
//!
//! A blank cell is a first-class [`ExcelValue::Empty`]: it is **not** the
//! number `0` and **not** the text `""`. Operators still treat it as one or
//! the other:
//!
//! - arithmetic / numeric compare rank → `0`
//! - concatenation / text equality → `""`
//! - `IF` / logical → `FALSE`
//! - `A1=0` and `A1=""` are both `TRUE` when `A1` is blank
//! - `0=""` is `FALSE` (the duality is only for empty, not for literals)

use xlsx_types::{excel_num_eq, ExcelValue};

/// Numeric value of a blank in arithmetic and numeric ordering.
pub fn as_number() -> f64 {
    0.0
}

/// Text value of a blank in concatenation and text equality.
pub fn as_text() -> String {
    String::new()
}

/// Logical value of a blank in `IF` / `AND` / `OR`.
pub fn as_logical() -> bool {
    false
}

/// Type-rank used by `<` / `>` (same bucket as numbers).
pub fn compare_rank() -> u8 {
    0
}

/// Equality of `Empty` against `other`. Returns `None` when `other` is not
/// something empty is defined to equal (caller should treat as `false`).
pub fn equals(other: &ExcelValue) -> Option<bool> {
    match other {
        ExcelValue::Empty => Some(true),
        ExcelValue::Number(n) => Some(excel_num_eq(*n, 0.0)),
        ExcelValue::Text(s) => Some(s.is_empty()),
        ExcelValue::Bool(b) => Some(!*b),
        ExcelValue::Error(_) | ExcelValue::Array(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xlsx_types::ExcelValue;

    #[test]
    fn duality_vs_literals() {
        assert_eq!(equals(&ExcelValue::Number(0.0)), Some(true));
        assert_eq!(equals(&ExcelValue::Text(String::new())), Some(true));
        assert_eq!(equals(&ExcelValue::Bool(false)), Some(true));
        assert_eq!(equals(&ExcelValue::Empty), Some(true));
        // 0="" is a *literal* comparison, not empty duality — handled in compare.
        assert_eq!(equals(&ExcelValue::Text("x".into())), Some(false));
    }
}
