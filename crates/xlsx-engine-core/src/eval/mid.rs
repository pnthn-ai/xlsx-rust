//! Excel `MID` kernel.
//!
//! `MID(text, start_num, num_chars)` returns a 1-based Unicode-scalar
//! substring. The production path finds UTF-8 byte offsets (ASCII is O(1)
//! index arithmetic) and clones only the requested slice. The `Vec<char>`
//! baseline lives beside it so benches can report a before/after.
//!
//! Character indexing matches this crate's `LEN` / `LEFT` / `RIGHT` /
//! `REPLACE`: Unicode scalar values (`str::chars`), which is Excel
//! Compatibility Version 2 (a surrogate-pair emoji is **one** character).
//! Version 1 counted UTF-16 code units (`😀` = 2); that legacy mode is not
//! implemented. Combining marks and variation selectors stay separate
//! scalars in both versions.
//!
//! Callers reject `start_num < 1`, `num_chars < 0`, and non-finite numerics
//! as `#VALUE!` before calling this. A start past `LEN(text)` returns `""`.
//! `num_chars` past the end returns the remainder. This kernel does **not**
//! read fixture goldens.

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

/// Production `MID` kernel on already-coerced text.
///
/// `start_num` is 1-based and must be `>= 1`. `num_chars` must be `>= 0`.
pub fn mid(text: &str, start_num: u64, num_chars: u64) -> String {
    debug_assert!(start_num >= 1);
    if num_chars == 0 {
        return String::new();
    }
    // Byte length ≥ scalar length, so a start past `text.len()` is always
    // empty — skip the ASCII / UTF-8 walk.
    if start_num.saturating_sub(1) >= text.len() as u64 {
        return String::new();
    }
    if text.is_ascii() {
        return mid_ascii(text, start_num, num_chars);
    }
    mid_utf8(text, start_num, num_chars)
}

/// `Vec<char>` baseline used for the hill-climb bench.
///
/// Same Excel semantics as [`mid`]. Kept so
/// `cargo bench -p xlsx-engine-core --bench mid` can print before/after.
pub fn mid_naive(text: &str, start_num: u64, num_chars: u64) -> String {
    debug_assert!(start_num >= 1);
    let chars: Vec<char> = text.chars().collect();
    let start0 = match usize::try_from(start_num.saturating_sub(1)) {
        Ok(n) => n,
        Err(_) => return String::new(),
    };
    if start0 >= chars.len() {
        return String::new();
    }
    let take = match usize::try_from(num_chars) {
        Ok(n) => n,
        Err(_) => chars.len() - start0,
    };
    chars.iter().skip(start0).take(take).collect()
}

/// Production `MID` on a scalar Excel value (no `Text` clone of the suffix).
pub fn mid_value(v: &ExcelValue, start_num: u64, num_chars: u64) -> Result<String, ExcelError> {
    match v {
        ExcelValue::Error(e) => Err(*e),
        ExcelValue::Empty => Ok(String::new()),
        ExcelValue::Text(s) => Ok(mid(s, start_num, num_chars)),
        ExcelValue::Bool(true) => Ok(mid("TRUE", start_num, num_chars)),
        ExcelValue::Bool(false) => Ok(mid("FALSE", start_num, num_chars)),
        ExcelValue::Number(n) => Ok(mid(&coerce::format_plain(*n), start_num, num_chars)),
        ExcelValue::Array(_) => Err(ExcelError::Value),
    }
}

/// Value-level baseline: full `to_text` clone + [`mid_naive`].
pub fn mid_value_naive(
    v: &ExcelValue,
    start_num: u64,
    num_chars: u64,
) -> Result<String, ExcelError> {
    Ok(mid_naive(&coerce::to_text(v)?, start_num, num_chars))
}

