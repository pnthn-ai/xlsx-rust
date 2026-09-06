//! Excel `EXACT` kernel.
//!
//! Semantics (desktop Excel / Microsoft docs):
//! - `EXACT(text1, text2)` is a **case-sensitive** compare after `&`-style
//!   text coercion (numbers use [`super::coerce::format_plain`], bools
//!   become `"TRUE"` / `"FALSE"`, blanks become `""`).
//! - Spaces, tabs, and NBSP are significant. `*` / `?` are literal.
//! - Unlike `=`: `"A"` ≠ `"a"`, `TRUE` ≠ `1`, and `0.1+0.2` ≠ `0.3` when
//!   the formatted strings differ (EXACT is not 15-digit numeric equality).
//! - `EXACT(1, "1")` is TRUE because the number coerces to `"1"`.
//! - Errors propagate left-to-right. Wrong arity is handled by the caller
//!   (`#VALUE!`).
//!
//! Production path borrows text / bool / empty (no `String` clone), rejects
//! on UTF-8 length, then `memcmp`. Integers in the `format_plain` short
//! path compare without formatting. The `to_text` + `Vec<char>` baseline
//! lives beside that path so benches can print before/after.

use super::coerce;
use xlsx_types::{ExcelError, ExcelValue};

/// Production `EXACT` kernel.
pub fn exact(a: &ExcelValue, b: &ExcelValue) -> Result<bool, ExcelError> {
    if let ExcelValue::Error(e) = a {
        return Err(*e);
    }
    if let ExcelValue::Error(e) = b {
        return Err(*e);
    }
    if matches!(a, ExcelValue::Array(_)) || matches!(b, ExcelValue::Array(_)) {
        return Err(ExcelError::Value);
    }
    Ok(exact_fast(a, b))
}

/// `to_text` + `Vec<char>` baseline used for the hill-climb bench.
///
/// Same Excel semantics as [`exact`]. Kept so
/// `cargo bench -p xlsx-engine-core --bench exact` can print before/after.
pub fn exact_naive(a: &ExcelValue, b: &ExcelValue) -> Result<bool, ExcelError> {
    let a = coerce::to_text(a)?;
    let b = coerce::to_text(b)?;
    let ca: Vec<char> = a.chars().collect();
    let cb: Vec<char> = b.chars().collect();
    Ok(ca == cb)
}

fn exact_fast(a: &ExcelValue, b: &ExcelValue) -> bool {
    match (a, b) {
        (ExcelValue::Text(x), ExcelValue::Text(y)) => eq_text(x, y),
        (ExcelValue::Empty, ExcelValue::Empty) => true,
        (ExcelValue::Empty, ExcelValue::Text(s)) | (ExcelValue::Text(s), ExcelValue::Empty) => {
            s.is_empty()
        }
        (ExcelValue::Bool(x), ExcelValue::Bool(y)) => x == y,
        (ExcelValue::Bool(true), ExcelValue::Text(s))
        | (ExcelValue::Text(s), ExcelValue::Bool(true)) => eq_text(s, "TRUE"),
        (ExcelValue::Bool(false), ExcelValue::Text(s))
        | (ExcelValue::Text(s), ExcelValue::Bool(false)) => eq_text(s, "FALSE"),
        (ExcelValue::Number(n), ExcelValue::Number(m)) => numbers_exact(*n, *m),
        (ExcelValue::Number(n), ExcelValue::Text(s))
        | (ExcelValue::Text(s), ExcelValue::Number(n)) => number_eq_text(*n, s),
        // Number / bool / empty never share a text form:
        // "1"≠"TRUE", "0"≠"FALSE", numbers never format to "".
        (ExcelValue::Number(_), ExcelValue::Bool(_))
        | (ExcelValue::Bool(_), ExcelValue::Number(_))
        | (ExcelValue::Number(_), ExcelValue::Empty)
        | (ExcelValue::Empty, ExcelValue::Number(_))
        | (ExcelValue::Bool(_), ExcelValue::Empty)
        | (ExcelValue::Empty, ExcelValue::Bool(_)) => false,
        (ExcelValue::Error(_), _) | (_, ExcelValue::Error(_)) => unreachable!("errors stripped"),
        (ExcelValue::Array(_), _) | (_, ExcelValue::Array(_)) => unreachable!("arrays stripped"),
    }
}

