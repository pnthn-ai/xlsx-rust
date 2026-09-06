//! Excel `UPPER` kernel.
//!
//! Semantics (desktop Excel / Microsoft docs):
//! - `UPPER(text)` converts lowercase letters to uppercase. Digits,
//!   punctuation, spaces, and uncased scalars are unchanged.
//! - ASCII `a–z` → `A–Z` (invariant, not Turkish).
//! - Precomposed Latin / Greek / Cyrillic letters follow Unicode default
//!   uppercase (`é` → `É`). Combining marks stay separate scalars (same
//!   as `LEN` / `MID`).
//! - Sharp s `ß` is **not** converted to `SS` (Microsoft `UPPER` note).
//!   Other 1→N Unicode special casings still apply.
//!
//! Production path: SWAR lowercase probe + word-wise XOR for ASCII, an
//! in-place Latin-1 (`C3 xx`) rewrite, then a reserved-buffer Unicode walk.
//! The `Vec<char>` baseline lives beside that path so benches can report a
//! before/after.

/// Production `UPPER` kernel.
pub fn upper(text: &str) -> String {
    upper_fast(text)
}

/// `Vec<char>` baseline used for the hill-climb bench.
///
/// Same Excel semantics as [`upper`]. Kept so
/// `cargo bench -p xlsx-engine-core --bench upper` can print before/after.
pub fn upper_naive(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    for c in chars {
        map_upper(c, &mut out);
    }
    out
}

fn upper_fast(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    if text.is_ascii() {
        return upper_ascii(text.as_bytes());
    }
    if let Some(out) = try_upper_latin1(text) {
        return out;
    }
    upper_unicode(text)
}

/// Fast path for ASCII + precomposed Latin-1 (`U+00C0`–`U+00FF`, UTF-8 `C3 xx`).
///
/// `ÿ` (`C3 BF` → `Ÿ` `C5 B8`) changes byte length, so that string falls
/// through to the Unicode walk. `ß` (`C3 9F`) is left unchanged.
fn try_upper_latin1(text: &str) -> Option<String> {
    let src = text.as_bytes();
    let n = src.len();
    let mut i = 0;
    let mut need = false;
    while i < n {
        let b = src[i];
        if b < 0x80 {
            if b.is_ascii_lowercase() {
                need = true;
            }
            i += 1;
            continue;
        }
        if b != 0xC3 || i + 1 >= n {
            return None;
        }
        let c = src[i + 1];
        if c == 0xBF {
            // ÿ → Ÿ is not an in-place UTF-8 rewrite.
            return None;
        }
        if is_latin1_lower_c3(c) {
            need = true;
        }
        i += 2;
    }
    if !need {
        return Some(text.to_owned());
    }
    let mut out = Vec::with_capacity(n);
    let dst: *mut u8 = out.as_mut_ptr();
    let mut i = 0;
    while i < n {
        let b = src[i];
        if b < 0x80 {
            let u = if b.is_ascii_lowercase() { b - 0x20 } else { b };
            unsafe {
                *dst.add(i) = u;
            }
            i += 1;
        } else {
            let c = src[i + 1];
            let u = if is_latin1_lower_c3(c) { c - 0x20 } else { c };
            unsafe {
                *dst.add(i) = 0xC3;
                *dst.add(i + 1) = u;
            }
            i += 2;
        }
    }
    // SAFETY: ASCII / `C3 xx` rewrite keeps valid UTF-8 of the same length.
    Some(unsafe {
        out.set_len(n);
        String::from_utf8_unchecked(out)
    })
}

/// Second byte of UTF-8 `C3 xx` for Latin-1 lowercase letters except `ß` / `ÿ`.
fn is_latin1_lower_c3(c: u8) -> bool {
    // à–ö (A0–B6) and ø–þ (B8–BE). ÷ (B7) and ß (9F) stay.
    (0xA0..=0xB6).contains(&c) || (0xB8..=0xBE).contains(&c)
}

fn upper_ascii(src: &[u8]) -> String {
    if !has_ascii_lower(src) {
        // SAFETY: `src` is a `&str` byte slice (ASCII subset).
        return unsafe { std::str::from_utf8_unchecked(src) }.to_owned();
    }
    let n = src.len();
    let mut out = Vec::with_capacity(n);
    let dst: *mut u8 = out.as_mut_ptr();
    let mut i = 0;
    while i + 8 <= n {
        let w = u64::from_le_bytes(src[i..i + 8].try_into().unwrap());
        let up = w ^ (ascii_lower_mask(w) >> 2);
        let bytes = up.to_le_bytes();
        // SAFETY: `i + 8 <= n` and `out` has capacity `n`.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst.add(i), 8);
        }
        i += 8;
    }
    while i < n {
        let b = src[i];
        let u = if b.is_ascii_lowercase() { b - 0x20 } else { b };
        unsafe {
            *dst.add(i) = u;
        }
        i += 1;
    }
    // SAFETY: every written byte is ASCII (lowercase flipped to uppercase).
    unsafe {
        out.set_len(n);
        String::from_utf8_unchecked(out)
    }
}

