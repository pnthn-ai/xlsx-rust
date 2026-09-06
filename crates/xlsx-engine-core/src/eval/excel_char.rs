//! Excel `CHAR(number)` — Windows ANSI (Windows-1252) code page, 1..=255.
//!
//! Desktop Excel on Western Windows maps `number` through the ANSI code page
//! (CP1252), not ISO-8859-1 / Latin-1 and not Unicode code points. That is
//! why `CHAR(128)` is `€` (U+20AC) rather than the C1 control U+0080, and
//! why `CHAR(129)` / `CHAR(141)` / `CHAR(143)` / `CHAR(144)` / `CHAR(157)`
//! are the leftover C1 bytes Microsoft documents as *not* stripped by
//! `CLEAN`.
//!
//! `number` is truncated toward zero (`CHAR(65.9)` → `"A"`). After truncate,
//! anything outside `1..=255` is `#VALUE!` (`0`, negatives, `256`, `1E+20`).
//! Non-finite values are `#VALUE!`. Callers coerce with arithmetic
//! `to_number` (`TRUE` → 1, blank / `FALSE` → 0 → `#VALUE!`, `"65"` → 65).
//!
//! [`excel_char`] is the production path: range-check, then a static
//! `&'static str` lookup (no per-call UTF-8 encode). [`excel_char_naive`]
//! remaps CP1252 through a `match` and `char::to_string` so
//! `cargo bench -p xlsx-engine-core --bench excel_char` can print
//! before/after.

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use std::sync::OnceLock;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

/// Production `CHAR` kernel: truncated code → Windows-1252 UTF-8.
pub fn excel_char(n: f64) -> Result<&'static str, ExcelError> {
    let code = trunc_code(n)?;
    Ok(utf8_table()[code as usize])
}

/// Like [`excel_char`], but always allocates a new `String`.
pub fn excel_char_owned(n: f64) -> Result<String, ExcelError> {
    excel_char(n).map(str::to_owned)
}

/// Baseline: CP1252 `match` + `char::to_string` every call.
///
/// Same Excel semantics as [`excel_char`]. Kept so
/// `cargo bench -p xlsx-engine-core --bench excel_char` can print before/after.
pub fn excel_char_naive(n: f64) -> Result<String, ExcelError> {
    let code = trunc_code_naive(n)?;
    let ch = char::from_u32(w1252_scalar(code)).expect("Windows-1252 maps to a scalar");
    Ok(ch.to_string())
}

/// Truncate toward zero and reject anything outside `1..=255`.
pub fn trunc_code(n: f64) -> Result<u8, ExcelError> {
    if !n.is_finite() {
        return Err(ExcelError::Value);
    }
    // Whole numbers in range skip a second `trunc` (the common CHAR(65) path).
    if n >= 1.0 && n <= 255.0 && n == n.trunc() {
        return Ok(n as u8);
    }
    let t = n.trunc();
    if !(1.0..=255.0).contains(&t) {
        return Err(ExcelError::Value);
    }
    Ok(t as u8)
}

fn trunc_code_naive(n: f64) -> Result<u8, ExcelError> {
    if !n.is_finite() {
        return Err(ExcelError::Value);
    }
    let t = n.trunc();
    if t < 1.0 || t > 255.0 {
        return Err(ExcelError::Value);
    }
    Ok(t as u8)
}

/// Windows-1252 byte → Unicode scalar.
///
/// Bytes `0x00..=0x7F` and `0xA0..=0xFF` match Latin-1. The `0x80..=0x9F`
/// C1 window is the CP1252 printables / leftover controls (not ISO-8859-1).
#[inline]
pub const fn w1252_scalar(code: u8) -> u32 {
    match code {
        0x80 => 0x20AC, // €
        0x81 => 0x0081,
        0x82 => 0x201A, // ‚
        0x83 => 0x0192, // ƒ
        0x84 => 0x201E, // „
        0x85 => 0x2026, // …
        0x86 => 0x2020, // †
        0x87 => 0x2021, // ‡
        0x88 => 0x02C6, // ˆ
        0x89 => 0x2030, // ‰
        0x8A => 0x0160, // Š
        0x8B => 0x2039, // ‹
        0x8C => 0x0152, // Œ
        0x8D => 0x008D,
        0x8E => 0x017D, // Ž
        0x8F => 0x008F,
        0x90 => 0x0090,
        0x91 => 0x2018, // ‘
        0x92 => 0x2019, // ’
        0x93 => 0x201C, // “
        0x94 => 0x201D, // ”
        0x95 => 0x2022, // •
        0x96 => 0x2013, // –
        0x97 => 0x2014, // —
        0x98 => 0x02DC, // ˜
        0x99 => 0x2122, // ™
        0x9A => 0x0161, // š
        0x9B => 0x203A, // ›
        0x9C => 0x0153, // œ
        0x9D => 0x009D,
        0x9E => 0x017E, // ž
        0x9F => 0x0178, // Ÿ
        c => c as u32,
    }
}

