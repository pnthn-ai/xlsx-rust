//! Excel `REPLACE` kernel.
//!
//! Semantics (desktop Excel / Microsoft docs; no golden-reading):
//! - `REPLACE(old_text, start_num, num_chars, new_text)` overwrites a
//!   1-based character span.
//! - `start_num` is 1-based. After truncate-toward-zero, `< 1` (blank /
//!   `FALSE` → 0, `0.9`) is `#VALUE!`.
//! - `num_chars < 0` after truncate is `#VALUE!`. `0` (and `0.9` / `-0.9`
//!   / `FALSE` / blank) inserts without deleting.
//! - `start_num` past `LEN(old_text)` **appends** `new_text` (no error).
//! - Empty `new_text` deletes the span. Empty `old_text` still yields
//!   `new_text` when `start_num >= 1`.
//! - Character indexing matches this crate's `LEN` / `MID` / `LEFT` /
//!   `RIGHT`: Unicode scalar values (`str::chars`). That is Excel
//!   Compatibility Version 2 — a supplementary-plane emoji is **one**
//!   character. Version 1 counted UTF-16 code units (`😀` = 2); that
//!   legacy mode is not implemented. Combining marks and variation
//!   selectors stay separate scalars. `REPLACEB` (byte / DBCS) is out
//!   of scope.
//! - Numbers / bools coerce like `&`. Errors evaluate left-to-right.
//!   Wrong arity is `#VALUE!`. The result is always text.
//!
//! Production path finds UTF-8 byte offsets (ASCII is O(1) index
//! arithmetic; `start_num == 1` skips the prefix walk) and builds the
//! result in one allocation. Equal-width overwrites patch in place.
//! [`replace_value`] borrows `Text` instead of cloning via `to_text`.
//! The `Vec<char>` baseline lives beside that path so benches can print
//! before/after.

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use std::borrow::Cow;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

/// Production `REPLACE` kernel.
///
/// `start_num` is 1-based and must be `>= 1`. `num_chars` must be `>= 0`.
/// Callers reject out-of-range / non-finite numeric arguments as `#VALUE!`
/// before calling this. A start past `LEN(old_text)` appends `new_text`.
pub fn replace(old_text: &str, start_num: u64, num_chars: u64, new_text: &str) -> String {
    debug_assert!(start_num >= 1);
    if num_chars == 0 && new_text.is_empty() {
        return old_text.to_owned();
    }
    let start0 = start_num - 1;
    // Byte length ≥ scalar length, so a start past `old_text.len()` is
    // always an append — skip the ASCII / UTF-8 walk.
    if start0 >= old_text.len() as u64 {
        return append(old_text, new_text);
    }
    // `num_chars` covering every byte covers every scalar: whole replace.
    if start0 == 0 && num_chars >= old_text.len() as u64 {
        return new_text.to_owned();
    }
    if old_text.is_ascii() {
        replace_ascii(old_text, start_num, num_chars, new_text)
    } else {
        replace_utf8(old_text, start_num, num_chars, new_text)
    }
}

/// Quadratic-ish baseline: materialize every Unicode scalar, then rebuild.
///
/// Same Excel semantics as [`replace`]. Kept so
/// `cargo bench -p xlsx-engine-core --bench replace` can print before/after.
pub fn replace_naive(old_text: &str, start_num: u64, num_chars: u64, new_text: &str) -> String {
    debug_assert!(start_num >= 1);
    let chars: Vec<char> = old_text.chars().collect();
    let start0 = match usize::try_from(start_num.saturating_sub(1)) {
        Ok(n) => n,
        Err(_) => return append(old_text, new_text),
    };
    if start0 >= chars.len() {
        return append(old_text, new_text);
    }
    let take = match usize::try_from(num_chars) {
        Ok(n) => n,
        Err(_) => chars.len() - start0,
    };
    let end = start0.saturating_add(take).min(chars.len());
    let mut out = String::new();
    out.extend(chars[..start0].iter());
    out.push_str(new_text);
    out.extend(chars[end..].iter());
    out
}

