//! Excel `LEN` kernel.
//!
//! Semantics (desktop Excel / Microsoft docs):
//! - `LEN(text)` returns the number of **characters** in `text`.
//! - Character indexing matches this crate's `MID` / `LEFT` / `RIGHT` /
//!   `REPLACE` / `UNICODE`: Unicode scalar values (`str::chars`). That is
//!   Excel Compatibility Version 2 — a supplementary-plane emoji is **one**
//!   character (`LEN("😀")` is `1`, not `2`). Version 1 counted UTF-16
//!   code units; that legacy mode is not implemented. Combining marks and
//!   variation selectors stay separate scalars (not grapheme clusters).
//! - Empty text — literal `""`, a stored empty string, or a blank cell
//!   after `&`-style coercion — is `0` (unlike `UNICODE`, which is
//!   `#VALUE!`).
//! - Spaces, tabs, NBSP, and C0 controls each count as one.
//! - Numbers / bools coerce like `&` (`TRUE` → `"TRUE"` → `4`, `65` →
//!   `"65"` → `2`) before the count. Errors propagate. Wrong arity is
//!   `#VALUE!`.
//! - `LENB` (DBCS / byte count) is out of scope.
//!
//! Production path counts UTF-8 scalars without allocating: ASCII is the
//! byte length after `is_ascii`; mixed UTF-8 is `len −` SWAR-counted
//! continuation bytes. `Text` is borrowed (no `to_text`
//! clone). Integers in the `format_plain` short path use a digit count
//! instead of formatting. The `to_text` + `Vec<char>` baseline lives
//! beside that path so benches can print before/after. This kernel does
//! **not** read fixture goldens.

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

/// Production `LEN` kernel on already-coerced text.
pub fn len(text: &str) -> f64 {
    scalar_count(text) as f64
}

/// `to_text` + `Vec<char>` baseline used for the hill-climb bench.
///
/// Same Excel semantics as [`len`]. Kept so
/// `cargo bench -p xlsx-engine-core --bench len` can print before/after.
pub fn len_naive(text: &str) -> f64 {
    let chars: Vec<char> = text.chars().collect();
    chars.len() as f64
}

/// Production `LEN` on a scalar Excel value (no `Text` clone).
pub fn len_value(v: &ExcelValue) -> Result<f64, ExcelError> {
    match v {
        ExcelValue::Error(e) => Err(*e),
        ExcelValue::Empty => Ok(0.0),
        ExcelValue::Text(s) => Ok(len(s)),
        ExcelValue::Bool(true) => Ok(4.0),  // "TRUE"
        ExcelValue::Bool(false) => Ok(5.0), // "FALSE"
        ExcelValue::Number(n) => Ok(len_number(*n)),
        ExcelValue::Array(_) => Err(ExcelError::Value),
    }
}

/// Value-level baseline: full `to_text` clone + [`len_naive`].
pub fn len_value_naive(v: &ExcelValue) -> Result<f64, ExcelError> {
    Ok(len_naive(&coerce::to_text(v)?))
}

/// Digit count of [`coerce::format_plain`] without allocating the integer
/// short path (`abs < 1e15` and no fractional part).
fn len_number(n: f64) -> f64 {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        // `format!("{n:.0}")` prints `-0` for negative zero.
        if n.to_bits() == (-0.0_f64).to_bits() {
            return 2.0;
        }
        return int_digit_count(n as i64) as f64;
    }
    coerce::format_plain(n).len() as f64
}

fn int_digit_count(n: i64) -> usize {
    if n == 0 {
        return 1;
    }
    let digits = n.unsigned_abs().ilog10() as usize + 1;
    if n < 0 {
        digits + 1
    } else {
        digits
    }
}

/// Unicode scalar count = UTF-8 byte length minus continuation bytes.
///
/// ASCII uses the slice `is_ascii` probe (byte length). Mixed UTF-8 is
/// `len −` SWAR-counted `10xxxxxx` continuation bytes.
#[inline]
pub fn scalar_count(text: &str) -> usize {
    let bytes = text.as_bytes();
    if bytes.is_ascii() {
        return bytes.len();
    }
    bytes.len() - continuation_count(bytes)
}

const HI8: u64 = 0x8080_8080_8080_8080;
const B6_8: u64 = 0x4040_4040_4040_4040;
const HI16: u128 = 0x8080_8080_8080_8080_8080_8080_8080_8080;
const B6_16: u128 = 0x4040_4040_4040_4040_4040_4040_4040_4040;

