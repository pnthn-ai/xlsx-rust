//! Excel `RIGHT(text, [num_chars])`.
//!
//! Semantics (desktop Excel / Microsoft docs, Compatibility Version 2):
//! - Returns the last `num_chars` characters of `text`. `num_chars` omitted
//!   defaults to **1** (`RIGHT("Stock Number")` → `"r"`).
//! - Character indexing matches `LEN` / `MID` / `LEFT` / `REPLACE`: Unicode
//!   scalar values (`str::chars`). A supplementary-plane emoji is **one**
//!   character. Version 1 counted UTF-16 code units (`😀` = 2); that legacy
//!   mode is not implemented. Combining marks and variation selectors stay
//!   separate scalars.
//! - `num_chars` is truncated toward zero (`4.9` → 4). Sign is checked
//!   **after** truncate (`−0.9` → 0 → `""`; `−1` is `#VALUE!`).
//! - `num_chars` greater than `LEN(text)` (including `1E+20`) returns all of
//!   `text`. Zero returns `""`.
//! - `text` coerces like `&` (`TRUE` → `"TRUE"`, `12` → `"12"`, blank → `""`).
//! - `num_chars` uses arithmetic coerce (`TRUE` → 1, `FALSE` / blank → 0,
//!   `"2"` → 2). Non-finite is `#VALUE!`.
//! - Wrong arity is `#VALUE!`. Errors evaluate left-to-right (`text` first).
//!
//! Production path: byte-length early-out, ASCII suffix slice, UTF-8 walk
//! from the end. The `Vec<char>` baseline lives beside that path so benches
//! can report a before/after. This kernel does **not** read fixture goldens.

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

/// Production `RIGHT` kernel: last `n` Unicode scalars.
///
/// `n` is already truncated toward zero and rejected when negative /
/// non-finite. `n == 0` is `""`. `n` past `LEN(text)` returns all of `text`.
pub fn right(text: &str, n: u64) -> String {
    right_fast(text, n)
}

/// Consume an already-owned string (the `to_text` result) and take the suffix.
///
/// When `n` covers the whole string (byte length is an upper bound on scalar
/// length) the buffer is returned unchanged — no second allocation.
pub fn right_owned(text: String, n: u64) -> String {
    if n == 0 {
        return String::new();
    }
    if n >= text.len() as u64 {
        return text;
    }
    right_fast(&text, n)
}

/// `Vec<char>` baseline used for the hill-climb bench.
///
/// Same Excel semantics as [`right`]. Kept so
/// `cargo bench -p xlsx-engine-core --bench right` can print before/after.
pub fn right_naive(text: &str, n: u64) -> String {
    if n == 0 {
        return String::new();
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return String::new();
    }
    let take = match usize::try_from(n) {
        Ok(k) => k.min(chars.len()),
        Err(_) => chars.len(),
    };
    chars[chars.len() - take..].iter().collect()
}

/// Truncate toward zero. Negative after truncate is `#VALUE!`.
///
/// Non-finite values are `#VALUE!`. Magnitudes that do not fit in `u64`
/// saturate at `u64::MAX` (RIGHT then returns the whole string).
pub fn trunc_num_chars(n: f64) -> Result<u64, ExcelError> {
    if !n.is_finite() {
        return Err(ExcelError::Value);
    }
    let t = n.trunc();
    if t < 0.0 {
        return Err(ExcelError::Value);
    }
    if t >= u64::MAX as f64 {
        return Ok(u64::MAX);
    }
    Ok(t as u64)
}

fn right_fast(text: &str, n: u64) -> String {
    if n == 0 || text.is_empty() {
        return String::new();
    }
    // Byte length ≥ scalar length, so `n` at or past the byte length always
    // covers the whole string — skip the ASCII / UTF-8 walk.
    if n >= text.len() as u64 {
        return text.to_owned();
    }
    if text.is_ascii() {
        return right_ascii(text, n as usize);
    }
    right_utf8(text, n)
}

fn right_ascii(text: &str, n: usize) -> String {
    debug_assert!(text.is_ascii());
    debug_assert!(n < text.len());
    text[text.len() - n..].to_owned()
}

