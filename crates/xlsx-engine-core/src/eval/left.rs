//! Excel `LEFT(text, [num_chars])` — prefix of Unicode scalars (Compat v2).
//!
//! Semantics (desktop Excel / Microsoft docs):
//! - Default `num_chars` is **1** (`LEFT("Sweden")` → `"S"`). An omitted
//!   optional argument (`LEFT(t,)` / `Expr::Missing`) uses that default.
//!   A **blank cell** is arithmetic `0` → `""`, not the default.
//! - `num_chars` truncates **toward zero** (CHOOSE / INDEX / LEFT family).
//!   `2.9` → 2; `0.9` → 0 → `""`; `-0.9` → 0 → `""`; `-1` → `#VALUE!`.
//! - `num_chars` past `LEN(text)` returns the whole string (no pad).
//! - Character indexing matches this crate's `LEN` / `MID` / `RIGHT` /
//!   `REPLACE`: Unicode scalar values (`str::chars`). That is Excel
//!   Compatibility Version 2 — a surrogate-pair emoji is **one**
//!   character. Version 1 counted UTF-16 units (`😀` = 2) and is not
//!   implemented. Combining marks and variation selectors stay separate
//!   scalars (not grapheme clusters).
//! - Text coerce is `&`-style (`TRUE` → `"TRUE"`, numbers via General).
//!   `num_chars` uses arithmetic coerce (`TRUE` → 1, `FALSE` / blank → 0,
//!   numeric text → number). Errors propagate left-to-right. Wrong arity
//!   is `#VALUE!`.
//!
//! Production path never builds a `Vec<char>`: ASCII is a byte prefix;
//! UTF-8 walks `char_indices` only as far as `num_chars`. A count that
//! is already `>=` the UTF-8 byte length is the whole string (byte length
//! ≥ scalar length). The `Vec<char>` baseline lives beside that path so
//! benches can print before/after. This kernel does **not** read fixture
//! goldens.

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

/// Production `LEFT` kernel on already-coerced text.
///
/// `num_chars` is a non-negative scalar count (callers reject `#VALUE!`
/// before calling this). `0` is the empty string.
pub fn left(text: &str, num_chars: u64) -> String {
    let end = prefix_byte_end(text, num_chars);
    if end == text.len() {
        text.to_owned()
    } else {
        text[..end].to_owned()
    }
}

/// Like [`left`], but reuses the owned buffer (truncate / move).
pub fn left_owned(text: String, num_chars: u64) -> String {
    let end = prefix_byte_end(&text, num_chars);
    if end == text.len() {
        return text;
    }
    let mut out = text;
    out.truncate(end);
    out
}

/// `Vec<char>` baseline used for the hill-climb bench.
///
/// Same Excel semantics as [`left`]. Kept so
/// `cargo bench -p xlsx-engine-core --bench left` can print before/after.
pub fn left_naive(text: &str, num_chars: u64) -> String {
    let chars: Vec<char> = text.chars().collect();
    let take = match usize::try_from(num_chars) {
        Ok(n) => n.min(chars.len()),
        Err(_) => chars.len(),
    };
    chars.iter().take(take).collect()
}

/// Truncate toward zero; reject negatives and non-finite as `#VALUE!`.
///
/// Counts larger than `u64::MAX` saturate (the prefix is then the whole
/// string). Matches `REPLACE`'s `num_chars` rule.
pub fn trunc_num_chars(n: f64) -> Result<u64, ExcelError> {
    if !n.is_finite() {
        return Err(ExcelError::Value);
    }
    let t = n.trunc();
    if t < 0.0 {
        return Err(ExcelError::Value);
    }
    if t > u64::MAX as f64 {
        Ok(u64::MAX)
    } else {
        Ok(t as u64)
    }
}

/// UTF-8 byte end of the first `num_chars` Unicode scalars.
fn prefix_byte_end(text: &str, num_chars: u64) -> usize {
    if num_chars == 0 {
        return 0;
    }
    // Byte length ≥ scalar length, so a count past `text.len()` is always
    // the whole string — skip the ASCII / UTF-8 walk.
    if num_chars >= text.len() as u64 {
        return text.len();
    }
    if text.is_ascii() {
        return num_chars as usize;
    }
    let mut seen = 0u64;
    for (byte_i, _) in text.char_indices() {
        if seen == num_chars {
            return byte_i;
        }
        seen += 1;
    }
    text.len()
}

