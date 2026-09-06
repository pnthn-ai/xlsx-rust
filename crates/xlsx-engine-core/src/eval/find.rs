//! Excel `FIND` kernel.
//!
//! Semantics (desktop Excel / Microsoft docs):
//! - `FIND(find_text, within_text, [start_num])` — **case-sensitive**, no
//!   wildcards (`*` / `?` / `~` are literal; `SEARCH` is a separate
//!   workstream).
//! - 1-based character index. Indexing matches this crate's `LEN` / `MID` /
//!   `LEFT` / `RIGHT` / `REPLACE` / `UNICODE`: Unicode scalar values
//!   (`str::chars`). That is Excel Compatibility Version 2 — a
//!   supplementary-plane emoji is **one** character (`FIND("😀","x😀")` is
//!   2, not 3). Version 1 counted UTF-16 code units; that legacy mode is
//!   not implemented. Combining marks stay separate scalars.
//! - Missing needle → `#VALUE!` (not `#N/A`).
//! - Empty `find_text` matches at `start_num`, including one past
//!   `LEN(within_text)`.
//! - `start_num` is 1-based; omitted (including a trailing-comma slot) means
//!   1. A blank cell / `FALSE` / `0` is `#VALUE!`. `< 1` is `#VALUE!`.
//! - Numbers / bools coerce like `&` before the search. Errors propagate
//!   left-to-right. Wrong arity is `#VALUE!`.
//!
//! Production search uses `str::find` (Two-Way / `memchr`) for Unicode, a
//! 1-byte ASCII `memchr` probe, and an ASCII last-byte SWAR probe for
//! multi-byte needles (the `aaa…aab` almost-match hill-climb). The returned
//! position uses the byte offset on ASCII haystacks so it does not need a
//! second character walk. The value-level path borrows `Text` / bool /
//! empty instead of `to_text` + `String` clone. The quadratic `Vec<char>`
//! sliding-window baseline lives beside that path so benches can report a
//! before/after. This kernel does **not** read fixture goldens.

use std::borrow::Cow;

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

/// Production `FIND` kernel.
///
/// `start_num` is already truncated toward zero (1-based). Returns the
/// 1-based character position, or [`ExcelError::Value`].
pub fn find(find_text: &str, within_text: &str, start_num: i64) -> Result<f64, ExcelError> {
    find_impl(find_text, within_text, start_num, FindMode::Fast)
}

/// Quadratic baseline used for the hill-climb bench (`Vec<char>` + window).
///
/// Same Excel semantics as [`find`]; slower on large haystacks. Kept so
/// `cargo bench -p xlsx-engine-core --bench find` can print before/after.
pub fn find_naive(find_text: &str, within_text: &str, start_num: i64) -> Result<f64, ExcelError> {
    find_impl(find_text, within_text, start_num, FindMode::Naive)
}

/// Production `FIND` on already-evaluated Excel values (no `Text` clone).
///
/// `start_num` is already truncated toward zero. Errors in `find_text`
/// beat errors in `within_text`.
pub fn find_value(
    find_text: &ExcelValue,
    within_text: &ExcelValue,
    start_num: i64,
) -> Result<f64, ExcelError> {
    let needle = text_cow(find_text)?;
    let hay = text_cow(within_text)?;
    find(&needle, &hay, start_num)
}

/// Value-level baseline: full `to_text` clone + [`find_naive`].
pub fn find_value_naive(
    find_text: &ExcelValue,
    within_text: &ExcelValue,
    start_num: i64,
) -> Result<f64, ExcelError> {
    find_naive(
        &coerce::to_text(find_text)?,
        &coerce::to_text(within_text)?,
        start_num,
    )
}

#[derive(Clone, Copy)]
enum FindMode {
    Fast,
    Naive,
}

fn find_impl(
    find_text: &str,
    within_text: &str,
    start_num: i64,
    mode: FindMode,
) -> Result<f64, ExcelError> {
    if start_num < 1 {
        return Err(ExcelError::Value);
    }
    // Characters ≤ bytes. A start past byte-len+1 cannot be a valid char index.
    if start_num as u64 > within_text.len() as u64 + 1 {
        return Err(ExcelError::Value);
    }
    match mode {
        FindMode::Naive => find_chars(find_text, within_text, start_num),
        FindMode::Fast => find_twoway(find_text, within_text, start_num),
    }
}