/// UTF-8 byte equality. Different lengths cannot match; same pointer is
/// identity; otherwise `memcmp`.
fn eq_text(a: &str, b: &str) -> bool {
    a.len() == b.len() && (std::ptr::eq(a.as_ptr(), b.as_ptr()) || a.as_bytes() == b.as_bytes())
}

fn numbers_exact(a: f64, b: f64) -> bool {
    if a.to_bits() == b.to_bits() {
        return true;
    }
    // ±0 both format as "0" on the integer `format_plain` path.
    if a == 0.0 && b == 0.0 {
        return true;
    }
    let a_int = is_plain_int(a);
    let b_int = is_plain_int(b);
    if a_int && b_int {
        return (a as i64) == (b as i64);
    }
    coerce::format_plain(a) == coerce::format_plain(b)
}

fn number_eq_text(n: f64, s: &str) -> bool {
    if is_plain_int(n) {
        return int_eq_text(n as i64, s);
    }
    coerce::format_plain(n) == s
}

fn is_plain_int(n: f64) -> bool {
    n.fract() == 0.0 && n.abs() < 1e15
}

/// Stack `itoa` matching `format!("{n:.0}")` for integers in range.
fn int_eq_text(n: i64, s: &str) -> bool {
    let mut buf = [0u8; 20];
    write_i64(&mut buf, n) == s
}