fn utf8_table() -> &'static [&'static str; 256] {
    static TABLE: OnceLock<[&'static str; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut slots = [""; 256];
        let mut i = 1u16;
        while i <= 255 {
            let code = i as u8;
            let ch = char::from_u32(w1252_scalar(code)).expect("Windows-1252 maps to a scalar");
            slots[code as usize] = Box::leak(ch.to_string().into_boxed_str());
            i += 1;
        }
        slots
    })
}

/// Production CHAR (scalar arg, arithmetic coerce, implicit intersection).
pub(crate) fn fn_char(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() != 1 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    match coerce::to_number(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(n) => match excel_char(n) {
            Ok(s) => Ok(ExcelValue::Text(s.to_owned())),
            Err(e) => Ok(ExcelValue::Error(e)),
        },
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn both(n: f64) -> Result<String, ExcelError> {
        let fast = excel_char(n).map(str::to_owned);
        let slow = excel_char_naive(n);
        assert_eq!(fast, slow, "fast/naive mismatch for CHAR({n})");
        fast
    }

    #[test]
    fn microsoft_examples() {
        assert_eq!(both(65.0).unwrap(), "A");
        assert_eq!(both(32.0).unwrap(), " ");
        assert_eq!(both(51.0).unwrap(), "3");
        assert_eq!(both(33.0).unwrap(), "!");
    }

    #[test]
    fn ascii_printables_and_controls() {
        assert_eq!(both(1.0).unwrap(), "\u{0001}");
        assert_eq!(both(7.0).unwrap(), "\u{0007}");
        assert_eq!(both(9.0).unwrap(), "\t");
        assert_eq!(both(10.0).unwrap(), "\n");
        assert_eq!(both(13.0).unwrap(), "\r");
        assert_eq!(both(31.0).unwrap(), "\u{001f}");
        assert_eq!(both(34.0).unwrap(), "\"");
        assert_eq!(both(48.0).unwrap(), "0");
        assert_eq!(both(90.0).unwrap(), "Z");
        assert_eq!(both(97.0).unwrap(), "a");
        assert_eq!(both(127.0).unwrap(), "\u{007f}");
    }

    #[test]
    fn windows1252_not_latin1() {
        assert_eq!(both(128.0).unwrap(), "€");
        assert_eq!(both(129.0).unwrap(), "\u{0081}");
        assert_eq!(both(130.0).unwrap(), "\u{201a}");
        assert_eq!(both(131.0).unwrap(), "\u{0192}");
        assert_eq!(both(146.0).unwrap(), "\u{2019}");
        assert_eq!(both(147.0).unwrap(), "\u{201c}");
        assert_eq!(both(149.0).unwrap(), "\u{2022}");
        assert_eq!(both(150.0).unwrap(), "\u{2013}");
        assert_eq!(both(151.0).unwrap(), "\u{2014}");
        assert_eq!(both(153.0).unwrap(), "\u{2122}");
        assert_eq!(both(160.0).unwrap(), "\u{00a0}");
        assert_eq!(both(163.0).unwrap(), "£");
        assert_eq!(both(169.0).unwrap(), "©");
        assert_eq!(both(174.0).unwrap(), "®");
        assert_eq!(both(255.0).unwrap(), "ÿ");
        // Leftovers Microsoft lists as not stripped by CLEAN.
        for n in [129u8, 141, 143, 144, 157] {
            assert_eq!(
                both(n as f64).unwrap(),
                char::from_u32(n as u32).unwrap().to_string(),
                "leftover CHAR({n})"
            );
        }
    }

    #[test]
    fn full_1_to_255_matches_naive() {
        for n in 1u16..=255 {
            both(n as f64).unwrap();
        }
    }

    #[test]
    fn domain_and_trunc() {
        assert_eq!(both(0.0), Err(ExcelError::Value));
        assert_eq!(both(-1.0), Err(ExcelError::Value));
        assert_eq!(both(-0.5), Err(ExcelError::Value));
        assert_eq!(both(0.9), Err(ExcelError::Value));
        assert_eq!(both(256.0), Err(ExcelError::Value));
        assert_eq!(both(1e20), Err(ExcelError::Value));
        assert_eq!(both(f64::NAN), Err(ExcelError::Value));
        assert_eq!(both(f64::INFINITY), Err(ExcelError::Value));
        assert_eq!(both(65.9).unwrap(), "A");
        assert_eq!(both(1.9).unwrap(), "\u{0001}");
        assert_eq!(both(255.9).unwrap(), "ÿ");
        assert_eq!(both(128.7).unwrap(), "€");
    }

    #[test]
    fn every_mapping_is_one_scalar() {
        for n in 1u16..=255 {
            assert_eq!(both(n as f64).unwrap().chars().count(), 1, "CHAR({n})");
        }
    }
}
