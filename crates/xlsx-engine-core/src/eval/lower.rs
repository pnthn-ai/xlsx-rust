//! Excel `LOWER` kernel.
//!
//! Semantics (desktop Excel / Microsoft docs, invariant / en-US):
//! - `LOWER(text)` converts uppercase **letters** to lowercase.
//! - Non-letters are unchanged (digits, punctuation, CJK, emoji, spaces).
//! - ASCII `A–Z` → `a–z` (set the `0x20` bit). Other ASCII is copied as-is.
//! - Non-ASCII letters use Unicode default case mapping (`str::to_lowercase`).
//!   Locale-specific mappings (Turkish `I` / `İ`) are not modeled.
//! - Numbers / bools / blanks coerce like `&` before lowering.
//! - Wrong arity is `#VALUE!`. Errors propagate.
//!
//! Production path: SWAR A–Z probe + in-place / one-copy ASCII fold; Unicode
//! falls through to `to_lowercase`. The `Vec<char>` baseline lives beside that
//! path so benches can report a before/after. This kernel does **not** read
//! fixture goldens.

/// Production `LOWER` kernel.
pub fn lower(text: &str) -> String {
    lower_fast(text)
}

/// Consume an already-owned string (the `to_text` result) and lowercase it.
///
/// ASCII input is folded in place — no second allocation. Unicode input that
/// contains no uppercase scalar is returned unchanged.
pub fn lower_owned(text: String) -> String {
    if text.is_ascii() {
        lower_ascii_owned(text)
    } else {
        lower_unicode(&text)
    }
}

/// `Vec<char>` baseline used for the hill-climb bench.
///
/// Same Excel semantics as [`lower`]. Kept so
/// `cargo bench -p xlsx-engine-core --bench lower` can print before/after.
pub fn lower_naive(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    for c in chars {
        for d in c.to_lowercase() {
            out.push(d);
        }
    }
    out
}

fn lower_fast(text: &str) -> String {
    if text.is_ascii() {
        lower_ascii(text)
    } else {
        lower_unicode(text)
    }
}

fn lower_unicode(text: &str) -> String {
    // `to_lowercase` always allocates. Skip it when no scalar maps away.
    // Titlecase (`Lt`) is not `is_uppercase` but still folds, so also catch
    // any char whose default lowercase mapping is not itself.
    if !needs_unicode_fold(text) {
        return text.to_owned();
    }
    text.to_lowercase()
}

fn needs_unicode_fold(text: &str) -> bool {
    for c in text.chars() {
        if c.is_ascii() {
            if c.is_ascii_uppercase() {
                return true;
            }
            continue;
        }
        let mut it = c.to_lowercase();
        if it.next() != Some(c) || it.next().is_some() {
            return true;
        }
    }
    false
}

fn lower_ascii(text: &str) -> String {
    debug_assert!(text.is_ascii());
    let src = text.as_bytes();
    let n = src.len();
    if n == 0 {
        return String::new();
    }
    if !has_ascii_upper(src) {
        return text.to_owned();
    }
    let mut out = Vec::with_capacity(n);
    let dst: *mut u8 = out.as_mut_ptr();
    let mut i = 0;
    while i + 8 <= n {
        let w = u64::from_le_bytes(src[i..i + 8].try_into().unwrap());
        let lowered = w | (ascii_upper_hi(w) >> 2);
        unsafe {
            core::ptr::write_unaligned(dst.add(i).cast::<u64>(), lowered.to_le());
        }
        i += 8;
    }
    while i < n {
        let b = src[i];
        unsafe {
            *dst.add(i) = if b.is_ascii_uppercase() { b + 32 } else { b };
        }
        i += 1;
    }
    // SAFETY: `n` ASCII bytes were written; A–Z → a–z stays valid UTF-8.
    unsafe {
        out.set_len(n);
        String::from_utf8_unchecked(out)
    }
}

