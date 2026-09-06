//! Excel `UNICODE` kernel.
//!
//! Semantics (desktop Excel / Microsoft docs):
//! - `UNICODE(text)` returns the Unicode **code point** of the **first**
//!   character of `text`. Later characters are ignored.
//! - Character indexing matches this crate's `LEN` / `MID` / `LEFT` /
//!   `RIGHT` / `REPLACE`: Unicode scalar values (`str::chars`). That is
//!   Excel Compatibility Version 2 — a supplementary-plane emoji is **one**
//!   character (`UNICODE("😀")` is `128512`, not the high surrogate).
//!   Version 1 counted UTF-16 code units; that legacy mode is not
//!   implemented. Combining marks and variation selectors stay separate
//!   scalars.
//! - Empty text — literal `""`, a stored empty string, or a blank cell
//!   after `&`-style coercion — is `#VALUE!`.
//! - Numbers / bools coerce like `&` (`TRUE` → `"TRUE"` → `84`, `65` →
//!   `"65"` → `54`) before the first-scalar read. Errors propagate.
//!   Wrong arity is `#VALUE!`.
//!
//! Production path peeks the first UTF-8 scalar in O(1) (ASCII is the first
//! byte; 2/3/4-byte sequences decode without walking the rest of the
//! string) and never clones `Text`. The `to_text` + `Vec<char>` baseline
//! lives beside that path so benches can print before/after. This kernel
//! does **not** read fixture goldens.

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

/// Production `UNICODE` kernel on already-coerced text.
///
/// Empty `text` is `#VALUE!`.
pub fn unicode(text: &str) -> Result<f64, ExcelError> {
    match first_code_point(text) {
        Some(cp) => Ok(cp as f64),
        None => Err(ExcelError::Value),
    }
}

/// `to_text` + `Vec<char>` baseline used for the hill-climb bench.
///
/// Same Excel semantics as [`unicode`]. Kept so
/// `cargo bench -p xlsx-engine-core --bench unicode` can print before/after.
pub fn unicode_naive(text: &str) -> Result<f64, ExcelError> {
    let chars: Vec<char> = text.chars().collect();
    match chars.first() {
        Some(c) => Ok(*c as u32 as f64),
        None => Err(ExcelError::Value),
    }
}

/// Production `UNICODE` on a scalar Excel value (no `Text` clone).
pub fn unicode_value(v: &ExcelValue) -> Result<f64, ExcelError> {
    match v {
        ExcelValue::Error(e) => Err(*e),
        ExcelValue::Empty => Err(ExcelError::Value),
        ExcelValue::Text(s) => unicode(s),
        ExcelValue::Bool(true) => Ok(b'T' as f64),
        ExcelValue::Bool(false) => Ok(b'F' as f64),
        ExcelValue::Number(n) => unicode(&coerce::format_plain(*n)),
        ExcelValue::Array(_) => Err(ExcelError::Value),
    }
}

/// Value-level baseline: full `to_text` clone + [`unicode_naive`].
pub fn unicode_value_naive(v: &ExcelValue) -> Result<f64, ExcelError> {
    unicode_naive(&coerce::to_text(v)?)
}

/// First Unicode scalar as a code point, or `None` when `text` is empty.
///
/// `text` is valid UTF-8. ASCII is one byte; longer scalars are decoded
/// from the leading 2/3/4-byte sequence without scanning the suffix.
#[inline]
pub fn first_code_point(text: &str) -> Option<u32> {
    let b = text.as_bytes();
    if b.is_empty() {
        return None;
    }
    let b0 = b[0];
    if b0 < 0x80 {
        return Some(b0 as u32);
    }
    // SAFETY: `text` is valid UTF-8 and the first byte is a non-ASCII
    // lead, so the documented continuation bytes exist.
    if b0 < 0xE0 {
        Some(((b0 as u32 & 0x1F) << 6) | (b[1] as u32 & 0x3F))
    } else if b0 < 0xF0 {
        Some(((b0 as u32 & 0x0F) << 12) | ((b[1] as u32 & 0x3F) << 6) | (b[2] as u32 & 0x3F))
    } else {
        Some(
            ((b0 as u32 & 0x07) << 18)
                | ((b[1] as u32 & 0x3F) << 12)
                | ((b[2] as u32 & 0x3F) << 6)
                | (b[3] as u32 & 0x3F),
        )
    }
}