/// Count UTF-8 continuation bytes (`10xxxxxx`) in 16-byte SWAR chunks.
fn continuation_count(bytes: &[u8]) -> usize {
    let n = bytes.len();
    let mut i = 0;
    let mut cont = 0usize;
    while i + 32 <= n {
        let a = u128::from_ne_bytes(bytes[i..i + 16].try_into().unwrap());
        let b = u128::from_ne_bytes(bytes[i + 16..i + 32].try_into().unwrap());
        cont += cont_ones_u128(a) + cont_ones_u128(b);
        i += 32;
    }
    while i + 16 <= n {
        let v = u128::from_ne_bytes(bytes[i..i + 16].try_into().unwrap());
        cont += cont_ones_u128(v);
        i += 16;
    }
    while i + 8 <= n {
        let v = u64::from_ne_bytes(bytes[i..i + 8].try_into().unwrap());
        let c = ((v & HI8) >> 1) & (!v & B6_8);
        cont += c.count_ones() as usize;
        i += 8;
    }
    while i < n {
        let b = bytes[i];
        cont += (b >= 0x80 && b < 0xC0) as usize;
        i += 1;
    }
    cont
}

#[inline]
fn cont_ones_u128(v: u128) -> usize {
    let c = ((v & HI16) >> 1) & (!v & B6_16);
    c.count_ones() as usize
}