fn lower_ascii_owned(mut text: String) -> String {
    debug_assert!(text.is_ascii());
    // SAFETY: ASCII A–Z → a–z stays valid UTF-8.
    let bytes = unsafe { text.as_bytes_mut() };
    if !has_ascii_upper(bytes) {
        return text;
    }
    lower_ascii_in_place(bytes);
    text
}

fn lower_ascii_in_place(bytes: &mut [u8]) {
    let n = bytes.len();
    let mut i = 0;
    while i + 8 <= n {
        let w = u64::from_le_bytes(bytes[i..i + 8].try_into().unwrap());
        let lowered = w | (ascii_upper_hi(w) >> 2);
        bytes[i..i + 8].copy_from_slice(&lowered.to_le_bytes());
        i += 8;
    }
    while i < n {
        let b = bytes[i];
        if b.is_ascii_uppercase() {
            bytes[i] = b + 32;
        }
        i += 1;
    }
}

const HI: u64 = 0x8080_8080_8080_8080;
const SPLAT_A: u64 = 0x4141_4141_4141_4141;
const SPLAT_Z: u64 = 0x5A5A_5A5A_5A5A_5A5A;

/// High bit set in each ASCII byte that is `A..=Z`.
///
/// `w` must be ASCII (no high bits set) so `(w | HI) - splat` cannot borrow
/// across byte lanes.
fn ascii_upper_hi(w: u64) -> u64 {
    let ge_a = (w | HI).wrapping_sub(SPLAT_A) & HI;
    let le_z = (SPLAT_Z | HI).wrapping_sub(w) & HI;
    ge_a & le_z
}