/// Production LEFT (scalar args, implicit intersection, omitted default).
pub(crate) fn fn_left(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.is_empty() || args.len() > 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let s = match coerce::to_text(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(s) => s,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let n = if args.len() >= 2 {
        match &args[1] {
            Expr::Missing => 1,
            other => match coerce::to_number(&ev.eval_scalar(other, ctx)?) {
                Ok(n) => match trunc_num_chars(n) {
                    Ok(n) => n,
                    Err(e) => return Ok(ExcelValue::Error(e)),
                },
                Err(e) => return Ok(ExcelValue::Error(e)),
            },
        }
    } else {
        1
    };
    Ok(ExcelValue::Text(left_owned(s, n)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::eval_formula_in;
    use xlsx_types::Workbook;

    fn both(text: &str, n: u64) -> String {
        let fast = left(text, n);
        let owned = left_owned(text.to_owned(), n);
        let slow = left_naive(text, n);
        assert_eq!(fast, slow, "left/naive mismatch for {text:?} n={n}");
        assert_eq!(owned, slow, "left_owned/naive mismatch for {text:?} n={n}");
        fast
    }

    #[test]
    fn microsoft_examples() {
        assert_eq!(both("Sale Price", 4), "Sale");
        assert_eq!(both("Sweden", 1), "S");
        assert_eq!(both("abc", 2), "ab");
    }

    #[test]
    fn zero_and_oversize() {
        assert_eq!(both("abc", 0), "");
        assert_eq!(both("abc", 3), "abc");
        assert_eq!(both("abc", 10), "abc");
        assert_eq!(both("", 1), "");
        assert_eq!(both("", 0), "");
        assert_eq!(both("abc", u64::MAX), "abc");
    }

    #[test]
    fn unicode_scalars_not_utf16() {
        assert_eq!(both("café", 3), "caf");
        assert_eq!(both("café", 4), "café");
        assert_eq!(both("日本語", 2), "日本");
        // U+1F600 is one scalar (Excel Compatibility Version 2).
        assert_eq!(both("a😀b", 1), "a");
        assert_eq!(both("a😀b", 2), "a😀");
        assert_eq!(both("a😀b", 3), "a😀b");
        assert_eq!(both("😀abc", 1), "😀");
        // Combining acute is its own scalar.
        assert_eq!(both("e\u{0301}", 1), "e");
        assert_eq!(both("e\u{0301}", 2), "e\u{0301}");
        // Variation selector stays a separate scalar.
        assert_eq!(both("❤\u{FE0F}", 1), "❤");
    }

    #[test]
    fn trunc_domain() {
        assert_eq!(trunc_num_chars(2.9).unwrap(), 2);
        assert_eq!(trunc_num_chars(0.9).unwrap(), 0);
        assert_eq!(trunc_num_chars(-0.9).unwrap(), 0);
        assert_eq!(trunc_num_chars(-1.0), Err(ExcelError::Value));
        assert_eq!(trunc_num_chars(-1.9), Err(ExcelError::Value));
        assert_eq!(trunc_num_chars(f64::NAN), Err(ExcelError::Value));
        assert_eq!(trunc_num_chars(f64::INFINITY), Err(ExcelError::Value));
        assert_eq!(trunc_num_chars(1e20).unwrap(), u64::MAX);
    }

    #[test]
    fn long_ascii_prefix_matches_naive() {
        let s = "x".repeat(200_000);
        assert_eq!(both(&s, 1).len(), 1);
        assert_eq!(both(&s, 16), "x".repeat(16));
        assert_eq!(both(&s, 200_000).len(), 200_000);
        assert_eq!(both(&s, 200_001).len(), 200_000);
    }

    #[test]
    fn formula_microsoft_and_default() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=LEFT(\"Sale Price\", 4)").unwrap(),
            ExcelValue::Text("Sale".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEFT(\"Sweden\")").unwrap(),
            ExcelValue::Text("S".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEFT(\"abc\",)").unwrap(),
            ExcelValue::Text("a".into())
        );
    }

    #[test]
    fn formula_trunc_coerce_arity() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=LEFT(\"abc\", 2.9)").unwrap(),
            ExcelValue::Text("ab".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEFT(\"abc\", 0.9)").unwrap(),
            ExcelValue::Text("".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEFT(\"abc\", -0.9)").unwrap(),
            ExcelValue::Text("".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEFT(\"abc\", -1)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEFT(\"abc\", TRUE)").unwrap(),
            ExcelValue::Text("a".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEFT(\"abc\", FALSE)").unwrap(),
            ExcelValue::Text("".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEFT(TRUE, 2)").unwrap(),
            ExcelValue::Text("TR".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEFT(12345, 2)").unwrap(),
            ExcelValue::Text("12".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEFT()").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEFT(\"abc\", 1, 2)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEFT(1/0, 1)").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEFT(\"abc\", 1/0)").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEFT(#N/A, #DIV/0!)").unwrap(),
            ExcelValue::Error(ExcelError::Na)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(LEFT(\"😀abc\", 1))").unwrap(),
            ExcelValue::Number(1.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEFT(LEFT(\"hello\", 4), 2)").unwrap(),
            ExcelValue::Text("he".into())
        );
    }
}
