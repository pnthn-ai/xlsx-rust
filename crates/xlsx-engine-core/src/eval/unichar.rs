//! Excel `UNICHAR(number)` — Unicode scalar from a numeric code point.
//!
//! Semantics (Microsoft Excel / Microsoft 365):
//! - `number` is truncated toward zero (`UNICHAR(66.9)` → `"B"`).
//! - Valid scalars are **1 ..= 1_114_111** (`0x10FFFF`). `0`, negatives,
//!   non-finite values, and anything above the Unicode max are `#VALUE!`.
//! - UTF-16 **surrogates** `0xD800 ..= 0xDFFF` are not Unicode scalars.
//!   Microsoft documents those (and other invalid Unicode data) as `#N/A`,
//!   not `#VALUE!`.
//! - The result is a one-scalar UTF-8 string. Supplementary-plane scalars
//!   (`😀` = 128512) are one character here (Compatibility Version 2 `LEN`
//!   is 1) even though Excel stores them as a UTF-16 surrogate pair.
//! - Argument coercion is the arithmetic kernel: blank / FALSE → 0 →
//!   `#VALUE!`, TRUE → 1, numeric text parses (`"66"` → `"B"`).
//!
//! [`unichar`] writes UTF-8 directly after the range check.
//! [`unichar_naive`] builds a `Vec<u16>` then `String::from_utf16` — the
//! same UTF-16 round-trip Excel uses internally — so
//! `cargo bench -p xlsx-engine-core --bench unichar` can print before/after.

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

/// First UTF-16 high-surrogate code unit.
const SURROGATE_START: u32 = 0xD800;
/// Last UTF-16 low-surrogate code unit.
const SURROGATE_END: u32 = 0xDFFF;
/// Unicode max scalar (`U+10FFFF`).
const UNICODE_MAX: u32 = 0x10FFFF;

/// Production `UNICHAR` kernel: truncate, classify, encode UTF-8.
pub fn unichar(n: f64) -> Result<String, ExcelError> {
    let cp = codepoint(n)?;
    Ok(encode_utf8_scalar(cp))
}

/// UTF-16 collect + `from_utf16` baseline used only as the Instant-bench
/// “before”. Same Excel errors as [`unichar`].
pub fn unichar_naive(n: f64) -> Result<String, ExcelError> {
    let cp = codepoint(n)?;
    let c = char::from_u32(cp).ok_or(ExcelError::Na)?;
    // Display → UTF-8, then collect UTF-16 and decode. Same errors as
    // [`unichar`]; kept only so the Instant bench has a “before”.
    let s = c.to_string();
    let units: Vec<u16> = s.encode_utf16().collect();
    String::from_utf16(&units).map_err(|_| ExcelError::Na)
}

/// Truncate `number` toward zero and accept a Unicode scalar.
///
/// `#VALUE!` for non-finite / `< 1` / `> 0x10FFFF`. `#N/A` for surrogates.
pub fn codepoint(n: f64) -> Result<u32, ExcelError> {
    if !n.is_finite() {
        return Err(ExcelError::Value);
    }
    let t = n.trunc();
    if t < 1.0 || t > UNICODE_MAX as f64 {
        return Err(ExcelError::Value);
    }
    let cp = t as u32;
    if (SURROGATE_START..=SURROGATE_END).contains(&cp) {
        return Err(ExcelError::Na);
    }
    Ok(cp)
}