/// Truncate `start_num` toward zero. `< 1` or non-finite is `#VALUE!`.
pub fn trunc_start_num(n: f64) -> Result<u64, ExcelError> {
    if !n.is_finite() {
        return Err(ExcelError::Value);
    }
    let t = n.trunc();
    if t < 1.0 {
        return Err(ExcelError::Value);
    }
    if t > u64::MAX as f64 {
        Ok(u64::MAX)
    } else {
        Ok(t as u64)
    }
}

/// Truncate `num_chars` toward zero. `< 0` or non-finite is `#VALUE!`.
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

fn mid_ascii(text: &str, start_num: u64, num_chars: u64) -> String {
    debug_assert!(text.is_ascii());
    let n = text.len() as u64;
    let start0 = start_num - 1;
    if start0 >= n {
        return String::new();
    }
    let lo = start0 as usize;
    let hi = start0.saturating_add(num_chars).min(n) as usize;
    text[lo..hi].to_owned()
}

fn mid_utf8(text: &str, start_num: u64, num_chars: u64) -> String {
    let (lo, hi) = utf8_span(text, start_num, num_chars);
    if lo == hi {
        return String::new();
    }
    text[lo..hi].to_owned()
}

/// Byte offsets `[lo, hi)` of the 1-based Unicode-scalar span.
fn utf8_span(s: &str, start_num: u64, num_chars: u64) -> (usize, usize) {
    let start0 = start_num - 1;
    let mut seen = 0u64;
    let mut lo = None;
    for (byte_i, _) in s.char_indices() {
        if lo.is_none() {
            if seen == start0 {
                lo = Some(byte_i);
                if num_chars == 0 {
                    return (byte_i, byte_i);
                }
            }
        } else if seen - start0 == num_chars {
            return (lo.unwrap(), byte_i);
        }
        seen += 1;
    }
    match lo {
        Some(i) => (i, s.len()),
        None => (s.len(), s.len()),
    }
}