/// Production LEN (scalar arg, UTF-8 scalar-count kernel).
pub(crate) fn fn_len(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() != 1 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    match len_value(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(n) => Ok(ExcelValue::Number(n)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::eval_formula_in;
    use xlsx_types::{Cell, ExcelError, ExcelValue, Sheet, Workbook};

    fn both(text: &str) -> f64 {
        let fast = len(text);
        let slow = len_naive(text);
        assert_eq!(fast, slow, "naive/fast mismatch for {text:?}");
        fast
    }

    fn both_value(v: &ExcelValue) -> Result<f64, ExcelError> {
        let fast = len_value(v);
        let slow = len_value_naive(v);
        assert_eq!(fast, slow, "value naive/fast mismatch for {v:?}");
        fast
    }

    #[test]
    fn microsoft_examples() {
        assert_eq!(both("Phoenix, AZ"), 11.0);
        assert_eq!(both(""), 0.0);
    }

    #[test]
    fn ascii_and_spaces() {
        assert_eq!(both("abc"), 3.0);
        assert_eq!(both(" "), 1.0);
        assert_eq!(both("  abc  "), 7.0);
        assert_eq!(both("A"), 1.0);
        assert_eq!(both("Hello"), 5.0);
    }

    #[test]
    fn empty_is_zero() {
        assert_eq!(both(""), 0.0);
        assert_eq!(both_value(&ExcelValue::Empty).unwrap(), 0.0);
        assert_eq!(both_value(&ExcelValue::Text(String::new())).unwrap(), 0.0);
    }

    #[test]
    fn ascii_controls() {
        assert_eq!(both("\t"), 1.0);
        assert_eq!(both("\n"), 1.0);
        assert_eq!(both("\r"), 1.0);
        assert_eq!(both("\u{0000}"), 1.0);
        assert_eq!(both("\u{001f}"), 1.0);
        assert_eq!(both("a\nb"), 3.0);
    }

    #[test]
    fn latin1_and_bmp() {
        assert_eq!(both("é"), 1.0);
        assert_eq!(both("café"), 4.0);
        assert_eq!(both("€"), 1.0);
        assert_eq!(both("α"), 1.0);
        assert_eq!(both("中"), 1.0);
        assert_eq!(both("日語"), 2.0);
        assert_eq!(both("\u{00a0}"), 1.0);
        assert_eq!(both("a\u{00a0}b"), 3.0);
    }

    #[test]
    fn supplementary_plane_is_one_scalar() {
        // U+1F600 GRINNING FACE — Compat v2 (not UTF-16 length 2).
        assert_eq!(both("😀"), 1.0);
        assert_eq!(both("a😀"), 2.0);
        assert_eq!(both("😀b"), 2.0);
        assert_eq!(both("🎉"), 1.0);
        assert_eq!(both("😀😀😀"), 3.0);
    }

    #[test]
    fn combining_mark_is_its_own_scalar() {
        assert_eq!(both("e\u{0301}"), 2.0);
        assert_eq!(both("\u{0301}"), 1.0);
        assert_eq!(both("café"), 4.0); // precomposed
        assert_eq!(both("cafe\u{0301}"), 5.0);
    }

    #[test]
    fn value_coercion_matches_ampersand() {
        assert_eq!(both_value(&ExcelValue::Number(123.0)).unwrap(), 3.0);
        assert_eq!(both_value(&ExcelValue::Number(0.0)).unwrap(), 1.0);
        assert_eq!(both_value(&ExcelValue::Number(-12.0)).unwrap(), 3.0);
        assert_eq!(both_value(&ExcelValue::Number(1.5)).unwrap(), 3.0);
        assert_eq!(both_value(&ExcelValue::Number(65.0)).unwrap(), 2.0);
        assert_eq!(both_value(&ExcelValue::Bool(true)).unwrap(), 4.0);
        assert_eq!(both_value(&ExcelValue::Bool(false)).unwrap(), 5.0);
        assert_eq!(
            both_value(&ExcelValue::Error(ExcelError::Div0)),
            Err(ExcelError::Div0)
        );
        assert_eq!(
            both_value(&ExcelValue::Error(ExcelError::Na)),
            Err(ExcelError::Na)
        );
        assert_eq!(
            both_value(&ExcelValue::Array(vec![vec![ExcelValue::Text("A".into())]])),
            Err(ExcelError::Value)
        );
    }

    #[test]
    fn negative_zero_formats_as_minus_zero() {
        assert_eq!(
            both_value(&ExcelValue::Number(-0.0)).unwrap(),
            coerce::format_plain(-0.0).len() as f64
        );
        assert_eq!(both_value(&ExcelValue::Number(-0.0)).unwrap(), 2.0);
    }

    #[test]
    fn integer_digit_count_matches_format_plain() {
        for n in [
            0.0, 1.0, 9.0, 10.0, 99.0, 100.0, -1.0, -9.0, -10.0, 1e14, -1e14,
        ] {
            let expected = coerce::format_plain(n).len() as f64;
            assert_eq!(len_number(n), expected, "{n}");
            assert_eq!(
                both_value(&ExcelValue::Number(n)).unwrap(),
                expected,
                "value {n}"
            );
        }
    }

    #[test]
    fn scalar_count_matches_chars() {
        for s in [
            "",
            "A",
            "abc",
            "\u{007f}",
            "é",
            "café",
            "€",
            "中",
            "😀",
            "e\u{0301}",
            "a😀b",
            "\u{10ffff}",
            "  spaces  ",
        ] {
            assert_eq!(scalar_count(s), s.chars().count(), "{s:?}");
            assert_eq!(both(s), s.chars().count() as f64, "{s:?}");
        }
        let long_ascii = "x".repeat(200_000);
        assert_eq!(scalar_count(&long_ascii), 200_000);
        assert_eq!(both(&long_ascii), 200_000.0);
        let long_emoji = "😀".repeat(10_000);
        assert_eq!(scalar_count(&long_emoji), 10_000);
        assert_eq!(both(&long_emoji), 10_000.0);
        let long_cafe = "é".repeat(50_000);
        assert_eq!(scalar_count(&long_cafe), 50_000);
        assert_eq!(both(&long_cafe), 50_000.0);
    }

    #[test]
    fn formula_microsoft_and_empty() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=LEN(\"Phoenix, AZ\")").unwrap(),
            ExcelValue::Number(11.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(\"abc\")").unwrap(),
            ExcelValue::Number(3.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(\"\")").unwrap(),
            ExcelValue::Number(0.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(\"  a  b  \")").unwrap(),
            ExcelValue::Number(8.0)
        );
    }

    #[test]
    fn formula_coercion_and_arity() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=LEN(123)").unwrap(),
            ExcelValue::Number(3.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(1.5)").unwrap(),
            ExcelValue::Number(3.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(-12)").unwrap(),
            ExcelValue::Number(3.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(TRUE)").unwrap(),
            ExcelValue::Number(4.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(FALSE)").unwrap(),
            ExcelValue::Number(5.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN()").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(\"a\",\"b\")").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(1/0)").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(NA())").unwrap(),
            ExcelValue::Error(ExcelError::Na)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(LOWER(\"AbC\"))").unwrap(),
            ExcelValue::Number(3.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(LEFT(\"xyz\",2))").unwrap(),
            ExcelValue::Number(2.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(\"A\")+1").unwrap(),
            ExcelValue::Number(2.0)
        );
    }

    #[test]
    fn formula_blank_cell_and_named() {
        let mut sheet = Sheet::new("Sheet1");
        sheet
            .cells
            .insert("A1".into(), Cell::value(ExcelValue::Text("Hello".into())));
        sheet
            .cells
            .insert("A2".into(), Cell::value(ExcelValue::Empty));
        sheet
            .cells
            .insert("A3".into(), Cell::value(ExcelValue::Text(String::new())));
        sheet
            .cells
            .insert("A4".into(), Cell::value(ExcelValue::Text("😀x".into())));
        let wb = Workbook {
            sheets: vec![sheet],
            names: vec![xlsx_types::DefinedName {
                name: "Title".into(),
                refers_to: "Sheet1!A1".into(),
            }],
        };
        assert_eq!(
            eval_formula_in(&wb, "=LEN(A1)").unwrap(),
            ExcelValue::Number(5.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(A2)").unwrap(),
            ExcelValue::Number(0.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(A3)").unwrap(),
            ExcelValue::Number(0.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(B1)").unwrap(),
            ExcelValue::Number(0.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(A4)").unwrap(),
            ExcelValue::Number(2.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(Title)").unwrap(),
            ExcelValue::Number(5.0)
        );
    }

    #[test]
    fn formula_unicode_scalars() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=LEN(\"é\")").unwrap(),
            ExcelValue::Number(1.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(\"café\")").unwrap(),
            ExcelValue::Number(4.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(\"αβ\")").unwrap(),
            ExcelValue::Number(2.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(\"中文\")").unwrap(),
            ExcelValue::Number(2.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=SUM(MAP({\"A\";\"BB\";\"CCC\"},LAMBDA(x,LEN(x))))").unwrap(),
            ExcelValue::Number(6.0)
        );
    }
}