fn has_ascii_upper(hay: &[u8]) -> bool {
    let n = hay.len();
    let mut i = 0;
    while i + 8 <= n {
        let w = u64::from_le_bytes(hay[i..i + 8].try_into().unwrap());
        if ascii_upper_hi(w) != 0 {
            return true;
        }
        i += 8;
    }
    hay[i..].iter().any(|&b| b.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::eval_formula_in;
    use xlsx_types::{Cell, ExcelError, ExcelValue, Sheet, Workbook};

    fn both(s: &str) -> String {
        let fast = lower(s);
        let slow = lower_naive(s);
        assert_eq!(fast, slow, "naive/fast mismatch for {s:?}");
        let owned = lower_owned(s.to_owned());
        assert_eq!(fast, owned, "owned/fast mismatch for {s:?}");
        fast
    }

    #[test]
    fn microsoft_examples() {
        assert_eq!(both("E. E. Cummings"), "e. e. cummings");
        assert_eq!(both("Apt. 2B"), "apt. 2b");
    }

    #[test]
    fn ascii_letters_and_identity() {
        assert_eq!(both(""), "");
        assert_eq!(both("abc"), "abc");
        assert_eq!(both("ABC"), "abc");
        assert_eq!(both("AbC"), "abc");
        assert_eq!(both("already lower"), "already lower");
        assert_eq!(both("ABC-123-XYZ"), "abc-123-xyz");
        assert_eq!(both("  Hello  "), "  hello  ");
        assert_eq!(both("TRUE"), "true");
        assert_eq!(both("FALSE"), "false");
        assert_eq!(both("123"), "123");
        assert_eq!(both("@[`{"), "@[`{");
    }

    #[test]
    fn non_letters_stay() {
        assert_eq!(both("\ta\t"), "\ta\t");
        assert_eq!(both("\nA\n"), "\na\n");
        assert_eq!(both("\rA\r"), "\ra\r");
        assert_eq!(both("!@#$%"), "!@#$%");
    }

    #[test]
    fn unicode_letters_and_non_letters() {
        assert_eq!(both("CAFÉ"), "café");
        assert_eq!(both("café"), "café");
        assert_eq!(both("ÄÖÜ"), "äöü");
        assert_eq!(both("日本語"), "日本語");
        assert_eq!(both("😀🎉"), "😀🎉");
        assert_eq!(both("Straße"), "straße");
        assert_eq!(both("ΑΒΓ"), "αβγ");
    }

    #[test]
    fn identity_and_large() {
        let clean = "x".repeat(4096);
        assert_eq!(both(&clean), clean);
        let uppers = "A".repeat(4096);
        assert_eq!(both(&uppers), "a".repeat(4096));
        let mixed = "Ab".repeat(2048);
        assert_eq!(both(&mixed), "ab".repeat(2048));
        let cafe = "CAFÉ".repeat(256);
        assert_eq!(both(&cafe), "café".repeat(256));
    }

    #[test]
    fn swar_word_edges() {
        // Lane boundaries around A/@/Z/[ and a 8-byte word of mixed case.
        assert!(has_ascii_upper(b"A"));
        assert!(has_ascii_upper(b"Z"));
        assert!(!has_ascii_upper(b"@"));
        assert!(!has_ascii_upper(b"["));
        assert!(!has_ascii_upper(b"a"));
        assert!(!has_ascii_upper(b""));
        assert!(!has_ascii_upper(b"abcdefg"));
        assert!(has_ascii_upper(b"abcdefgH"));
        assert!(has_ascii_upper(b"1234567A"));
        assert!(!has_ascii_upper(b"12345678"));
        assert!(has_ascii_upper(b"12345678A"));
        let word_a = u64::from_le_bytes(*b"A@@@@@@@");
        assert_eq!(ascii_upper_hi(word_a).to_le_bytes()[0], 0x80);
        let word_at = u64::from_le_bytes(*b"@@@@@@@@");
        assert_eq!(ascii_upper_hi(word_at), 0);
        let word_z = u64::from_le_bytes(*b"Z[[[[[[[");
        assert_eq!(ascii_upper_hi(word_z).to_le_bytes()[0], 0x80);
    }

    #[test]
    fn formula_coercion_and_arity() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=LOWER(\"AbC\")").unwrap(),
            ExcelValue::Text("abc".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=LOWER(123)").unwrap(),
            ExcelValue::Text("123".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=LOWER(1.5)").unwrap(),
            ExcelValue::Text("1.5".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=LOWER(-2)").unwrap(),
            ExcelValue::Text("-2".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=LOWER(TRUE)").unwrap(),
            ExcelValue::Text("true".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=LOWER(FALSE)").unwrap(),
            ExcelValue::Text("false".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=LOWER()").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LOWER(\"a\",\"b\")").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LOWER(1/0)").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LOWER(NA())").unwrap(),
            ExcelValue::Error(ExcelError::Na)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LOWER(LOWER(\"AbC\"))").unwrap(),
            ExcelValue::Text("abc".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(LOWER(\"AbC\"))").unwrap(),
            ExcelValue::Number(3.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LOWER(UPPER(\"abc\"))").unwrap(),
            ExcelValue::Text("abc".into())
        );
    }

    #[test]
    fn formula_blank_and_cell() {
        let mut sheet = Sheet::new("Sheet1");
        sheet
            .cells
            .insert("A1".into(), Cell::value(ExcelValue::Text("HeLLo".into())));
        sheet
            .cells
            .insert("A2".into(), Cell::value(ExcelValue::Empty));
        sheet
            .cells
            .insert("A3".into(), Cell::value(ExcelValue::Text(String::new())));
        let wb = Workbook {
            sheets: vec![sheet],
            names: vec![],
        };
        assert_eq!(
            eval_formula_in(&wb, "=LOWER(A1)").unwrap(),
            ExcelValue::Text("hello".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=LOWER(A2)").unwrap(),
            ExcelValue::Text(String::new())
        );
        assert_eq!(
            eval_formula_in(&wb, "=LOWER(A3)").unwrap(),
            ExcelValue::Text(String::new())
        );
        assert_eq!(
            eval_formula_in(&wb, "=LOWER(B1)").unwrap(),
            ExcelValue::Text(String::new())
        );
    }
}