/// Production MID (scalar args, UTF-8 slice kernel).
pub(crate) fn fn_mid(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() != 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let text = ev.eval_scalar(&args[0], ctx)?;
    // Surface a text error before coercing later args (LTR, same as REPLACE).
    if let ExcelValue::Error(e) = text {
        return Ok(ExcelValue::Error(e));
    }
    let start_num = match coerce::to_number(&ev.eval_scalar(&args[1], ctx)?) {
        Ok(n) => match trunc_start_num(n) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        },
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let num_chars = match coerce::to_number(&ev.eval_scalar(&args[2], ctx)?) {
        Ok(n) => match trunc_num_chars(n) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        },
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    match mid_value(&text, start_num, num_chars) {
        Ok(s) => Ok(ExcelValue::Text(s)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::eval_formula_in;
    use xlsx_types::{Cell, ExcelError, ExcelValue, Sheet, Workbook};

    fn both(text: &str, start: u64, n: u64) -> String {
        let fast = mid(text, start, n);
        let slow = mid_naive(text, start, n);
        assert_eq!(
            fast, slow,
            "naive/fast mismatch for {text:?} start={start} n={n}"
        );
        fast
    }

    fn both_value(v: &ExcelValue, start: u64, n: u64) -> Result<String, ExcelError> {
        let fast = mid_value(v, start, n);
        let slow = mid_value_naive(v, start, n);
        assert_eq!(fast, slow, "value naive/fast mismatch for {v:?}");
        fast
    }

    #[test]
    fn microsoft_examples() {
        assert_eq!(both("Fluid Flow", 1, 5), "Fluid");
        assert_eq!(both("Fluid Flow", 7, 20), "Flow");
        assert_eq!(both("Fluid Flow", 20, 5), "");
    }

    #[test]
    fn one_based_start() {
        assert_eq!(both("abc", 1, 1), "a");
        assert_eq!(both("abc", 2, 1), "b");
        assert_eq!(both("abc", 3, 1), "c");
        assert_eq!(both("abc", 2, 2), "bc");
    }

    #[test]
    fn num_chars_zero_is_empty() {
        assert_eq!(both("abc", 1, 0), "");
        assert_eq!(both("abc", 2, 0), "");
        assert_eq!(both("abc", 4, 0), "");
        assert_eq!(both("", 1, 0), "");
    }

    #[test]
    fn past_end_is_empty_or_remainder() {
        assert_eq!(both("abc", 4, 1), "");
        assert_eq!(both("abc", 100, 5), "");
        assert_eq!(both("abc", 2, 10), "bc");
        assert_eq!(both("abc", 1, 10), "abc");
    }

    #[test]
    fn empty_text() {
        assert_eq!(both("", 1, 1), "");
        assert_eq!(both("", 2, 1), "");
        assert_eq!(both_value(&ExcelValue::Empty, 1, 1).unwrap(), "");
        assert_eq!(
            both_value(&ExcelValue::Text(String::new()), 1, 1).unwrap(),
            ""
        );
    }

    #[test]
    fn unicode_scalars_not_utf16() {
        assert_eq!(both("café", 4, 1), "é");
        assert_eq!(both("日本語", 2, 1), "本");
        // U+1F600 is one scalar (Excel Compatibility Version 2).
        assert_eq!(both("a😀b", 2, 1), "😀");
        assert_eq!(both("a😀b", 3, 1), "b");
        assert_eq!(both("😀", 1, 1), "😀");
        // Combining acute is its own scalar.
        assert_eq!(both("e\u{0301}", 2, 1), "\u{0301}");
        assert_eq!(both("e\u{0301}", 1, 1), "e");
    }

    #[test]
    fn large_start_is_empty() {
        assert_eq!(both("ab", u64::MAX, 1), "");
        assert_eq!(both("ab", 1, u64::MAX), "ab");
    }

    #[test]
    fn value_coercion_matches_ampersand() {
        assert_eq!(
            both_value(&ExcelValue::Number(12345.0), 2, 3).unwrap(),
            "234"
        );
        assert_eq!(both_value(&ExcelValue::Bool(true), 1, 1).unwrap(), "T");
        assert_eq!(both_value(&ExcelValue::Bool(false), 1, 1).unwrap(), "F");
        assert_eq!(
            both_value(&ExcelValue::Error(ExcelError::Div0), 1, 1),
            Err(ExcelError::Div0)
        );
        assert_eq!(
            both_value(
                &ExcelValue::Array(vec![vec![ExcelValue::Text("A".into())]]),
                1,
                1
            ),
            Err(ExcelError::Value)
        );
    }

    #[test]
    fn trunc_helpers() {
        assert_eq!(trunc_start_num(2.9).unwrap(), 2);
        assert_eq!(trunc_start_num(1.0).unwrap(), 1);
        assert_eq!(trunc_start_num(0.9), Err(ExcelError::Value));
        assert_eq!(trunc_start_num(0.0), Err(ExcelError::Value));
        assert_eq!(trunc_start_num(-1.0), Err(ExcelError::Value));
        assert_eq!(trunc_start_num(f64::NAN), Err(ExcelError::Value));
        assert_eq!(trunc_start_num(f64::INFINITY), Err(ExcelError::Value));
        assert_eq!(trunc_num_chars(1.9).unwrap(), 1);
        assert_eq!(trunc_num_chars(0.9).unwrap(), 0);
        assert_eq!(trunc_num_chars(0.0).unwrap(), 0);
        assert_eq!(trunc_num_chars(-0.1), Err(ExcelError::Value));
        assert_eq!(trunc_num_chars(f64::NAN), Err(ExcelError::Value));
        assert!(trunc_start_num(1e20).unwrap() > 0);
        assert!(trunc_num_chars(1e20).unwrap() > 0);
    }

    #[test]
    fn formula_microsoft_and_remainder() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=MID(\"Fluid Flow\", 1, 5)").unwrap(),
            ExcelValue::Text("Fluid".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(\"Fluid Flow\", 7, 20)").unwrap(),
            ExcelValue::Text("Flow".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(\"Fluid Flow\", 20, 5)").unwrap(),
            ExcelValue::Text(String::new())
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(\"abc\", 2, 1)").unwrap(),
            ExcelValue::Text("b".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(\"abc\", 2, 10)").unwrap(),
            ExcelValue::Text("bc".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(\"abc\", 1, 0)").unwrap(),
            ExcelValue::Text(String::new())
        );
    }

    #[test]
    fn formula_coercion_and_arity() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=MID(12345, 2, 3)").unwrap(),
            ExcelValue::Text("234".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(TRUE, 1, 1)").unwrap(),
            ExcelValue::Text("T".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(FALSE, 1, 1)").unwrap(),
            ExcelValue::Text("F".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(\"abc\", TRUE, 1)").unwrap(),
            ExcelValue::Text("a".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(\"abc\", FALSE, 1)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(\"abc\", 1, TRUE)").unwrap(),
            ExcelValue::Text("a".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(\"abc\", 1, FALSE)").unwrap(),
            ExcelValue::Text(String::new())
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(\"abc\", 2.9, 1)").unwrap(),
            ExcelValue::Text("b".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(\"abc\", 0.9, 1)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(\"abc\", 1, 1.9)").unwrap(),
            ExcelValue::Text("a".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(\"abc\", \"2\", \"1\")").unwrap(),
            ExcelValue::Text("b".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(\"abc\", \"x\", 1)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID()").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(\"abc\")").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(\"abc\", 1)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(\"abc\", 1, 1, 1)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(1/0, 1, 1)").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(\"abc\", 1/0, 1)").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(\"abc\", 1, 1/0)").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(#DIV/0!, #N/A, 1)").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(\"ab\"&\"cd\", 2, 2)").unwrap(),
            ExcelValue::Text("bc".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(LEFT(\"abcdef\", 4), 2, 2)").unwrap(),
            ExcelValue::Text("bc".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(MID(\"abc\", 2, 10))").unwrap(),
            ExcelValue::Number(2.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(\"abc\", 1E20, 1)").unwrap(),
            ExcelValue::Text(String::new())
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(\"abc\", 1, 1E20)").unwrap(),
            ExcelValue::Text("abc".into())
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
            .insert("A4".into(), Cell::value(ExcelValue::Text("a😀b".into())));
        let wb = Workbook {
            sheets: vec![sheet],
            names: vec![xlsx_types::DefinedName {
                name: "Title".into(),
                refers_to: "Sheet1!A1".into(),
            }],
        };
        assert_eq!(
            eval_formula_in(&wb, "=MID(A1, 2, 3)").unwrap(),
            ExcelValue::Text("ell".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(A2, 1, 1)").unwrap(),
            ExcelValue::Text(String::new())
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(A3, 1, 1)").unwrap(),
            ExcelValue::Text(String::new())
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(A1, A2, 1)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(A1, 1, A2)").unwrap(),
            ExcelValue::Text(String::new())
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(A4, 2, 1)").unwrap(),
            ExcelValue::Text("😀".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(Title, 1, 1)").unwrap(),
            ExcelValue::Text("H".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(MID(A4, 2, 1))").unwrap(),
            ExcelValue::Number(1.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=UNICODE(MID(A4, 2, 1))").unwrap(),
            ExcelValue::Number(128512.0)
        );
    }

    #[test]
    fn formula_unicode_and_map() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=MID(\"café\", 4, 1)").unwrap(),
            ExcelValue::Text("é".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=MID(\"日本語\", 2, 1)").unwrap(),
            ExcelValue::Text("本".into())
        );
        assert_eq!(
            eval_formula_in(
                &wb,
                "=TEXTJOIN(\"\",TRUE,MAP({\"ab\";\"cd\";\"ef\"},LAMBDA(x,MID(x,2,1))))"
            )
            .unwrap(),
            ExcelValue::Text("bdf".into())
        );
    }
}