/// Encode a known-valid scalar (`1..=0x10FFFF`, not a surrogate) as UTF-8.
///
/// Length is known from the code point, so this is a 1–4 byte write plus
/// one small `String` alloc — no UTF-16, no `char::to_string` formatter.
fn encode_utf8_scalar(cp: u32) -> String {
    debug_assert!(
        (1..=UNICODE_MAX).contains(&cp) && !(SURROGATE_START..=SURROGATE_END).contains(&cp)
    );
    let mut buf = [0u8; 4];
    let n = if cp < 0x80 {
        buf[0] = cp as u8;
        1
    } else if cp < 0x800 {
        buf[0] = 0xC0 | (cp >> 6) as u8;
        buf[1] = 0x80 | (cp & 0x3F) as u8;
        2
    } else if cp < 0x10000 {
        buf[0] = 0xE0 | (cp >> 12) as u8;
        buf[1] = 0x80 | ((cp >> 6) & 0x3F) as u8;
        buf[2] = 0x80 | (cp & 0x3F) as u8;
        3
    } else {
        buf[0] = 0xF0 | (cp >> 18) as u8;
        buf[1] = 0x80 | ((cp >> 12) & 0x3F) as u8;
        buf[2] = 0x80 | ((cp >> 6) & 0x3F) as u8;
        buf[3] = 0x80 | (cp & 0x3F) as u8;
        4
    };
    // SAFETY: the branches write well-formed UTF-8 for a Unicode scalar.
    let mut s = String::with_capacity(n);
    s.push_str(unsafe { std::str::from_utf8_unchecked(&buf[..n]) });
    s
}

/// Production UNICHAR (one coerced number → one-scalar text or error).
pub(crate) fn fn_unichar(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() != 1 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    match coerce::to_number(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(n) => match unichar(n) {
            Ok(s) => Ok(ExcelValue::Text(s)),
            Err(e) => Ok(ExcelValue::Error(e)),
        },
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn both(n: f64) -> Result<String, ExcelError> {
        let a = unichar(n);
        let b = unichar_naive(n);
        assert_eq!(a, b, "unichar vs unichar_naive mismatch for {n}");
        a
    }

    #[test]
    fn microsoft_examples() {
        assert_eq!(both(66.0).unwrap(), "B");
        assert_eq!(both(32.0).unwrap(), " ");
        assert_eq!(both(0.0), Err(ExcelError::Value));
    }

    #[test]
    fn ascii_and_c0() {
        assert_eq!(both(1.0).unwrap(), "\u{1}");
        assert_eq!(both(9.0).unwrap(), "\t");
        assert_eq!(both(10.0).unwrap(), "\n");
        assert_eq!(both(65.0).unwrap(), "A");
        assert_eq!(both(97.0).unwrap(), "a");
        assert_eq!(both(127.0).unwrap(), "\u{7f}");
    }

    #[test]
    fn bmp_and_supplementary() {
        assert_eq!(both(169.0).unwrap(), "©");
        assert_eq!(both(233.0).unwrap(), "é");
        assert_eq!(both(0x4E2D as f64).unwrap(), "中");
        assert_eq!(both(128512.0).unwrap(), "😀");
        assert_eq!(both(65536.0).unwrap().chars().count(), 1);
        assert_eq!(both(UNICODE_MAX as f64).unwrap(), "\u{10FFFF}");
    }

    #[test]
    fn surrogate_is_na() {
        assert_eq!(both(0xD800 as f64), Err(ExcelError::Na));
        assert_eq!(both(0xDFFF as f64), Err(ExcelError::Na));
        assert_eq!(both(0xD7FF as f64).unwrap(), "\u{D7FF}");
        assert_eq!(both(0xE000 as f64).unwrap(), "\u{E000}");
    }

    #[test]
    fn range_and_trunc() {
        assert_eq!(both(66.9).unwrap(), "B");
        assert_eq!(both(0.9), Err(ExcelError::Value));
        assert_eq!(both(-0.5), Err(ExcelError::Value));
        assert_eq!(both(-1.0), Err(ExcelError::Value));
        assert_eq!(both((UNICODE_MAX + 1) as f64), Err(ExcelError::Value));
        assert_eq!(both(f64::NAN), Err(ExcelError::Value));
        assert_eq!(both(f64::INFINITY), Err(ExcelError::Value));
        assert_eq!(both(1e20), Err(ExcelError::Value));
    }

    #[test]
    fn encode_matches_char() {
        for cp in [
            1u32,
            0x7F,
            0x80,
            0x7FF,
            0x800,
            0xD7FF,
            0xE000,
            0xFFFF,
            0x10000,
            0x1F600,
            UNICODE_MAX,
        ] {
            let got = encode_utf8_scalar(cp);
            let expect = char::from_u32(cp).unwrap().to_string();
            assert_eq!(got, expect, "U+{cp:04X}");
        }
    }
}