fn find_chars(find_text: &str, within_text: &str, start_num: i64) -> Result<f64, ExcelError> {
    let hay: Vec<char> = within_text.chars().collect();
    let needle: Vec<char> = find_text.chars().collect();
    let start = (start_num as usize) - 1;
    if needle.is_empty() {
        return if start <= hay.len() {
            Ok(start_num as f64)
        } else {
            Err(ExcelError::Value)
        };
    }
    if start >= hay.len() || start + needle.len() > hay.len() {
        return Err(ExcelError::Value);
    }
    let nlen = needle.len();
    for i in start..=hay.len() - nlen {
        if hay[i..i + nlen] == needle[..] {
            return Ok((i + 1) as f64);
        }
    }
    Err(ExcelError::Value)
}

fn find_twoway(find_text: &str, within_text: &str, start_num: i64) -> Result<f64, ExcelError> {
    let skip = (start_num as usize) - 1;
    let Some(suffix) = skip_chars(within_text, skip) else {
        return Err(ExcelError::Value);
    };
    if find_text.is_empty() {
        return Ok(start_num as f64);
    }
    let Some(byte_off) = search_bytes(suffix, find_text) else {
        return Err(ExcelError::Value);
    };
    let extra = if suffix.is_ascii() {
        byte_off
    } else {
        suffix[..byte_off].chars().count()
    };
    Ok((start_num as usize + extra) as f64)
}

/// `&`-style text without cloning `Text` / bool / empty.
fn text_cow(v: &ExcelValue) -> Result<Cow<'_, str>, ExcelError> {
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

/// `str::find` is already Two-Way/`memchr`. ASCII 1-byte needles use the
/// SWAR `memchr` probe; multi-byte ASCII needles whose last byte is rare
/// in the haystack (the `aaa…aab` hill-climb) probe that last byte then
/// verify — beating a prefix-heavy Two-Way scan.
fn search_bytes(hay: &str, needle: &str) -> Option<usize> {
    if hay.is_ascii() && needle.is_ascii() {
        match needle.len() {
            0 => return Some(0),
            1 => return memchr_byte(hay.as_bytes(), needle.as_bytes()[0]),
            _ => return find_ascii_last_byte(hay.as_bytes(), needle.as_bytes()),
        }
    }
    hay.find(needle)
}

fn find_ascii_last_byte(hay: &[u8], needle: &[u8]) -> Option<usize> {
    debug_assert!(needle.len() >= 2);
    let nlen = needle.len();
    if hay.len() < nlen {
        return None;
    }
    let last = needle[nlen - 1];
    let mut i = nlen - 1;
    while i < hay.len() {
        let Some(rel) = memchr_byte(&hay[i..], last) else {
            return None;
        };
        let end = i + rel;
        let start = end + 1 - nlen;
        if &hay[start..=end] == needle {
            return Some(start);
        }
        i = end + 1;
    }
    None
}

/// Word-at-a-time `memchr`. Faster than a scalar scan on large haystacks;
/// enough of a hill-climb that we do not need a `memchr` crate dep.
fn memchr_byte(hay: &[u8], needle: u8) -> Option<usize> {
    const W: usize = std::mem::size_of::<usize>();
    let splat = usize::from(needle).wrapping_mul(usize::from_ne_bytes([0x01; W]));
    let ones = usize::from_ne_bytes([0x01; W]);
    let highs = usize::from_ne_bytes([0x80; W]);
    let mut i = 0;
    while i + W <= hay.len() {
        // SAFETY: `i + W <= hay.len()`, and we only read `W` bytes.
        let word = unsafe { std::ptr::read_unaligned(hay.as_ptr().add(i).cast::<usize>()) };
        let xor = word ^ splat;
        let mask = xor.wrapping_sub(ones) & !xor & highs;
        if mask != 0 {
            for j in 0..W {
                if hay[i + j] == needle {
                    return Some(i + j);
                }
            }
        }
        i += W;
    }
    hay[i..].iter().position(|&b| b == needle).map(|p| i + p)
}

fn skip_chars(s: &str, n: usize) -> Option<&str> {
    if s.is_ascii() {
        if n > s.len() {
            None
        } else {
            Some(&s[n..])
        }
    } else {
        let mut iter = s.chars();
        for _ in 0..n {
            iter.next()?;
        }
        Some(iter.as_str())
    }
}