fn write_i64(buf: &mut [u8; 20], n: i64) -> &str {
    if n == 0 {
        buf[0] = b'0';
        return "0";
    }
    let neg = n < 0;
    let mut v = n.unsigned_abs();
    let mut i = 20;
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    if neg {
        i -= 1;
        buf[i] = b'-';
    }
    // SAFETY: digits and an optional ASCII '-' are valid UTF-8.
    unsafe { std::str::from_utf8_unchecked(&buf[i..]) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::eval_formula_in;
    use xlsx_types::{Cell, ExcelError, ExcelValue, Sheet, Workbook};

    fn both(a: &ExcelValue, b: &ExcelValue) -> Result<bool, ExcelError> {
        let fast = exact(a, b);
        let slow = exact_naive(a, b);
        assert_eq!(fast, slow, "naive/fast mismatch for {a:?} vs {b:?}");
        fast
    }

    fn t(s: &str) -> ExcelValue {
        ExcelValue::Text(s.into())
    }

    #[test]
    fn microsoft_examples() {
        assert_eq!(both(&t("word"), &t("word")), Ok(true));
        assert_eq!(both(&t("Word"), &t("word")), Ok(false));
        assert_eq!(both(&t("w ord"), &t("word")), Ok(false));
    }

    #[test]
    fn case_sensitive_unlike_eq() {
        assert_eq!(both(&t("A"), &t("a")), Ok(false));
        assert_eq!(both(&t("A"), &t("A")), Ok(true));
        assert_eq!(both(&t("AbC"), &t("AbC")), Ok(true));
        assert_eq!(both(&t("TRUE"), &t("true")), Ok(false));
    }

    #[test]
    fn spaces_and_wildcards_are_literal() {
        assert_eq!(both(&t("A "), &t("A")), Ok(false));
        assert_eq!(both(&t(" A"), &t("A")), Ok(false));
        assert_eq!(both(&t(""), &t("")), Ok(true));
        assert_eq!(both(&t(""), &t(" ")), Ok(false));
        assert_eq!(both(&t("a*"), &t("a*")), Ok(true));
        assert_eq!(both(&t("a*"), &t("abc")), Ok(false));
        assert_eq!(both(&t("a?"), &t("ab")), Ok(false));
        assert_eq!(both(&t("\t"), &t(" ")), Ok(false));
        assert_eq!(both(&t("\u{00a0}"), &t(" ")), Ok(false));
    }

    #[test]
    fn unicode_scalars() {
        assert_eq!(both(&t("café"), &t("café")), Ok(true));
        assert_eq!(both(&t("café"), &t("cafe")), Ok(false));
        assert_eq!(both(&t("日本語"), &t("日本語")), Ok(true));
        assert_eq!(both(&t("日本語"), &t("日本")), Ok(false));
        assert_eq!(both(&t("😀"), &t("😀")), Ok(true));
        assert_eq!(both(&t("😀"), &t("🎉")), Ok(false));
    }

    #[test]
    fn number_and_bool_coercion() {
        assert_eq!(
            both(&ExcelValue::Number(1.0), &ExcelValue::Number(1.0)),
            Ok(true)
        );
        assert_eq!(both(&ExcelValue::Number(1.0), &t("1")), Ok(true));
        assert_eq!(both(&ExcelValue::Number(1.0), &t("1.0")), Ok(false));
        assert_eq!(both(&ExcelValue::Number(1.5), &t("1.5")), Ok(true));
        assert_eq!(both(&ExcelValue::Number(-2.0), &t("-2")), Ok(true));
        assert_eq!(both(&ExcelValue::Number(0.0), &t("0")), Ok(true));
        assert_eq!(
            both(&ExcelValue::Number(-0.0), &ExcelValue::Number(0.0)),
            Ok(true)
        );
        assert_eq!(both(&ExcelValue::Bool(true), &t("TRUE")), Ok(true));
        assert_eq!(both(&ExcelValue::Bool(true), &t("true")), Ok(false));
        assert_eq!(
            both(&ExcelValue::Bool(true), &ExcelValue::Number(1.0)),
            Ok(false)
        );
        assert_eq!(both(&ExcelValue::Bool(false), &t("FALSE")), Ok(true));
        assert_eq!(
            both(&ExcelValue::Bool(false), &ExcelValue::Number(0.0)),
            Ok(false)
        );
        assert_eq!(
            both(&ExcelValue::Bool(true), &ExcelValue::Bool(true)),
            Ok(true)
        );
        assert_eq!(
            both(&ExcelValue::Bool(true), &ExcelValue::Bool(false)),
            Ok(false)
        );
    }

    #[test]
    fn empty_vs_text_and_number() {
        assert_eq!(both(&ExcelValue::Empty, &ExcelValue::Empty), Ok(true));
        assert_eq!(both(&ExcelValue::Empty, &t("")), Ok(true));
        assert_eq!(both(&ExcelValue::Empty, &t("0")), Ok(false));
        assert_eq!(both(&ExcelValue::Empty, &ExcelValue::Number(0.0)), Ok(false));
        assert_eq!(both(&ExcelValue::Empty, &ExcelValue::Bool(false)), Ok(false));
    }

    #[test]
    fn ieee_formatted_strings_differ() {
        // EXACT is text, not 15-digit `=`. 0.1+0.2 formats longer than 0.3.
        let sum = ExcelValue::Number(0.1 + 0.2);
        let third = ExcelValue::Number(0.3);
        assert_eq!(both(&sum, &third), Ok(false));
        assert_eq!(
            coerce::format_plain(0.1 + 0.2),
            "0.30000000000000004"
        );
        assert_eq!(coerce::format_plain(0.3), "0.3");
    }

    #[test]
    fn errors_left_to_right() {
        assert_eq!(
            both(&ExcelValue::Error(ExcelError::Div0), &t("x")),
            Err(ExcelError::Div0)
        );
        assert_eq!(
            both(&t("x"), &ExcelValue::Error(ExcelError::Na)),
            Err(ExcelError::Na)
        );
        assert_eq!(
            both(
                &ExcelValue::Error(ExcelError::Div0),
                &ExcelValue::Error(ExcelError::Na)
            ),
            Err(ExcelError::Div0)
        );
        assert_eq!(
            exact(&ExcelValue::Array(vec![]), &t("x")),
            Err(ExcelError::Value)
        );
    }

    #[test]
    fn large_and_identity() {
        let a = "x".repeat(4096);
        let b = format!("{}y", "x".repeat(4095));
        assert_eq!(both(&t(&a), &t(&a)), Ok(true));
        assert_eq!(both(&t(&a), &t(&b)), Ok(false));
        let cafe = "café".repeat(256);
        assert_eq!(both(&t(&cafe), &t(&cafe)), Ok(true));
        let same = t("hello");
        assert_eq!(both(&same, &same), Ok(true));
    }

    #[test]
    fn write_i64_matches_format_plain() {
        for n in [0i64, 1, -1, 42, -42, 999_999_999_999_999, -999_999_999_999_999] {
            let mut buf = [0u8; 20];
            assert_eq!(write_i64(&mut buf, n), format!("{n:.0}"));
            assert!(int_eq_text(n, &format!("{n:.0}")));
            assert!(!int_eq_text(n, "nope"));
        }
    }

    #[test]
    fn formula_coercion_and_arity() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=EXACT(\"word\",\"word\")").unwrap(),
            ExcelValue::Bool(true)
        );
        assert_eq!(
            eval_formula_in(&wb, "=EXACT(\"Word\",\"word\")").unwrap(),
            ExcelValue::Bool(false)
        );
        assert_eq!(
            eval_formula_in(&wb, "=EXACT(\"A\",\"a\")").unwrap(),
            ExcelValue::Bool(false)
        );
        assert_eq!(
            eval_formula_in(&wb, "=EXACT(1,\"1\")").unwrap(),
            ExcelValue::Bool(true)
        );
        assert_eq!(
            eval_formula_in(&wb, "=EXACT(TRUE,\"TRUE\")").unwrap(),
            ExcelValue::Bool(true)
        );
        assert_eq!(
            eval_formula_in(&wb, "=EXACT(TRUE,1)").unwrap(),
            ExcelValue::Bool(false)
        );
        assert_eq!(
            eval_formula_in(&wb, "=EXACT(TRUE,TRUE)").unwrap(),
            ExcelValue::Bool(true)
        );
        assert_eq!(
            eval_formula_in(&wb, "=EXACT(0.1+0.2,0.3)").unwrap(),
            ExcelValue::Bool(false)
        );
        assert_eq!(
            eval_formula_in(&wb, "=EXACT()").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=EXACT(\"a\")").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=EXACT(\"a\",\"b\",\"c\")").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=EXACT(1/0,\"x\")").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=EXACT(NA(),1/0)").unwrap(),
            ExcelValue::Error(ExcelError::Na)
        );
        assert_eq!(
            eval_formula_in(&wb, "=EXACT(UPPER(\"word\"),\"WORD\")").unwrap(),
            ExcelValue::Bool(true)
        );
        assert_eq!(
            eval_formula_in(&wb, "=EXACT(\"a\"&\"b\",\"ab\")").unwrap(),
            ExcelValue::Bool(true)
        );
    }

    #[test]
    fn formula_blank_and_cell() {
        let mut sheet = Sheet::new("Sheet1");
        sheet
            .cells
            .insert("A1".into(), Cell::value(ExcelValue::Text("Hello".into())));
        sheet
            .cells
            .insert("A2".into(), Cell::value(ExcelValue::Text("hello".into())));
        sheet
            .cells
            .insert("A3".into(), Cell::value(ExcelValue::Empty));
        sheet
            .cells
            .insert("A4".into(), Cell::value(ExcelValue::Text(String::new())));
        let wb = Workbook {
            sheets: vec![sheet],
            names: vec![],
        };
        assert_eq!(
            eval_formula_in(&wb, "=EXACT(A1,\"Hello\")").unwrap(),
            ExcelValue::Bool(true)
        );
        assert_eq!(
            eval_formula_in(&wb, "=EXACT(A1,A2)").unwrap(),
            ExcelValue::Bool(false)
        );
        assert_eq!(
            eval_formula_in(&wb, "=EXACT(A3,\"\")").unwrap(),
            ExcelValue::Bool(true)
        );
        assert_eq!(
            eval_formula_in(&wb, "=EXACT(A3,A4)").unwrap(),
            ExcelValue::Bool(true)
        );
        assert_eq!(
            eval_formula_in(&wb, "=EXACT(A3,0)").unwrap(),
            ExcelValue::Bool(false)
        );
        assert_eq!(
            eval_formula_in(&wb, "=EXACT(B1,\"\")").unwrap(),
            ExcelValue::Bool(true)
        );
    }
}