/// Last `n` Unicode scalars of a non-ASCII UTF-8 string, walking from the end.
///
/// `n` is already known to be smaller than the byte length. If the string
/// has fewer than `n` scalars, the whole string is returned.
fn right_utf8(text: &str, n: u64) -> String {
    debug_assert!(n > 0);
    debug_assert!(n < text.len() as u64);
    let bytes = text.as_bytes();
    let mut i = bytes.len();
    let mut left = n;
    while i > 0 && left > 0 {
        i -= 1;
        // Start byte: ASCII (`0xxxxxxx`) or leading (`11xxxxxx`).
        // Continuation bytes are `10xxxxxx`.
        if bytes[i] < 0x80 || bytes[i] >= 0xC0 {
            left -= 1;
        }
    }
    if left > 0 {
        return text.to_owned();
    }
    // `i` is the start of the first kept scalar.
    text[i..].to_owned()
}

/// Production RIGHT (scalar args, `&` / arithmetic coerce, implicit intersection).
pub(crate) fn fn_right(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.is_empty() || args.len() > 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let text = match coerce::to_text(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(s) => s,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let n = if args.len() == 2 {
        match coerce::to_number(&ev.eval_scalar(&args[1], ctx)?) {
            Ok(n) => match trunc_num_chars(n) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            },
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        1
    };
    Ok(ExcelValue::Text(right_owned(text, n)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::eval_formula_in;
    use xlsx_types::{Cell, ExcelError, ExcelValue, Sheet, Workbook};

    fn both(text: &str, n: u64) -> String {
        let fast = right(text, n);
        let slow = right_naive(text, n);
        assert_eq!(fast, slow, "naive/fast mismatch for {text:?} n={n}");
        let owned = right_owned(text.to_owned(), n);
        assert_eq!(fast, owned, "owned/fast mismatch for {text:?} n={n}");
        fast
    }

    #[test]
    fn microsoft_examples() {
        assert_eq!(both("Sale Price", 5), "Price");
        assert_eq!(both("Stock Number", 1), "r");
    }

    #[test]
    fn ascii_suffix_and_identity() {
        assert_eq!(both("", 0), "");
        assert_eq!(both("", 1), "");
        assert_eq!(both("", 10), "");
        assert_eq!(both("abc", 0), "");
        assert_eq!(both("abc", 1), "c");
        assert_eq!(both("abc", 2), "bc");
        assert_eq!(both("abc", 3), "abc");
        assert_eq!(both("abc", 4), "abc");
        assert_eq!(both("abc", 100), "abc");
        assert_eq!(both("a", 1), "a");
        assert_eq!(both("Hello World", 5), "World");
    }

    #[test]
    fn unicode_scalars_not_utf16() {
        assert_eq!(both("café", 1), "é");
        assert_eq!(both("café", 2), "fé");
        assert_eq!(both("café", 4), "café");
        assert_eq!(both("日本語", 1), "語");
        assert_eq!(both("日本語", 2), "本語");
        // U+1F600 is one scalar (Excel Compatibility Version 2).
        assert_eq!(both("a😀b", 1), "b");
        assert_eq!(both("a😀b", 2), "😀b");
        assert_eq!(both("a😀b", 3), "a😀b");
        assert_eq!(both("😀", 1), "😀");
        assert_eq!(both("😀", 2), "😀");
        // Combining acute is its own scalar.
        assert_eq!(both("e\u{0301}", 1), "\u{0301}");
        assert_eq!(both("e\u{0301}", 2), "e\u{0301}");
    }

    #[test]
    fn large_n_and_owned_identity() {
        let long = "x".repeat(4096);
        assert_eq!(both(&long, 8), "x".repeat(8));
        assert_eq!(both(&long, 4096), long);
        assert_eq!(both(&long, 4097), long);
        assert_eq!(both(&long, u64::MAX), long);
        let cafe = "café".repeat(256);
        assert_eq!(both(&cafe, 2), "fé");
        assert_eq!(both(&cafe, 4), "café");
        let emoji = "😀".repeat(64);
        assert_eq!(both(&emoji, 1), "😀");
        assert_eq!(both(&emoji, 2), "😀😀");
    }

    #[test]
    fn trunc_num_chars_sign_and_domain() {
        assert_eq!(trunc_num_chars(0.0).unwrap(), 0);
        assert_eq!(trunc_num_chars(-0.0).unwrap(), 0);
        assert_eq!(trunc_num_chars(0.9).unwrap(), 0);
        assert_eq!(trunc_num_chars(-0.9).unwrap(), 0);
        assert_eq!(trunc_num_chars(1.9).unwrap(), 1);
        assert_eq!(trunc_num_chars(4.9).unwrap(), 4);
        assert_eq!(trunc_num_chars(-1.0), Err(ExcelError::Value));
        assert_eq!(trunc_num_chars(-1.9), Err(ExcelError::Value));
        assert_eq!(trunc_num_chars(f64::NAN), Err(ExcelError::Value));
        assert_eq!(trunc_num_chars(f64::INFINITY), Err(ExcelError::Value));
        assert_eq!(trunc_num_chars(f64::NEG_INFINITY), Err(ExcelError::Value));
        assert_eq!(trunc_num_chars(1e20).unwrap(), u64::MAX);
        assert_eq!(trunc_num_chars(u64::MAX as f64).unwrap(), u64::MAX);
    }

    #[test]
    fn utf8_walk_edges() {
        // 2-byte, 3-byte, 4-byte scalars at the end.
        assert_eq!(both("xé", 1), "é");
        assert_eq!(both("x€", 1), "€");
        assert_eq!(both("x😀", 1), "😀");
        assert_eq!(both("ééé", 2), "éé");
        assert_eq!(both("€€€", 1), "€");
        // n between scalar count and byte length.
        assert_eq!(both("😀", 2), "😀"); // 1 scalar, 4 bytes
        assert_eq!(both("é", 2), "é"); // 1 scalar, 2 bytes
    }

    #[test]
    fn formula_coercion_and_arity() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=RIGHT(\"Sale Price\", 5)").unwrap(),
            ExcelValue::Text("Price".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=RIGHT(\"Stock Number\")").unwrap(),
            ExcelValue::Text("r".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=RIGHT(\"abc\", 2.9)").unwrap(),
            ExcelValue::Text("bc".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=RIGHT(\"abc\", 0)").unwrap(),
            ExcelValue::Text(String::new())
        );
        assert_eq!(
            eval_formula_in(&wb, "=RIGHT(\"abc\", -0.9)").unwrap(),
            ExcelValue::Text(String::new())
        );
        assert_eq!(
            eval_formula_in(&wb, "=RIGHT(\"abc\", -1)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=RIGHT(12345, 2)").unwrap(),
            ExcelValue::Text("45".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=RIGHT(TRUE, 2)").unwrap(),
            ExcelValue::Text("UE".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=RIGHT(FALSE, 3)").unwrap(),
            ExcelValue::Text("LSE".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=RIGHT(\"abc\", TRUE)").unwrap(),
            ExcelValue::Text("c".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=RIGHT(\"abc\", FALSE)").unwrap(),
            ExcelValue::Text(String::new())
        );
        assert_eq!(
            eval_formula_in(&wb, "=RIGHT(\"abc\", \"2\")").unwrap(),
            ExcelValue::Text("bc".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=RIGHT()").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=RIGHT(\"a\", 1, 2)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=RIGHT(1/0, 1)").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=RIGHT(\"abc\", 1/0)").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=RIGHT(NA())").unwrap(),
            ExcelValue::Error(ExcelError::Na)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(RIGHT(\"a😀b\", 2))").unwrap(),
            ExcelValue::Number(2.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=RIGHT(\"abc\", 1E+20)").unwrap(),
            ExcelValue::Text("abc".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=RIGHT(\"abc\",)").unwrap(),
            ExcelValue::Text(String::new())
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
            .insert("A2".into(), Cell::value(ExcelValue::Empty));
        sheet
            .cells
            .insert("A3".into(), Cell::value(ExcelValue::Number(3.0)));
        let wb = Workbook {
            sheets: vec![sheet],
            names: vec![],
        };
        assert_eq!(
            eval_formula_in(&wb, "=RIGHT(A1, 2)").unwrap(),
            ExcelValue::Text("lo".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=RIGHT(A2, 1)").unwrap(),
            ExcelValue::Text(String::new())
        );
        assert_eq!(
            eval_formula_in(&wb, "=RIGHT(A1, A2)").unwrap(),
            ExcelValue::Text(String::new())
        );
        assert_eq!(
            eval_formula_in(&wb, "=RIGHT(A1, A3)").unwrap(),
            ExcelValue::Text("llo".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=RIGHT(B1, 1)").unwrap(),
            ExcelValue::Text(String::new())
        );
    }
}