fn upper_unicode(text: &str) -> String {
    if !needs_unicode_upper(text) {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        map_upper(c, &mut out);
    }
    out
}

fn needs_unicode_upper(text: &str) -> bool {
    text.chars().any(|c| c != 'ß' && c.is_lowercase())
}

fn map_upper(c: char, out: &mut String) {
    if c == 'ß' {
        // Microsoft: UPPER does not convert ß to SS.
        out.push('ß');
    } else {
        out.extend(c.to_uppercase());
    }
}

const HI: u64 = 0x8080_8080_8080_8080;

/// High bit set in each lane whose byte is ASCII lowercase `a–z`.
fn ascii_lower_mask(w: u64) -> u64 {
    // Bytes with the high bit set are non-ASCII; never treat them as `a–z`.
    let ascii = !w & HI;
    // For b < 0x80: b + 0x1F has bit 7 set iff b >= 'a' (0x61).
    let ge_a = w.wrapping_add(0x1F1F_1F1F_1F1F_1F1F) & HI;
    // For b < 0x80: b + 0x05 has bit 7 set iff b >= 'z'+1 (0x7B).
    let ge_z1 = w.wrapping_add(0x0505_0505_0505_0505) & HI;
    ge_a & !ge_z1 & ascii
}

fn has_ascii_lower(bytes: &[u8]) -> bool {
    let mut i = 0;
    let n = bytes.len();
    while i + 8 <= n {
        let w = u64::from_le_bytes(bytes[i..i + 8].try_into().unwrap());
        if ascii_lower_mask(w) != 0 {
            return true;
        }
        i += 8;
    }
    bytes[i..].iter().any(|&b| b.is_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::eval_formula_in;
    use xlsx_types::{Cell, ExcelError, ExcelValue, Sheet, Workbook};

    fn both(s: &str) -> String {
        let fast = upper(s);
        let slow = upper_naive(s);
        assert_eq!(fast, slow, "naive/fast mismatch for {s:?}");
        fast
    }

    #[test]
    fn microsoft_examples() {
        assert_eq!(both("total"), "TOTAL");
        assert_eq!(both("Total"), "TOTAL");
        assert_eq!(both("AbC"), "ABC");
    }

    #[test]
    fn ascii_letters_and_identity() {
        assert_eq!(both(""), "");
        assert_eq!(both("ABC"), "ABC");
        assert_eq!(both("abc"), "ABC");
        assert_eq!(both("a"), "A");
        assert_eq!(both("z"), "Z");
        assert_eq!(both("A"), "A");
        assert_eq!(both("Z"), "Z");
        assert_eq!(both("Hello World"), "HELLO WORLD");
        assert_eq!(both("already UPPER"), "ALREADY UPPER");
    }

    #[test]
    fn digits_punctuation_spaces_stay() {
        assert_eq!(both("123"), "123");
        assert_eq!(both("a1b2"), "A1B2");
        assert_eq!(both("  abc  "), "  ABC  ");
        assert_eq!(both("e-mail!"), "E-MAIL!");
        assert_eq!(both("\tfoo\n"), "\tFOO\n");
        assert_eq!(both("@[`{"), "@[`{");
    }

    #[test]
    fn sharp_s_not_converted() {
        // Microsoft docs: UPPER does not convert ß to SS.
        assert_eq!(both("ß"), "ß");
        assert_eq!(both("straße"), "STRAßE");
        assert_eq!(both("SS"), "SS");
        assert_eq!(both("ẞ"), "ẞ"); // already capital sharp s
    }

    #[test]
    fn unicode_letters() {
        assert_eq!(both("café"), "CAFÉ");
        assert_eq!(both("CAFÉ"), "CAFÉ");
        assert_eq!(both("niño"), "NIÑO");
        assert_eq!(both("über"), "ÜBER");
        // ÿ → Ÿ changes UTF-8 length; must not use the Latin-1 in-place path.
        assert_eq!(both("ÿ"), "Ÿ");
        assert_eq!(both("piaffe ÿ"), "PIAFFE Ÿ");
        assert_eq!(both("αβγ"), "ΑΒΓ");
        assert_eq!(both("русский"), "РУССКИЙ");
        assert_eq!(both("日本語"), "日本語");
        assert_eq!(both("😀abc🎉"), "😀ABC🎉");
    }

    #[test]
    fn identity_and_large() {
        let clean = "X".repeat(4096);
        assert_eq!(both(&clean), clean);
        let lower = "x".repeat(4096);
        assert_eq!(both(&lower), "X".repeat(4096));
        let mixed = "aB".repeat(2048);
        assert_eq!(both(&mixed), "AB".repeat(2048));
        let cafe = "café".repeat(256);
        assert_eq!(both(&cafe), "CAFÉ".repeat(256));
        let sharp = "ß".repeat(256);
        assert_eq!(both(&sharp), sharp);
    }

    #[test]
    fn formula_coercion_and_arity() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=UPPER(\"AbC\")").unwrap(),
            ExcelValue::Text("ABC".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=UPPER(123)").unwrap(),
            ExcelValue::Text("123".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=UPPER(1.5)").unwrap(),
            ExcelValue::Text("1.5".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=UPPER(TRUE)").unwrap(),
            ExcelValue::Text("TRUE".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=UPPER(FALSE)").unwrap(),
            ExcelValue::Text("FALSE".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=UPPER()").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=UPPER(\"a\",\"b\")").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=UPPER(1/0)").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=UPPER(NA())").unwrap(),
            ExcelValue::Error(ExcelError::Na)
        );
        assert_eq!(
            eval_formula_in(&wb, "=UPPER(UPPER(\"AbC\"))").unwrap(),
            ExcelValue::Text("ABC".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(UPPER(\"AbC\"))").unwrap(),
            ExcelValue::Number(3.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=EXACT(UPPER(\"AbC\"), \"ABC\")").unwrap(),
            ExcelValue::Bool(true)
        );
    }

    #[test]
    fn formula_blank_and_cell() {
        let mut sheet = Sheet::new("Sheet1");
        sheet
            .cells
            .insert("A1".into(), Cell::value(ExcelValue::Text("hello".into())));
        sheet
            .cells
            .insert("A2".into(), Cell::value(ExcelValue::Empty));
        sheet
            .cells
            .insert("A3".into(), Cell::value(ExcelValue::Text(String::new())));
        sheet
            .cells
            .insert("A4".into(), Cell::value(ExcelValue::Text("café".into())));
        let wb = Workbook {
            sheets: vec![sheet],
            names: vec![],
        };
        assert_eq!(
            eval_formula_in(&wb, "=UPPER(A1)").unwrap(),
            ExcelValue::Text("HELLO".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=UPPER(A2)").unwrap(),
            ExcelValue::Text(String::new())
        );
        assert_eq!(
            eval_formula_in(&wb, "=UPPER(A3)").unwrap(),
            ExcelValue::Text(String::new())
        );
        assert_eq!(
            eval_formula_in(&wb, "=UPPER(B1)").unwrap(),
            ExcelValue::Text(String::new())
        );
        assert_eq!(
            eval_formula_in(&wb, "=UPPER(A4)").unwrap(),
            ExcelValue::Text("CAFÉ".into())
        );
    }

    #[test]
    fn swar_mask_on_short_and_long() {
        assert!(!has_ascii_lower(b""));
        assert!(!has_ascii_lower(b"ABC"));
        assert!(!has_ascii_lower(b"12345678"));
        assert!(!has_ascii_lower(b"@[`{XYZ"));
        assert!(has_ascii_lower(b"a"));
        assert!(has_ascii_lower(b"z"));
        assert!(has_ascii_lower(b"ABCDx"));
        assert!(has_ascii_lower(b"1234567a"));
        assert!(has_ascii_lower(b"12345678a"));
        // Word with mixed case at every lane.
        let mixed = b"aBcDeFgH";
        assert!(has_ascii_lower(mixed));
        assert_eq!(upper_ascii(mixed), "ABCDEFGH");
        // High-bit bytes must not look like 'a'–'z'.
        let hi = [0xE1, 0x61, 0x80, 0x7A, 0xFF, 0x41, 0x00, 0x20];
        let w = u64::from_le_bytes(hi);
        let mask = ascii_lower_mask(w);
        // Only 0x61 ('a') and 0x7A ('z') — little-endian lanes 1 and 3.
        assert_eq!(mask, 0x0000_0000_8000_8000);
    }
}