/// Production UNICODE (scalar arg, first-code-point kernel).
pub(crate) fn fn_unicode(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() != 1 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    match unicode_value(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(n) => Ok(ExcelValue::Number(n)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::eval_formula_in;
    use xlsx_types::{Cell, ExcelError, ExcelValue, Sheet, Workbook};

    fn both(text: &str) -> Result<f64, ExcelError> {
        let fast = unicode(text);
        let slow = unicode_naive(text);
        assert_eq!(fast, slow, "naive/fast mismatch for {text:?}");
        fast
    }

    fn both_value(v: &ExcelValue) -> Result<f64, ExcelError> {
        let fast = unicode_value(v);
        let slow = unicode_value_naive(v);
        assert_eq!(fast, slow, "value naive/fast mismatch for {v:?}");
        fast
    }

    #[test]
    fn microsoft_examples() {
        assert_eq!(both(" ").unwrap(), 32.0);
        assert_eq!(both("A").unwrap(), 65.0);
    }

    #[test]
    fn first_scalar_only() {
        assert_eq!(both("ABC").unwrap(), 65.0);
        assert_eq!(both("abc").unwrap(), 97.0);
        assert_eq!(both("Zzz").unwrap(), 90.0);
        assert_eq!(both("0").unwrap(), 48.0);
        assert_eq!(both("9x").unwrap(), 57.0);
    }

    #[test]
    fn empty_is_value() {
        assert_eq!(both(""), Err(ExcelError::Value));
        assert_eq!(both_value(&ExcelValue::Empty), Err(ExcelError::Value));
        assert_eq!(
            both_value(&ExcelValue::Text(String::new())),
            Err(ExcelError::Value)
        );
    }

    #[test]
    fn ascii_controls_and_del() {
        assert_eq!(both("\t").unwrap(), 9.0);
        assert_eq!(both("\n").unwrap(), 10.0);
        assert_eq!(both("\r").unwrap(), 13.0);
        assert_eq!(both("\u{0000}").unwrap(), 0.0);
        assert_eq!(both("\u{001f}").unwrap(), 31.0);
        assert_eq!(both("\u{007f}").unwrap(), 127.0);
    }

    #[test]
    fn latin1_and_bmp() {
        assert_eq!(both("é").unwrap(), 233.0);
        assert_eq!(both("€").unwrap(), 8364.0);
        assert_eq!(both("α").unwrap(), 945.0);
        assert_eq!(both("中").unwrap(), 20013.0);
        assert_eq!(both("日語").unwrap(), 26085.0);
        assert_eq!(both("\u{00a0}").unwrap(), 160.0);
        assert_eq!(both("\u{feff}").unwrap(), 65279.0);
    }

    #[test]
    fn supplementary_plane_is_one_scalar() {
        // U+1F600 GRINNING FACE — Compat v2 (not UTF-16 high surrogate 55357).
        assert_eq!(both("😀").unwrap(), 128512.0);
        assert_eq!(both("a😀").unwrap(), 97.0);
        assert_eq!(both("😀b").unwrap(), 128512.0);
        assert_eq!(both("🎉").unwrap(), 127881.0);
    }

    #[test]
    fn combining_mark_is_its_own_scalar() {
        assert_eq!(both("e\u{0301}").unwrap(), 101.0);
        assert_eq!(both("\u{0301}").unwrap(), 769.0);
    }

    #[test]
    fn value_coercion_matches_ampersand() {
        assert_eq!(both_value(&ExcelValue::Number(65.0)).unwrap(), 54.0);
        assert_eq!(both_value(&ExcelValue::Number(0.0)).unwrap(), 48.0);
        assert_eq!(both_value(&ExcelValue::Number(-12.0)).unwrap(), 45.0);
        assert_eq!(both_value(&ExcelValue::Number(1.5)).unwrap(), 49.0);
        assert_eq!(both_value(&ExcelValue::Bool(true)).unwrap(), 84.0);
        assert_eq!(both_value(&ExcelValue::Bool(false)).unwrap(), 70.0);
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
    fn first_code_point_matches_chars() {
        for s in [
            "",
            "A",
            "\u{007f}",
            "é",
            "€",
            "中",
            "😀",
            "e\u{0301}",
            "\u{10ffff}",
        ] {
            assert_eq!(
                first_code_point(s),
                s.chars().next().map(|c| c as u32),
                "{s:?}"
            );
        }
        let long = format!("Ω{}", "x".repeat(200_000));
        assert_eq!(first_code_point(&long), Some(937));
        assert_eq!(both(&long).unwrap(), 937.0);
    }

    #[test]
    fn formula_microsoft_and_first_only() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=UNICODE(\" \")").unwrap(),
            ExcelValue::Number(32.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=UNICODE(\"A\")").unwrap(),
            ExcelValue::Number(65.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=UNICODE(\"ABC\")").unwrap(),
            ExcelValue::Number(65.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=UNICODE(\"\")").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn formula_coercion_and_arity() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=UNICODE(65)").unwrap(),
            ExcelValue::Number(54.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=UNICODE(1.5)").unwrap(),
            ExcelValue::Number(49.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=UNICODE(-2)").unwrap(),
            ExcelValue::Number(45.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=UNICODE(TRUE)").unwrap(),
            ExcelValue::Number(84.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=UNICODE(FALSE)").unwrap(),
            ExcelValue::Number(70.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=UNICODE()").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=UNICODE(\"a\",\"b\")").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=UNICODE(1/0)").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=UNICODE(NA())").unwrap(),
            ExcelValue::Error(ExcelError::Na)
        );
        assert_eq!(
            eval_formula_in(&wb, "=UNICODE(LOWER(\"A\"))").unwrap(),
            ExcelValue::Number(97.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=UNICODE(LEFT(\"xyz\",1))").unwrap(),
            ExcelValue::Number(120.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=UNICODE(\"A\")+1").unwrap(),
            ExcelValue::Number(66.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=IFERROR(UNICODE(\"\"),0)").unwrap(),
            ExcelValue::Number(0.0)
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
            eval_formula_in(&wb, "=UNICODE(A1)").unwrap(),
            ExcelValue::Number(72.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=UNICODE(A2)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=UNICODE(A3)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=UNICODE(B1)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=UNICODE(A4)").unwrap(),
            ExcelValue::Number(128512.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=UNICODE(Title)").unwrap(),
            ExcelValue::Number(72.0)
        );
    }

    #[test]
    fn formula_unicode_scalars() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=UNICODE(\"é\")").unwrap(),
            ExcelValue::Number(233.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=UNICODE(\"€\")").unwrap(),
            ExcelValue::Number(8364.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=UNICODE(\"α\")").unwrap(),
            ExcelValue::Number(945.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=UNICODE(\"中\")").unwrap(),
            ExcelValue::Number(20013.0)
        );
    }
}