/// Production path on already-evaluated scalars (borrows `Text`).
///
/// Errors combine left-to-right: `old_text`, then `start_num`, then
/// `num_chars`, then `new_text`.
pub fn replace_value(
    old: &ExcelValue,
    start: &ExcelValue,
    num: &ExcelValue,
    new: &ExcelValue,
) -> Result<String, ExcelError> {
    if let ExcelValue::Error(e) = old {
        return Err(*e);
    }
    let start_num = trunc_start_num(coerce::to_number(start)?)?;
    let num_chars = trunc_num_chars(coerce::to_number(num)?)?;
    let old_s = text_ref(old)?;
    let new_s = text_ref(new)?;
    Ok(replace(
        old_s.as_ref(),
        start_num,
        num_chars,
        new_s.as_ref(),
    ))
}

/// Value-level baseline: full `to_text` clones + [`replace_naive`].
pub fn replace_value_naive(
    old: &ExcelValue,
    start: &ExcelValue,
    num: &ExcelValue,
    new: &ExcelValue,
) -> Result<String, ExcelError> {
    let old_s = coerce::to_text(old)?;
    let start_num = trunc_start_num(coerce::to_number(start)?)?;
    let num_chars = trunc_num_chars(coerce::to_number(num)?)?;
    let new_s = coerce::to_text(new)?;
    Ok(replace_naive(&old_s, start_num, num_chars, &new_s))
}

/// Truncate `start_num` toward zero. `< 1` / non-finite → `#VALUE!`.
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

/// Truncate `num_chars` toward zero. `< 0` / non-finite → `#VALUE!`.
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