/// Production FIND (scalar args, borrow / SWAR kernel).
pub(crate) fn fn_find(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 || args.len() > 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let find_text = ev.eval_scalar(&args[0], ctx)?;
    let within_text = ev.eval_scalar(&args[1], ctx)?;
    // Omitted optional start_num (not provided, or a trailing-comma slot)
    // defaults to 1. A blank cell is Empty → 0 → #VALUE!, which is different.
    let start_num = if args.len() == 3 && !args[2].is_omitted() {
        match coerce::to_number(&ev.eval_scalar(&args[2], ctx)?) {
            Ok(n) => {
                if !n.is_finite() {
                    return Ok(ExcelValue::Error(ExcelError::Value));
                }
                n.trunc() as i64
            }
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        1
    };
    match find_value(&find_text, &within_text, start_num) {
        Ok(pos) => Ok(ExcelValue::Number(pos)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::eval_formula_in;
    use xlsx_types::{Cell, ExcelError, ExcelValue, Sheet, Workbook};

    fn both(needle: &str, hay: &str, start: i64) -> Result<f64, ExcelError> {
        let fast = find(needle, hay, start);
        let slow = find_naive(needle, hay, start);
        assert_eq!(
            fast, slow,
            "naive/fast mismatch for {needle:?} in {hay:?} start={start}"
        );
        fast
    }

    fn both_value(needle: &ExcelValue, hay: &ExcelValue, start: i64) -> Result<f64, ExcelError> {
        let fast = find_value(needle, hay, start);
        let slow = find_value_naive(needle, hay, start);
        assert_eq!(
            fast, slow,
            "value naive/fast mismatch for {needle:?} in {hay:?} start={start}"
        );
        fast
    }

    fn t(s: &str) -> ExcelValue {
        ExcelValue::Text(s.into())
    }

    #[test]
    fn microsoft_examples() {
        assert_eq!(both("M", "Miriam McGovern", 1), Ok(1.0));
        assert_eq!(both("m", "Miriam McGovern", 1), Ok(6.0));
        assert_eq!(both("M", "Miriam McGovern", 3), Ok(8.0));
    }

    #[test]
    fn start_num_and_miss() {
        assert_eq!(both("a", "banana", 3), Ok(4.0));
        assert_eq!(both("a", "banana", 6), Ok(6.0));
        assert_eq!(both("a", "banana", 7), Err(ExcelError::Value));
        assert_eq!(both("z", "abc", 1), Err(ExcelError::Value));
        assert_eq!(both("a", "abc", 0), Err(ExcelError::Value));
        assert_eq!(both("a", "abc", -1), Err(ExcelError::Value));
        assert_eq!(both("a", "abc", 4), Err(ExcelError::Value));
        assert_eq!(both("abcd", "abc", 1), Err(ExcelError::Value));
        assert_eq!(both("abc", "abc", 1), Ok(1.0));
    }

    #[test]
    fn empty_find_text() {
        assert_eq!(both("", "abc", 1), Ok(1.0));
        assert_eq!(both("", "abc", 3), Ok(3.0));
        assert_eq!(both("", "abc", 4), Ok(4.0));
        assert_eq!(both("", "abc", 5), Err(ExcelError::Value));
        assert_eq!(both("", "", 1), Ok(1.0));
        assert_eq!(both("a", "", 1), Err(ExcelError::Value));
    }

    #[test]
    fn case_sensitive_unlike_search() {
        assert_eq!(both("a", "ABC", 1), Err(ExcelError::Value));
        assert_eq!(both("A", "ABC", 1), Ok(1.0));
        assert_eq!(both("bc", "ABC", 1), Err(ExcelError::Value));
        assert_eq!(both("BC", "ABC", 1), Ok(2.0));
        assert_eq!(both("B", "aaB", 1), Ok(3.0));
    }

    #[test]
    fn wildcards_and_tilde_are_literal() {
        assert_eq!(both("*", "abc", 1), Err(ExcelError::Value));
        assert_eq!(both("a*", "abc", 1), Err(ExcelError::Value));
        assert_eq!(both("*", "a*b", 1), Ok(2.0));
        assert_eq!(both("?", "a?b", 1), Ok(2.0));
        assert_eq!(both("~", "a~b", 1), Ok(2.0));
        assert_eq!(both("~*", "a*b", 1), Err(ExcelError::Value));
        assert_eq!(both("~*", "a~*b", 1), Ok(2.0));
    }

    #[test]
    fn unicode_scalar_index_compat_v2() {
        assert_eq!(both("é", "café", 1), Ok(4.0));
        assert_eq!(both("é", "café", 4), Ok(4.0));
        assert_eq!(both("é", "café", 5), Err(ExcelError::Value));
        assert_eq!(both("日", "日本語", 1), Ok(1.0));
        assert_eq!(both("本", "日本語", 1), Ok(2.0));
        assert_eq!(both("語", "日本語", 1), Ok(3.0));
        assert_eq!(both("é", "cafe", 1), Err(ExcelError::Value));
        // U+1F600 is one scalar (not UTF-16 high surrogate).
        assert_eq!(both("😀", "x😀y", 1), Ok(2.0));
        assert_eq!(both("y", "x😀y", 1), Ok(3.0));
        assert_eq!(both("😀", "😀", 1), Ok(1.0));
        // Combining acute is not precomposed é.
        assert_eq!(both("é", "e\u{0301}", 1), Err(ExcelError::Value));
        assert_eq!(both("\u{0301}", "e\u{0301}", 1), Ok(2.0));
        assert_eq!(both("a", "éabc", 2), Ok(2.0));
        assert_eq!(both(" ", "a b", 1), Ok(2.0));
        assert_eq!(both("\t", "a\tb", 1), Ok(2.0));
        assert_eq!(both(" ", "a\tb", 1), Err(ExcelError::Value));
        assert_eq!(both("\u{00a0}", "a\u{00a0}b", 1), Ok(2.0));
    }

    #[test]
    fn overlapping_and_first_match() {
        assert_eq!(both("aa", "aaa", 1), Ok(1.0));
        assert_eq!(both("aa", "aaa", 2), Ok(2.0));
        assert_eq!(both("aa", "aaa", 3), Err(ExcelError::Value));
        assert_eq!(both("an", "banana", 1), Ok(2.0));
        assert_eq!(both("an", "banana", 3), Ok(4.0));
    }

    #[test]
    fn huge_start_rejects_without_scan() {
        assert_eq!(both("a", "abc", i64::MAX), Err(ExcelError::Value));
    }

    #[test]
    fn almost_match_suffix() {
        let hay = format!("{}aab", "aaa".repeat(80));
        assert_eq!(both("aab", &hay, 1), Ok((hay.len() - 2) as f64));
        assert_eq!(both("aac", &hay, 1), Err(ExcelError::Value));
    }

    #[test]
    fn ascii_one_byte_memchr() {
        let hay = format!("{}z", "x".repeat(4_000));
        assert_eq!(both("z", &hay, 1), Ok(4_001.0));
        assert_eq!(both("z", &hay, 4_000), Ok(4_001.0));
        assert_eq!(both("y", &hay, 1), Err(ExcelError::Value));
        assert_eq!(both("x", &hay, 2), Ok(2.0));
    }

    #[test]
    fn value_borrows_text_and_bools() {
        assert_eq!(both_value(&t("M"), &t("Miriam McGovern"), 1), Ok(1.0));
        assert_eq!(both_value(&t(""), &t("abc"), 1), Ok(1.0));
        assert_eq!(both_value(&ExcelValue::Empty, &t("abc"), 1), Ok(1.0));
        assert_eq!(
            both_value(&t("a"), &ExcelValue::Empty, 1),
            Err(ExcelError::Value)
        );
        assert_eq!(
            both_value(&ExcelValue::Bool(true), &t("TRUEBLUE"), 1),
            Ok(1.0)
        );
        assert_eq!(both_value(&t("R"), &ExcelValue::Bool(true), 1), Ok(2.0));
        assert_eq!(
            both_value(&ExcelValue::Number(2.0), &ExcelValue::Number(12321.0), 1),
            Ok(2.0)
        );
        assert_eq!(both_value(&t("."), &ExcelValue::Number(12.5), 1), Ok(3.0));
        assert_eq!(both_value(&t("-"), &ExcelValue::Number(-0.0), 1), Ok(1.0));
        assert_eq!(
            both_value(&t("-"), &ExcelValue::Number(0.0), 1),
            Err(ExcelError::Value)
        );
        assert_eq!(
            both_value(&ExcelValue::Error(ExcelError::Div0), &t("abc"), 1),
            Err(ExcelError::Div0)
        );
        assert_eq!(
            both_value(&t("a"), &ExcelValue::Error(ExcelError::Na), 1),
            Err(ExcelError::Na)
        );
        let long = "x".repeat(8_000) + "needle";
        assert_eq!(both_value(&t("needle"), &t(&long), 1), Ok(8_001.0));
    }

    #[test]
    fn formula_microsoft_and_omitted_start() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=FIND(\"M\", \"Miriam McGovern\")").unwrap(),
            ExcelValue::Number(1.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIND(\"m\", \"Miriam McGovern\")").unwrap(),
            ExcelValue::Number(6.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIND(\"M\", \"Miriam McGovern\", 3)").unwrap(),
            ExcelValue::Number(8.0)
        );
        // Trailing-comma omitted start_num uses the default 1, not Empty→0.
        assert_eq!(
            eval_formula_in(&wb, "=FIND(\"M\", \"Miriam McGovern\",)").unwrap(),
            ExcelValue::Number(1.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIND(,\"abc\")").unwrap(),
            ExcelValue::Number(1.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIND(\"a\",)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIND(,)").unwrap(),
            ExcelValue::Number(1.0)
        );
    }

    #[test]
    fn formula_coercion_nested_and_arity() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=FIND(2, 12321)").unwrap(),
            ExcelValue::Number(2.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIND(\"R\", TRUE)").unwrap(),
            ExcelValue::Number(2.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIND(\"1\", 10%)").unwrap(),
            ExcelValue::Number(3.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIND(\"a\", LEFT(\"banana\", 3))").unwrap(),
            ExcelValue::Number(2.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIND(\"\", \"abc\", LEN(\"abc\")+1)").unwrap(),
            ExcelValue::Number(4.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIND(CHAR(65), \"ABC\")").unwrap(),
            ExcelValue::Number(1.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIND(\"b\", REPT(\"a\", 10)&\"b\")").unwrap(),
            ExcelValue::Number(11.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIND(\"c\", \"ab\"&\"cd\")").unwrap(),
            ExcelValue::Number(3.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIND(\"1\", DATE(1900,1,1))").unwrap(),
            ExcelValue::Number(1.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIND(\"M\", \"Miriam McGovern\")+1").unwrap(),
            ExcelValue::Number(2.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=IFERROR(FIND(\"z\", \"abc\"), 0)").unwrap(),
            ExcelValue::Number(0.0)
        );
        assert_eq!(
            eval_formula_in(
                &wb,
                "=SUM(MAP({\"cat\";\"bat\";\"rat\"},LAMBDA(x,FIND(\"a\",x))))"
            )
            .unwrap(),
            ExcelValue::Number(6.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIND()").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIND(\"a\")").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIND(\"a\", \"abc\", 1, 1)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIND(1/0, \"abc\")").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIND(#DIV/0!, #N/A)").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIND(\"a\", \"abc\", 1E+20)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIND({\"a\",\"z\"}, \"abc\")").unwrap(),
            ExcelValue::Number(1.0)
        );
    }

    #[test]
    fn formula_blank_cell_vs_omitted() {
        let mut sheet = Sheet::new("Sheet1");
        sheet
            .cells
            .insert("A1".into(), Cell::value(ExcelValue::Empty));
        sheet
            .cells
            .insert("A2".into(), Cell::value(ExcelValue::Text("banana".into())));
        sheet
            .cells
            .insert("A3".into(), Cell::value(ExcelValue::Text(String::new())));
        let wb = Workbook {
            sheets: vec![sheet],
            names: vec![],
        };
        // Blank start_num cell coerces to 0 → #VALUE! (not the omitted default).
        assert_eq!(
            eval_formula_in(&wb, "=FIND(\"a\", \"abc\", A1)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIND(A1, \"abc\")").unwrap(),
            ExcelValue::Number(1.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIND(\"a\", A2, 3)").unwrap(),
            ExcelValue::Number(4.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIND(\"a\", A3)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
    }
}
