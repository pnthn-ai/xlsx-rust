//! Excel `REPT(text, number_times)`.
//!
//! Semantics (Microsoft Excel / Microsoft 365):
//! - Repeats `text` `number_times` times. `number_times` is truncated toward
//!   zero (`REPT("x", 3.9)` → `"xxx"`). After truncate, a negative count is
//!   `#VALUE!`. Zero (including `−0.9` → `0`) returns `""`.
//! - `text` coerces like `&` (`TRUE` → `"TRUE"`, `12` → `"12"`, blank → `""`).
//! - Empty `text` (literal `""` or a blank cell) returns `""` for any
//!   non-negative `number_times`, including values far above the result cap:
//!   `0 × n` never overflows.
//! - The result cannot exceed **32,767 UTF-16 code units** — Excel’s cell
//!   content limit, the same cap as `CONCAT` / `TEXTJOIN`. Microsoft’s REPT
//!   page says “32,767 characters”; that limit is the stored cell width, not
//!   this crate’s Compatibility Version 2 `LEN` (Unicode scalars). A
//!   supplementary-plane scalar (`😀`) is **two** UTF-16 units, so
//!   `REPT("😀", 16384)` is `#VALUE!` even though `LEN` of 16383 copies is
//!   16383. One copy of a string that is already longer than the cap is also
//!   `#VALUE!`.
//! - Overflow is rejected **before** allocation. `REPT("a", 1E+20)` is
//!   `#VALUE!` without building a giant string.
//! - Wrong arity is `#VALUE!`. Errors evaluate left-to-right (`text` first).
//!
//! [`rept`] is the production path (UTF-16 length check, then `str::repeat`
//! / single-byte ASCII fill). [`rept_naive`] is the unreserved `push_str`
//! loop kept so `cargo bench -p xlsx-engine-core --bench rept` can print
//! before/after.

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

/// Excel cell-content limit used by REPT (UTF-16 code units).
pub const REPT_MAX_CHARS: usize = 32767;

/// Production `REPT` kernel.
///
/// `times` is already truncated toward zero and rejected when negative /
/// non-finite. Empty `text` or `times == 0` is `""`.
pub fn rept(text: &str, times: u64) -> Result<String, ExcelError> {
    if times == 0 || text.is_empty() {
        return Ok(String::new());
    }
    let unit = utf16_len(text);
    if unit > REPT_MAX_CHARS {
        return Err(ExcelError::Value);
    }
    let max_times = (REPT_MAX_CHARS / unit) as u64;
    if times > max_times {
        return Err(ExcelError::Value);
    }
    let n = match usize::try_from(times) {
        Ok(n) => n,
        Err(_) => return Err(ExcelError::Value),
    };
    if text.len() == 1 && text.is_ascii() {
        return Ok(repeat_ascii_byte(text.as_bytes()[0], n));
    }
    Ok(text.repeat(n))
}

/// Unreserved `push_str` loop used only as the Instant-bench “before”.
///
/// Same Excel semantics as [`rept`], including the UTF-16 cap. Walks UTF-16
/// even for ASCII and does not pre-size the buffer.
pub fn rept_naive(text: &str, times: u64) -> Result<String, ExcelError> {
    if times == 0 || text.is_empty() {
        return Ok(String::new());
    }
    let unit = text.encode_utf16().count();
    if unit > REPT_MAX_CHARS {
        return Err(ExcelError::Value);
    }
    let max_times = (REPT_MAX_CHARS / unit) as u64;
    if times > max_times {
        return Err(ExcelError::Value);
    }
    let n = match usize::try_from(times) {
        Ok(n) => n,
        Err(_) => return Err(ExcelError::Value),
    };
    let mut out = String::new();
    for _ in 0..n {
        out.push_str(text);
    }
    Ok(out)
}

/// Production REPT (scalar args, overflow-checked kernel).
pub(crate) fn fn_rept(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() != 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let text = match coerce::to_text(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(s) => s,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let times = match coerce::to_number(&ev.eval_scalar(&args[1], ctx)?) {
        Ok(n) => match trunc_times(n) {
            Ok(t) => t,
            Err(e) => return Ok(ExcelValue::Error(e)),
        },
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    match rept(&text, times) {
        Ok(s) => Ok(ExcelValue::Text(s)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

/// Truncate `number_times` toward zero. Non-finite → `#VALUE!`. After
/// truncate, negative → `#VALUE!`; values above `u64::MAX` saturate so the
/// kernel’s cap check can reject them without wrapping.
pub fn trunc_times(n: f64) -> Result<u64, ExcelError> {
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

fn utf16_len(s: &str) -> usize {
    if s.is_ascii() {
        s.len()
    } else {
        s.encode_utf16().count()
    }
}

fn repeat_ascii_byte(b: u8, n: usize) -> String {
    debug_assert!(b.is_ascii());
    // A single ASCII byte is valid UTF-8; `vec![b; n]` is a memset.
    String::from_utf8(vec![b; n]).expect("ASCII byte is valid UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn both(text: &str, times: u64) -> Result<String, ExcelError> {
        let a = rept(text, times);
        let b = rept_naive(text, times);
        assert_eq!(a, b, "rept vs rept_naive mismatch for {text:?} × {times}");
        a
    }

    #[test]
    fn microsoft_examples() {
        assert_eq!(both("*-", 3).unwrap(), "*-*-*-");
        assert_eq!(both("-", 10).unwrap(), "----------");
    }

    #[test]
    fn zero_and_empty() {
        assert_eq!(both("a", 0).unwrap(), "");
        assert_eq!(both("", 5).unwrap(), "");
        assert_eq!(both("", 100_000).unwrap(), "");
        assert_eq!(both("", u64::MAX).unwrap(), "");
    }

    #[test]
    fn cap_ascii() {
        assert_eq!(both("a", 32767).unwrap().len(), 32767);
        assert_eq!(both("a", 32768), Err(ExcelError::Value));
        assert_eq!(both("ab", 16383).unwrap().len(), 32766);
        assert_eq!(both("ab", 16384), Err(ExcelError::Value));
        assert_eq!(both(&"x".repeat(32767), 1).unwrap().len(), 32767);
        assert_eq!(both(&"x".repeat(32768), 1), Err(ExcelError::Value));
    }

    #[test]
    fn cap_is_utf16_not_scalars() {
        // U+1F600 is one Unicode scalar (LEN = 1) and two UTF-16 units.
        assert_eq!(both("😀", 16383).unwrap().chars().count(), 16383);
        assert_eq!(both("😀", 16384), Err(ExcelError::Value));
        let almost = "x".repeat(32766);
        assert_eq!(both(&format!("{almost}😀"), 1), Err(ExcelError::Value));
    }

    #[test]
    fn trunc_times_rules() {
        assert_eq!(trunc_times(3.9).unwrap(), 3);
        assert_eq!(trunc_times(0.9).unwrap(), 0);
        assert_eq!(trunc_times(-0.9).unwrap(), 0);
        assert_eq!(trunc_times(-1.0), Err(ExcelError::Value));
        assert_eq!(trunc_times(f64::NAN), Err(ExcelError::Value));
        assert_eq!(trunc_times(f64::INFINITY), Err(ExcelError::Value));
        assert_eq!(trunc_times(1e20).unwrap(), u64::MAX);
        assert_eq!(
            rept("a", trunc_times(1e20).unwrap()),
            Err(ExcelError::Value)
        );
    }

    #[test]
    fn unicode_copies() {
        assert_eq!(both("é", 3).unwrap(), "ééé");
        assert_eq!(both("日本語", 2).unwrap(), "日本語日本語");
    }
}
