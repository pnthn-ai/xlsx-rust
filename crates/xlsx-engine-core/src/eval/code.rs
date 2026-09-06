//! Excel `CODE` kernel.
//!
//! Semantics (desktop Excel / Microsoft docs, Windows en-US):
//! - `CODE(text)` returns the **Windows-1252** numeric code of the first
//!   Unicode scalar after `&`-style text coercion.
//! - Empty text / blank cell → `#VALUE!`.
//! - ASCII `0..=127` (including C0 and DEL) are identity.
//! - Latin-1 `U+00A0..=U+00FF` map to `160..=255`.
//! - The `0x80..=0x9F` Windows-1252 specials (`€` → 128, `…` → 133, …)
//!   plus the unused C1 leftovers `129` / `141` / `143` / `144` / `157`
//!   (same set CLEAN documents as not-stripped) map to their CP1252 byte.
//! - Any other scalar (CJK, emoji, most of BMP) is `#VALUE!`. That is
//!   `CODE`, not `UNICODE`.
//! - First-character only: `CODE("ABC")` is `65`. Combining marks after
//!   a base letter are ignored (`"e\u{0301}"` is `101`).
//!
//! Production path inspects the first UTF-8 sequence only (ASCII byte,
//! Latin-1 `C2`/`C3`, then a first-scalar CP1252 table). The `Vec<char>`
//! baseline lives beside it so benches can print before/after.

use super::coerce;
use xlsx_types::{ExcelError, ExcelValue};

/// Production `CODE` kernel on already-coerced text.
pub fn code(text: &str) -> Result<f64, ExcelError> {
    code_fast(text)
}

/// `Vec<char>` baseline used for the hill-climb bench.
///
/// Same Excel semantics as [`code`]. Kept so
/// `cargo bench -p xlsx-engine-core --bench code` can print before/after.
pub fn code_naive(text: &str) -> Result<f64, ExcelError> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Err(ExcelError::Value);
    }
    match cp1252_byte(chars[0]) {
        Some(n) => Ok(n as f64),
        None => Err(ExcelError::Value),
    }
}

/// Production path on an evaluated Excel value (no full-string clone).
pub fn code_value(v: &ExcelValue) -> Result<f64, ExcelError> {
    match v {
        ExcelValue::Error(e) => Err(*e),
        ExcelValue::Empty => Err(ExcelError::Value),
        ExcelValue::Text(s) => code(s),
        ExcelValue::Bool(true) => Ok(84.0),  // "TRUE"
        ExcelValue::Bool(false) => Ok(70.0), // "FALSE"
        ExcelValue::Number(n) => code_number(*n),
        ExcelValue::Array(_) => Err(ExcelError::Value),
    }
}

/// Naive value path: `to_text` + `Vec<char>` (bench baseline).
pub fn code_value_naive(v: &ExcelValue) -> Result<f64, ExcelError> {
    let s = coerce::to_text(v)?;
    code_naive(&s)
}

fn code_fast(text: &str) -> Result<f64, ExcelError> {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return Err(ExcelError::Value);
    }
    let b0 = bytes[0];
    if b0 < 0x80 {
        return Ok(b0 as f64);
    }
    // Latin-1 block in UTF-8: `C2 A0..=BF` → U+00A0..=U+00BF;
    // `C3 80..=BF` → U+00C0..=U+00FF. Unused C1 leftovers are `C2 81/8D/8F/90/9D`.
    if b0 == 0xC2 && bytes.len() >= 2 {
        let b1 = bytes[1];
        if b1 >= 0xA0 {
            return Ok(b1 as f64);
        }
        if matches!(b1, 0x81 | 0x8D | 0x8F | 0x90 | 0x9D) {
            return Ok(b1 as f64);
        }
        return Err(ExcelError::Value);
    }
    if b0 == 0xC3 && bytes.len() >= 2 && bytes[1] >= 0x80 {
        return Ok((0xC0 + (bytes[1] - 0x80)) as f64);
    }
    let ch = match text.chars().next() {
        Some(c) => c,
        None => return Err(ExcelError::Value),
    };
    match cp1252_byte(ch) {
        Some(n) => Ok(n as f64),
        None => Err(ExcelError::Value),
    }
}

/// First character of [`coerce::format_plain`] without allocating the
/// common integer / sign cases.
fn code_number(n: f64) -> Result<f64, ExcelError> {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        if n < 0.0 || n.to_bits() == (-0.0_f64).to_bits() {
            return Ok(45.0); // '-'
        }
        return Ok(leading_digit_ascii(n as i64) as f64);
    }
    code(&coerce::format_plain(n))
}

fn leading_digit_ascii(n: i64) -> u8 {
    debug_assert!(n >= 0);
    if n == 0 {
        return b'0';
    }
    let mut v = n as u64;
    while v >= 10 {
        v /= 10;
    }
    b'0' + v as u8
}