/// Production REPLACE (four scalar args, borrow-`Text` kernel).
pub(crate) fn fn_replace(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() != 4 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let old = ev.eval_scalar(&args[0], ctx)?;
    let start = ev.eval_scalar(&args[1], ctx)?;
    let num = ev.eval_scalar(&args[2], ctx)?;
    let new = ev.eval_scalar(&args[3], ctx)?;
    match replace_value(&old, &start, &num, &new) {
        Ok(s) => Ok(ExcelValue::Text(s)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn text_ref(v: &ExcelValue) -> Result<Cow<'_, str>, ExcelError> {
    match v {
        ExcelValue::Error(e) => Err(*e),
        ExcelValue::Text(s) => Ok(Cow::Borrowed(s)),
        ExcelValue::Empty => Ok(Cow::Borrowed("")),
        ExcelValue::Bool(true) => Ok(Cow::Borrowed("TRUE")),
        ExcelValue::Bool(false) => Ok(Cow::Borrowed("FALSE")),
        ExcelValue::Number(n) => Ok(Cow::Owned(coerce::format_plain(*n))),
        ExcelValue::Array(_) => Err(ExcelError::Value),
    }
}

fn append(old: &str, new_text: &str) -> String {
    let mut out = String::with_capacity(old.len() + new_text.len());
    out.push_str(old);
    out.push_str(new_text);
    out
}

fn replace_ascii(old: &str, start_num: u64, num_chars: u64, new_text: &str) -> String {
    debug_assert!(old.is_ascii());
    let n = old.len() as u64;
    let start0 = start_num - 1;
    if start0 >= n {
        return append(old, new_text);
    }
    let lo = start0 as usize;
    let hi = start0.saturating_add(num_chars).min(n) as usize;
    stitch(old, lo, hi, new_text)
}

fn replace_utf8(old: &str, start_num: u64, num_chars: u64, new_text: &str) -> String {
    let (lo, hi) = utf8_span(old, start_num, num_chars);
    stitch(old, lo, hi, new_text)
}

/// Prefix + `new_text` + suffix. Equal-width UTF-8/ASCII patches in place.
fn stitch(old: &str, lo: usize, hi: usize, new_text: &str) -> String {
    if lo == hi && new_text.is_empty() {
        return old.to_owned();
    }
    if lo == 0 && hi == old.len() {
        return new_text.to_owned();
    }
    // Equal-width overwrite: clone once and patch. Valid UTF-8 of equal
    // byte length cannot produce an invalid sequence at the join.
    if hi - lo == new_text.len() {
        let mut buf = old.to_owned();
        // SAFETY: `old[lo..hi]` and `new_text` are valid UTF-8 of equal
        // byte length, so overwriting the span stays valid UTF-8.
        unsafe {
            buf.as_bytes_mut()[lo..hi].copy_from_slice(new_text.as_bytes());
        }
        return buf;
    }
    let mut out = String::with_capacity(lo + new_text.len() + (old.len() - hi));
    out.push_str(&old[..lo]);
    out.push_str(new_text);
    out.push_str(&old[hi..]);
    out
}

/// Byte offsets `[lo, hi)` of the 1-based Unicode-scalar span.
fn utf8_span(s: &str, start_num: u64, num_chars: u64) -> (usize, usize) {
    let start0 = start_num - 1;
    if start0 == 0 {
        if num_chars == 0 {
            return (0, 0);
        }
        let mut seen = 0u64;
        for (byte_i, _) in s.char_indices() {
            if seen == num_chars {
                return (0, byte_i);
            }
            seen += 1;
        }
        return (0, s.len());
    }
    let mut seen = 0u64;
    let mut prefix_end = s.len();
    let mut found = false;
    for (byte_i, _) in s.char_indices() {
        if !found {
            if seen == start0 {
                prefix_end = byte_i;
                found = true;
                if num_chars == 0 {
                    return (byte_i, byte_i);
                }
            }
        } else if seen - start0 == num_chars {
            return (prefix_end, byte_i);
        }
        seen += 1;
    }
    if found {
        (prefix_end, s.len())
    } else {
        (s.len(), s.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::eval_formula_in;
    use xlsx_types::{Cell, DefinedName, ExcelError, ExcelValue, Sheet, Workbook};

    fn both(old: &str, start: u64, n: u64, new: &str) -> String {
        let fast = replace(old, start, n, new);
        let slow = replace_naive(old, start, n, new);
        assert_eq!(
            fast, slow,
            "naive/fast mismatch for {old:?} start={start} n={n} new={new:?}"
        );
        fast
    }

    fn both_value(
        old: &ExcelValue,
        start: &ExcelValue,
        num: &ExcelValue,
        new: &ExcelValue,
    ) -> Result<String, ExcelError> {
        let fast = replace_value(old, start, num, new);
        let slow = replace_value_naive(old, start, num, new);
        assert_eq!(fast, slow, "value naive/fast mismatch");
        fast
    }

    #[test]
    fn microsoft_examples() {
        assert_eq!(both("abcdefghijk", 6, 5, "*"), "abcde*k");
        assert_eq!(both("2009", 3, 2, "10"), "2010");
        assert_eq!(both("123456", 1, 3, "@"), "@456");
    }

    #[test]
    fn one_based_start() {
        assert_eq!(both("abc", 1, 1, "X"), "Xbc");
        assert_eq!(both("abc", 2, 1, "X"), "aXc");
        assert_eq!(both("abc", 3, 1, "X"), "abX");
    }

    #[test]
    fn num_chars_zero_inserts() {
        assert_eq!(both("abc", 1, 0, "X"), "Xabc");
        assert_eq!(both("abc", 2, 0, "X"), "aXbc");
        assert_eq!(both("abc", 4, 0, "X"), "abcX");
    }

    #[test]
    fn empty_new_text_deletes() {
        assert_eq!(both("abc", 2, 1, ""), "ac");
        assert_eq!(both("abc", 1, 3, ""), "");
        assert_eq!(both("abc", 2, 0, ""), "abc");
    }

    #[test]
    fn out_of_range_appends() {
        assert_eq!(both("abc", 4, 1, "X"), "abcX");
        assert_eq!(both("abc", 100, 5, "X"), "abcX");
        assert_eq!(both("abc", 2, 10, "X"), "aX");
    }

    #[test]
    fn empty_old_text() {
        assert_eq!(both("", 1, 0, "X"), "X");
        assert_eq!(both("", 1, 1, "X"), "X");
        assert_eq!(both("", 2, 1, "X"), "X");
    }

    #[test]
    fn whole_string_and_grow_shrink() {
        assert_eq!(both("abc", 1, 3, "XYZ"), "XYZ");
        assert_eq!(both("abc", 1, 100, "Z"), "Z");
        assert_eq!(both("abc", 2, 1, "XYZ"), "aXYZc");
        assert_eq!(both("a b", 2, 1, "-"), "a-b");
        assert_eq!(both("AbC", 2, 1, "x"), "AxC");
    }

    #[test]
    fn unicode_scalars_not_utf16() {
        assert_eq!(both("café", 4, 1, "e"), "cafe");
        assert_eq!(both("日本語", 2, 1, "X"), "日X語");
        // U+1F600 is one scalar (Excel Compatibility Version 2).
        assert_eq!(both("a😀b", 2, 1, "X"), "aXb");
        assert_eq!(both("a😀b", 3, 1, "X"), "a😀X");
        assert_eq!(both("😀😀", 1, 1, "X"), "X😀");
        // Combining acute is its own scalar.
        assert_eq!(both("e\u{0301}", 2, 1, ""), "e");
        assert_eq!(both("e\u{0301}", 1, 1, "o"), "o\u{0301}");
        // Variation selector is its own scalar (not clustered).
        assert_eq!(both("a\u{FE0F}b", 2, 1, "X"), "aXb");
        // Equal-width 4-byte overwrite (emoji → emoji).
        assert_eq!(both("a😀b", 2, 1, "🎉"), "a🎉b");
    }

    #[test]
    fn large_start_appends() {
        assert_eq!(both("ab", u64::MAX, 1, "Z"), "abZ");
    }

    #[test]
    fn trunc_rejects_non_finite_and_below_one() {
        assert_eq!(trunc_start_num(0.9), Err(ExcelError::Value));
        assert_eq!(trunc_start_num(0.0), Err(ExcelError::Value));
        assert_eq!(trunc_start_num(-1.0), Err(ExcelError::Value));
        assert_eq!(trunc_start_num(f64::INFINITY), Err(ExcelError::Value));
        assert_eq!(trunc_start_num(f64::NAN), Err(ExcelError::Value));
        assert_eq!(trunc_start_num(2.9).unwrap(), 2);
        assert_eq!(trunc_num_chars(-0.9).unwrap(), 0);
        assert_eq!(trunc_num_chars(0.9).unwrap(), 0);
        assert_eq!(trunc_num_chars(-1.0), Err(ExcelError::Value));
        assert_eq!(trunc_num_chars(f64::NEG_INFINITY), Err(ExcelError::Value));
    }

    #[test]
    fn value_coercion_matches_ampersand() {
        assert_eq!(
            both_value(
                &ExcelValue::Number(2009.0),
                &ExcelValue::Number(3.0),
                &ExcelValue::Number(2.0),
                &ExcelValue::Number(10.0),
            )
            .unwrap(),
            "2010"
        );
        assert_eq!(
            both_value(
                &ExcelValue::Bool(true),
                &ExcelValue::Number(1.0),
                &ExcelValue::Number(1.0),
                &ExcelValue::Text("X".into()),
            )
            .unwrap(),
            "XRUE"
        );
        assert_eq!(
            both_value(
                &ExcelValue::Text("abc".into()),
                &ExcelValue::Empty,
                &ExcelValue::Number(1.0),
                &ExcelValue::Text("X".into()),
            ),
            Err(ExcelError::Value)
        );
        assert_eq!(
            both_value(
                &ExcelValue::Text("abc".into()),
                &ExcelValue::Bool(false),
                &ExcelValue::Number(1.0),
                &ExcelValue::Text("X".into()),
            ),
            Err(ExcelError::Value)
        );
        assert_eq!(
            both_value(
                &ExcelValue::Text("abc".into()),
                &ExcelValue::Number(2.0),
                &ExcelValue::Empty,
                &ExcelValue::Text("X".into()),
            )
            .unwrap(),
            "aXbc"
        );
        assert_eq!(
            both_value(
                &ExcelValue::Error(ExcelError::Div0),
                &ExcelValue::Error(ExcelError::Na),
                &ExcelValue::Number(1.0),
                &ExcelValue::Text("X".into()),
            ),
            Err(ExcelError::Div0)
        );
        assert_eq!(
            both_value(
                &ExcelValue::Array(vec![vec![ExcelValue::Text("A".into())]]),
                &ExcelValue::Number(1.0),
                &ExcelValue::Number(1.0),
                &ExcelValue::Text("X".into()),
            ),
            Err(ExcelError::Value)
        );
    }

    #[test]
    fn formula_microsoft_and_insert_delete() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(\"abcdefghijk\",6,5,\"*\")").unwrap(),
            ExcelValue::Text("abcde*k".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(\"2009\",3,2,\"10\")").unwrap(),
            ExcelValue::Text("2010".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(\"123456\",1,3,\"@\")").unwrap(),
            ExcelValue::Text("@456".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(\"abc\",2,0,\"X\")").unwrap(),
            ExcelValue::Text("aXbc".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(\"abc\",2,1,\"\")").unwrap(),
            ExcelValue::Text("ac".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(\"abc\",4,1,\"X\")").unwrap(),
            ExcelValue::Text("abcX".into())
        );
    }

    #[test]
    fn formula_coercion_trunc_arity() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(2009,3,2,10)").unwrap(),
            ExcelValue::Text("2010".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(TRUE,1,1,\"X\")").unwrap(),
            ExcelValue::Text("XRUE".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(\"abc\",2.9,1,\"X\")").unwrap(),
            ExcelValue::Text("aXc".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(\"abc\",0.9,1,\"X\")").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(\"abc\",2,0.9,\"X\")").unwrap(),
            ExcelValue::Text("aXbc".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(\"abc\",2,-0.9,\"X\")").unwrap(),
            ExcelValue::Text("aXbc".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(\"abc\",200%,1,\"X\")").unwrap(),
            ExcelValue::Text("aXc".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(\"abc\",1E+20,1,\"X\")").unwrap(),
            ExcelValue::Text("abcX".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE()").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(\"abc\",1,1)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(\"abc\",1,1,\"X\",1)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(\"abc\",,1,\"X\")").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(\"abc\",1,,\"X\")").unwrap(),
            ExcelValue::Text("Xabc".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(\"abc\",1,1,)").unwrap(),
            ExcelValue::Text("bc".into())
        );
    }

    #[test]
    fn formula_errors_and_nested() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(1/0,1,1,\"X\")").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(#DIV/0!,#N/A,1,\"X\")").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(\"abc\",NA(),1,\"X\")").unwrap(),
            ExcelValue::Error(ExcelError::Na)
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(LEFT(\"hello\",3),2,1,\"X\")").unwrap(),
            ExcelValue::Text("hXl".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(\"abc\",LEN(\"abc\"),1,\"Z\")").unwrap(),
            ExcelValue::Text("abZ".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(REPLACE(\"abc\",2,1,\"\"))").unwrap(),
            ExcelValue::Number(2.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=EXACT(REPLACE(\"abc\",1,0,\"\"),\"abc\")").unwrap(),
            ExcelValue::Bool(true)
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(\"abc\"&\"def\",4,1,\"X\")").unwrap(),
            ExcelValue::Text("abcXef".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=IFERROR(REPLACE(\"abc\",0,1,\"X\"),\"bad\")").unwrap(),
            ExcelValue::Text("bad".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACEB(\"abc\",1,1,\"X\")").unwrap(),
            ExcelValue::Error(ExcelError::Name)
        );
        assert_eq!(
            eval_formula_in(
                &wb,
                "=TEXTJOIN(\",\",TRUE,MAP({\"abc\";\"def\"},LAMBDA(x,REPLACE(x,1,1,\"X\"))))"
            )
            .unwrap(),
            ExcelValue::Text("Xbc,Xef".into())
        );
    }

    #[test]
    fn formula_unicode_and_cells() {
        let mut sheet = Sheet::new("Sheet1");
        sheet
            .cells
            .insert("A1".into(), Cell::value(ExcelValue::Text("Hello".into())));
        sheet
            .cells
            .insert("A2".into(), Cell::value(ExcelValue::Empty));
        sheet
            .cells
            .insert("A3".into(), Cell::value(ExcelValue::Text("a😀b".into())));
        sheet
            .cells
            .insert("B1".into(), Cell::value(ExcelValue::Text("ONE".into())));
        sheet
            .cells
            .insert("B2".into(), Cell::value(ExcelValue::Text("TWO".into())));
        sheet
            .cells
            .insert("B3".into(), Cell::value(ExcelValue::Text("THREE".into())));
        let wb = Workbook {
            sheets: vec![sheet],
            names: vec![DefinedName {
                name: "Title".into(),
                refers_to: "Sheet1!A1".into(),
            }],
        };
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(A1,1,1,\"Y\")").unwrap(),
            ExcelValue::Text("Yello".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(A2,1,0,\"X\")").unwrap(),
            ExcelValue::Text("X".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(A3,2,1,\"X\")").unwrap(),
            ExcelValue::Text("aXb".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(Title,2,3,\"i\")").unwrap(),
            ExcelValue::Text("Hio".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(\"café\",4,1,\"e\")").unwrap(),
            ExcelValue::Text("cafe".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=REPLACE(\"日本語\",2,1,\"X\")").unwrap(),
            ExcelValue::Text("日X語".into())
        );
    }
}
