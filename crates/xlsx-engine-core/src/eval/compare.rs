//! Excel comparison quirks: equality vs ranking.
//!
//! - 15-significant-digit numeric equality (`0.1+0.2=0.3`)
//! - Case-insensitive text equality
//! - `TRUE=1` / `FALSE=0`
//! - Empty-cell duality (delegated to [`super::empty`])
//! - `"2"=2` is FALSE (no arithmetic coercion on `=`)
//! - Type ranking for `<`/`>`: logical > text > number (`FALSE>100`)

use super::empty;
use xlsx_types::{excel_num_eq, ExcelValue};

pub fn num_eq(a: f64, b: f64) -> bool {
    excel_num_eq(a, b)
}

/// Excel `=` / `<>` (caller inverts for `<>`). Errors already stripped.
pub fn equal(l: &ExcelValue, r: &ExcelValue) -> bool {
    match (l, r) {
        (ExcelValue::Empty, other) | (other, ExcelValue::Empty)
            if !matches!(other, ExcelValue::Empty) =>
        {
            empty::equals(other).unwrap_or(false)
        }
        (ExcelValue::Empty, ExcelValue::Empty) => true,
        (ExcelValue::Number(a), ExcelValue::Number(b)) => num_eq(*a, *b),
        (ExcelValue::Text(a), ExcelValue::Text(b)) => a.eq_ignore_ascii_case(b),
        (ExcelValue::Bool(a), ExcelValue::Bool(b)) => a == b,
        (ExcelValue::Number(n), ExcelValue::Bool(b))
        | (ExcelValue::Bool(b), ExcelValue::Number(n)) => num_eq(*n, if *b { 1.0 } else { 0.0 }),
        _ => false,
    }
}

/// Rank for ordered comparisons. Lower ranks compare less than higher ranks
/// regardless of payload (`"abc">1`, `FALSE>100`).
pub fn type_rank(v: &ExcelValue) -> u8 {
    match v {
        ExcelValue::Number(_) => 0,
        ExcelValue::Empty => empty::compare_rank(),
        ExcelValue::Text(_) => 1,
        ExcelValue::Bool(_) => 2,
        ExcelValue::Error(_) | ExcelValue::Array(_) => 9,
    }
}

pub fn ordering(l: &ExcelValue, r: &ExcelValue) -> std::cmp::Ordering {
    let rl = type_rank(l);
    let rr = type_rank(r);
    if rl != rr {
        return rl.cmp(&rr);
    }
    match (l, r) {
        (ExcelValue::Number(a), ExcelValue::Number(b)) => {
            a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
        }
        (ExcelValue::Empty, ExcelValue::Number(b)) => empty::as_number()
            .partial_cmp(b)
            .unwrap_or(std::cmp::Ordering::Equal),
        (ExcelValue::Number(a), ExcelValue::Empty) => a
            .partial_cmp(&empty::as_number())
            .unwrap_or(std::cmp::Ordering::Equal),
        (ExcelValue::Empty, ExcelValue::Empty) => std::cmp::Ordering::Equal,
        (ExcelValue::Text(a), ExcelValue::Text(b)) => {
            a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase())
        }
        (ExcelValue::Bool(a), ExcelValue::Bool(b)) => a.cmp(b),
        _ => std::cmp::Ordering::Equal,
    }
}

/// `want` is the ordering that should yield TRUE. When `invert` is set
/// (for `<=` / `>=`), TRUE means "not the opposite ordering".
pub fn ordered(l: &ExcelValue, r: &ExcelValue, want: std::cmp::Ordering, invert: bool) -> bool {
    let ord = ordering(l, r);
    if invert {
        ord != want
    } else {
        ord == want
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xlsx_types::ExcelValue;

    #[test]
    fn equality_quirks() {
        assert!(equal(
            &ExcelValue::Text("A".into()),
            &ExcelValue::Text("a".into())
        ));
        assert!(!equal(
            &ExcelValue::Text("2".into()),
            &ExcelValue::Number(2.0)
        ));
        assert!(equal(&ExcelValue::Bool(true), &ExcelValue::Number(1.0)));
        assert!(equal(&ExcelValue::Empty, &ExcelValue::Number(0.0)));
        assert!(equal(&ExcelValue::Empty, &ExcelValue::Text(String::new())));
        assert!(!equal(
            &ExcelValue::Number(0.0),
            &ExcelValue::Text(String::new())
        ));
        assert!(equal(
            &ExcelValue::Number(0.1 + 0.2),
            &ExcelValue::Number(0.3)
        ));
    }

    #[test]
    fn ranking_quirks() {
        assert!(ordered(
            &ExcelValue::Text("abc".into()),
            &ExcelValue::Number(1.0),
            std::cmp::Ordering::Greater,
            false
        ));
        assert!(ordered(
            &ExcelValue::Bool(false),
            &ExcelValue::Number(100.0),
            std::cmp::Ordering::Greater,
            false
        ));
    }
}