/// Windows-1252 byte for a Unicode scalar, or `None` if not representable.
fn cp1252_byte(c: char) -> Option<u8> {
    let u = c as u32;
    if u <= 0x7F {
        return Some(u as u8);
    }
    if (0xA0..=0xFF).contains(&u) {
        return Some(u as u8);
    }
    Some(match c {
        '€' => 0x80,
        '\u{0081}' => 0x81,
        '‚' => 0x82,
        'ƒ' => 0x83,
        '„' => 0x84,
        '…' => 0x85,
        '†' => 0x86,
        '‡' => 0x87,
        'ˆ' => 0x88,
        '‰' => 0x89,
        'Š' => 0x8A,
        '‹' => 0x8B,
        'Œ' => 0x8C,
        '\u{008D}' => 0x8D,
        'Ž' => 0x8E,
        '\u{008F}' => 0x8F,
        '\u{0090}' => 0x90,
        '‘' => 0x91,
        '’' => 0x92,
        '“' => 0x93,
        '”' => 0x94,
        '•' => 0x95,
        '–' => 0x96,
        '—' => 0x97,
        '˜' => 0x98,
        '™' => 0x99,
        'š' => 0x9A,
        '›' => 0x9B,
        'œ' => 0x9C,
        '\u{009D}' => 0x9D,
        'ž' => 0x9E,
        'Ÿ' => 0x9F,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::eval_formula_in;
    use xlsx_types::{Cell, ExcelError, ExcelValue, Sheet, Workbook};

    fn both(s: &str) -> Result<f64, ExcelError> {
        let fast = code(s);
        let slow = code_naive(s);
        assert_eq!(fast, slow, "naive/fast mismatch for {s:?}");
        fast
    }

    fn both_v(v: &ExcelValue) -> Result<f64, ExcelError> {
        let fast = code_value(v);
        let slow = code_value_naive(v);
        assert_eq!(fast, slow, "naive/fast value mismatch for {v:?}");
        fast
    }

    #[test]
    fn microsoft_examples() {
        assert_eq!(both("A"), Ok(65.0));
        assert_eq!(both("a"), Ok(97.0));
    }

    #[test]
    fn first_character_only() {
        assert_eq!(both("ABC"), Ok(65.0));
        assert_eq!(both("xyz"), Ok(120.0));
        assert_eq!(both("!hello"), Ok(33.0));
        assert_eq!(both("  leading space"), Ok(32.0));
    }

    #[test]
    fn empty_is_value() {
        assert_eq!(both(""), Err(ExcelError::Value));
        assert_eq!(both_v(&ExcelValue::Empty), Err(ExcelError::Value));
        assert_eq!(
            both_v(&ExcelValue::Text(String::new())),
            Err(ExcelError::Value)
        );
    }

    #[test]
    fn ascii_controls_and_del() {
        assert_eq!(both("\u{0001}"), Ok(1.0));
        assert_eq!(both("\u{0007}"), Ok(7.0));
        assert_eq!(both("\t"), Ok(9.0));
        assert_eq!(both("\n"), Ok(10.0));
        assert_eq!(both("\r"), Ok(13.0));
        assert_eq!(both("\u{001f}"), Ok(31.0));
        assert_eq!(both(" "), Ok(32.0));
        assert_eq!(both("\u{007f}"), Ok(127.0));
    }

    #[test]
    fn latin1_and_windows1252() {
        assert_eq!(both("é"), Ok(233.0));
        assert_eq!(both("É"), Ok(201.0));
        assert_eq!(both("ñ"), Ok(241.0));
        assert_eq!(both("ü"), Ok(252.0));
        assert_eq!(both("\u{00a0}"), Ok(160.0)); // NBSP
        assert_eq!(both("€"), Ok(128.0));
        assert_eq!(both("…"), Ok(133.0));
        assert_eq!(both("’"), Ok(146.0));
        assert_eq!(both("—"), Ok(151.0));
        assert_eq!(both("™"), Ok(153.0));
        assert_eq!(both("Œ"), Ok(140.0));
        assert_eq!(both("Ÿ"), Ok(159.0));
        assert_eq!(both("\u{0081}"), Ok(129.0));
        assert_eq!(both("\u{008d}"), Ok(141.0));
        assert_eq!(both("\u{008f}"), Ok(143.0));
        assert_eq!(both("\u{0090}"), Ok(144.0));
        assert_eq!(both("\u{009d}"), Ok(157.0));
    }

    #[test]
    fn outside_code_page_is_value() {
        assert_eq!(both("中"), Err(ExcelError::Value));
        assert_eq!(both("日本語"), Err(ExcelError::Value));
        assert_eq!(both("😀"), Err(ExcelError::Value));
        assert_eq!(both("α"), Err(ExcelError::Value));
        assert_eq!(both("\u{200b}"), Err(ExcelError::Value)); // ZWSP
    }

    #[test]
    fn combining_mark_uses_first_scalar() {
        // Precomposed é is one scalar (233). Decomposed e + combining acute
        // is first-scalar 'e' (101), matching LEN/MID Compat v2.
        assert_eq!(both("é"), Ok(233.0));
        assert_eq!(both("e\u{0301}"), Ok(101.0));
    }

    #[test]
    fn number_and_bool_coercion() {
        assert_eq!(both_v(&ExcelValue::Number(65.0)), Ok(54.0)); // "65"
        assert_eq!(both_v(&ExcelValue::Number(123.0)), Ok(49.0)); // "123"
        assert_eq!(both_v(&ExcelValue::Number(0.0)), Ok(48.0));
        assert_eq!(both_v(&ExcelValue::Number(-2.0)), Ok(45.0));
        assert_eq!(both_v(&ExcelValue::Number(-0.0)), Ok(45.0));
        assert_eq!(both_v(&ExcelValue::Number(1.5)), Ok(49.0)); // "1.5"
        assert_eq!(both_v(&ExcelValue::Bool(true)), Ok(84.0));
        assert_eq!(both_v(&ExcelValue::Bool(false)), Ok(70.0));
        assert_eq!(
            both_v(&ExcelValue::Error(ExcelError::Div0)),
            Err(ExcelError::Div0)
        );
        assert_eq!(both_v(&ExcelValue::Array(vec![])), Err(ExcelError::Value));
    }

    #[test]
    fn leading_digit_matches_format_plain() {
        for n in [
            0i64,
            1,
            9,
            10,
            42,
            65,
            99,
            100,
            123456789,
            999_999_999_999_999,
        ] {
            let s = coerce::format_plain(n as f64);
            let expected = s.as_bytes()[0] as f64;
            assert_eq!(code_number(n as f64).unwrap(), expected, "n={n}");
        }
        assert_eq!(code_number(-0.0).unwrap(), 45.0);
        assert_eq!(code_number(-12.0).unwrap(), 45.0);
    }

    #[test]
    fn long_string_is_first_byte_only() {
        let long = format!("Z{}", "x".repeat(4096));
        assert_eq!(both(&long), Ok(90.0));
        let cafe = format!("é{}", "x".repeat(4096));
        assert_eq!(both(&cafe), Ok(233.0));
        let euro = format!("€{}", "x".repeat(1024));
        assert_eq!(both(&euro), Ok(128.0));
        let cjk = format!("中{}", "x".repeat(1024));
        assert_eq!(both(&cjk), Err(ExcelError::Value));
    }

    #[test]
    fn formula_coercion_and_arity() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=CODE(\"A\")").unwrap(),
            ExcelValue::Number(65.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=CODE(\"ABC\")").unwrap(),
            ExcelValue::Number(65.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=CODE(\"\")").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=CODE(65)").unwrap(),
            ExcelValue::Number(54.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=CODE(TRUE)").unwrap(),
            ExcelValue::Number(84.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=CODE(FALSE)").unwrap(),
            ExcelValue::Number(70.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=CODE()").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=CODE(\"A\",\"B\")").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=CODE(1/0)").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=CODE(NA())").unwrap(),
            ExcelValue::Error(ExcelError::Na)
        );
        assert_eq!(
            eval_formula_in(&wb, "=CODE(UPPER(\"a\"))").unwrap(),
            ExcelValue::Number(65.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=CODE(LEFT(\"xyz\"))").unwrap(),
            ExcelValue::Number(120.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=CODE(\"é\")").unwrap(),
            ExcelValue::Number(233.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=CODE(\"€\")").unwrap(),
            ExcelValue::Number(128.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=CODE(\"中\")").unwrap(),
            ExcelValue::Error(ExcelError::Value)
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
            .insert("A3".into(), Cell::value(ExcelValue::Text(String::new())));
        sheet
            .cells
            .insert("A4".into(), Cell::value(ExcelValue::Text("€uro".into())));
        let wb = Workbook {
            sheets: vec![sheet],
            names: vec![],
        };
        assert_eq!(
            eval_formula_in(&wb, "=CODE(A1)").unwrap(),
            ExcelValue::Number(72.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=CODE(A2)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=CODE(A3)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=CODE(B1)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=CODE(A4)").unwrap(),
            ExcelValue::Number(128.0)
        );
    }

    #[test]
    fn cp1252_roundtrip_defined_specials() {
        let pairs: &[(char, u8)] = &[
            ('€', 128),
            ('‚', 130),
            ('ƒ', 131),
            ('„', 132),
            ('…', 133),
            ('†', 134),
            ('‡', 135),
            ('ˆ', 136),
            ('‰', 137),
            ('Š', 138),
            ('‹', 139),
            ('Œ', 140),
            ('Ž', 142),
            ('‘', 145),
            ('’', 146),
            ('“', 147),
            ('”', 148),
            ('•', 149),
            ('–', 150),
            ('—', 151),
            ('˜', 152),
            ('™', 153),
            ('š', 154),
            ('›', 155),
            ('œ', 156),
            ('ž', 158),
            ('Ÿ', 159),
        ];
        for &(ch, byte) in pairs {
            let s = ch.to_string();
            assert_eq!(both(&s), Ok(byte as f64), "CODE({ch:?})");
        }
    }
}
