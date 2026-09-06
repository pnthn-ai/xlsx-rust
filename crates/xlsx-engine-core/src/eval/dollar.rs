//! Excel `DOLLAR(number, [decimals])` — en-US currency text.
//!
//! Shared kernel so `calc-core` and `seed-compliant` stay aligned. Does **not**
//! read fixture goldens.
//!
//! Desktop Excel / Microsoft DOLLAR help (en-US):
//! - Converts a number to **text** using currency format. Microsoft's
//!   documented skeleton is `$#,##0.00_);($#,##0.00)`. The `_` pad is **not**
//!   emitted: positives have no trailing space (`DOLLAR(1234.567, 2)` is
//!   `"$1,234.57"`).
//! - Omitted `decimals` (including a trailing-comma slot) defaults to **2**.
//!   Negative `decimals` rounds to the left of the decimal
//!   (`DOLLAR(1234.567, -2)` is `"$1,200"`).
//! - Rounding is Excel `ROUND` (half **away from zero**). The sign used for
//!   display is taken from the **rounded** value (`DOLLAR(-0.001, 2)` is
//!   `"$0.00"`, not parentheses).
//! - Negatives use accounting parentheses with `$` inside
//!   (`DOLLAR(-1234.567)` is `"($1,234.57)"`). That is **not** `TEXT` with
//!   `"$#,##0.00"`, which emits `-$1,234.57`.
//! - Thousands grouping (`,`) and `.` decimal. Currency symbol is `$`
//!   (en-US). Other locales are not implemented.
//! - Arithmetic coerce: empty → `0`, `TRUE` → `1`, `FALSE` → `0`, numeric
//!   text parsed; `"$5"` / `"1,000"` / other text → `#VALUE!` (not `VALUE`).
//! - Errors evaluate left-to-right (number, then decimals). Wrong arity
//!   (`DOLLAR()` / three+ args) is `#VALUE!`. Non-finite after coerce / round
//!   is `#NUM!`.
//!
//! Production writes digits into a stack buffer (specialised `decimals = 2`
//! cents path). The naive path uses `excel_round_naive` plus `format!` and
//! an allocating comma walk so benches can print before/after.

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{excel_round, excel_round_naive, EvalError, ExcelError, ExcelValue};

/// Exact `10^e` for `e` in `0..=20` (all representable as f64 integers).
const POW10: [f64; 21] = [
    1.0,
    10.0,
    100.0,
    1_000.0,
    10_000.0,
    100_000.0,
    1_000_000.0,
    10_000_000.0,
    100_000_000.0,
    1_000_000_000.0,
    10_000_000_000.0,
    100_000_000_000.0,
    1_000_000_000_000.0,
    10_000_000_000_000.0,
    100_000_000_000_000.0,
    1_000_000_000_000_000.0,
    10_000_000_000_000_000.0,
    100_000_000_000_000_000.0,
    1_000_000_000_000_000_000.0,
    10_000_000_000_000_000_000.0,
    100_000_000_000_000_000_000.0,
];

/// Format `decimals` is clamped to this (f64 has ~15 significant digits).
const MAX_PLACES: i32 = 20;

/// Production `DOLLAR` on an already-coerced number.
#[inline]
pub fn dollar(n: f64, decimals: i32) -> Result<String, ExcelError> {
    format_currency(n, decimals, excel_round, emit)
}

/// Allocating `format!` + comma-walk baseline used for the hill-climb bench.
///
/// Same Excel result as [`dollar`] on the documented cases. Kept so
/// `cargo bench -p xlsx-engine-core --bench dollar` can print before/after.
pub fn dollar_naive(n: f64, decimals: i32) -> Result<String, ExcelError> {
    format_currency(n, decimals, excel_round_naive, emit_naive)
}

/// Production `DOLLAR` on scalar Excel values. `decimals = None` is the
/// omitted-arg default (2).
pub fn dollar_value(number: &ExcelValue, decimals: Option<&ExcelValue>) -> ExcelValue {
    let n = match number {
        ExcelValue::Number(n) => *n,
        ExcelValue::Empty => 0.0,
        ExcelValue::Bool(true) => 1.0,
        ExcelValue::Bool(false) => 0.0,
        ExcelValue::Text(s) => match coerce::parse_numeric_text(s) {
            Ok(n) => n,
            Err(e) => return ExcelValue::Error(e),
        },
        ExcelValue::Error(e) => return ExcelValue::Error(*e),
        ExcelValue::Array(_) => return ExcelValue::Error(ExcelError::Value),
    };
    let d = match decimals {
        None => 2,
        Some(ExcelValue::Number(x)) => trunc_decimals(*x),
        Some(other) => match coerce::to_number(other) {
            Ok(x) => trunc_decimals(x),
            Err(e) => return ExcelValue::Error(e),
        },
    };
    match dollar(n, d) {
        Ok(s) => ExcelValue::Text(s),
        Err(e) => ExcelValue::Error(e),
    }
}

/// Value-level baseline: full [`coerce::to_number`] + [`dollar_naive`].
pub fn dollar_value_naive(number: &ExcelValue, decimals: Option<&ExcelValue>) -> ExcelValue {
    let n = match coerce::to_number(number) {
        Ok(n) => n,
        Err(e) => return ExcelValue::Error(e),
    };
    let d = match decimals {
        None => 2,
        Some(v) => match coerce::to_number(v) {
            Ok(x) => trunc_decimals(x),
            Err(e) => return ExcelValue::Error(e),
        },
    };
    match dollar_naive(n, d) {
        Ok(s) => ExcelValue::Text(s),
        Err(e) => ExcelValue::Error(e),
    }
}

/// Packed walk. Used by the kernel bench (and MAP-like callers).
pub fn dollar_slice(src: &[f64], decimals: i32, dst: &mut [String]) {
    let n = src.len().min(dst.len());
    for i in 0..n {
        dst[i] = dollar(src[i], decimals).unwrap_or_default();
    }
}

/// Allocating packed walk (bench baseline).
pub fn dollar_slice_naive(src: &[f64], decimals: i32, dst: &mut [String]) {
    let n = src.len().min(dst.len());
    for i in 0..n {
        dst[i] = dollar_naive(src[i], decimals).unwrap_or_default();
    }
}

/// `DOLLAR(number, [decimals])` — scalar context, wrong arity → `#VALUE!`.
pub(crate) fn fn_dollar(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.is_empty() || args.len() > 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let number = ev.eval_scalar(&args[0], ctx)?;
    let decimals = if args.len() >= 2 {
        match &args[1] {
            Expr::Missing => None,
            other => Some(ev.eval_scalar(other, ctx)?),
        }
    } else {
        None
    };
    Ok(dollar_value(&number, decimals.as_ref()))
}

#[inline]
fn trunc_decimals(n: f64) -> i32 {
    if !n.is_finite() {
        return 2;
    }
    let t = n.trunc();
    if t > i32::MAX as f64 {
        i32::MAX
    } else if t < i32::MIN as f64 {
        i32::MIN
    } else {
        t as i32
    }
}

fn format_currency(
    n: f64,
    decimals: i32,
    round: fn(f64, i32) -> f64,
    emit_fn: fn(f64, i32) -> String,
) -> Result<String, ExcelError> {
    if !n.is_finite() {
        return Err(ExcelError::Num);
    }
    let digits = decimals.clamp(-MAX_PLACES, MAX_PLACES);
    let r = round(n, digits);
    if !r.is_finite() {
        return Err(ExcelError::Num);
    }
    Ok(emit_fn(r, digits))
}

fn emit(rounded: f64, decimals: i32) -> String {
    let neg = rounded < 0.0;
    let mag = rounded.abs();
    if decimals == 2 {
        emit_places(neg, mag, 2)
    } else if decimals > 0 {
        emit_places(neg, mag, decimals as u32)
    } else {
        emit_places(neg, mag, 0)
    }
}

/// Fast path: already-rounded magnitude → integer + optional fraction.
fn emit_places(neg: bool, mag: f64, places: u32) -> String {
    if mag < 1e15 {
        let scale = pow10_u(places);
        let scaled = (mag * scale).round();
        if scaled.is_finite() && scaled < 1e16 {
            let n = scaled as u128;
            let div = 10u128.pow(places);
            return emit_parts(neg, n / div, n % div, places);
        }
    }
    emit_large(neg, mag, places)
}

fn emit_large(neg: bool, mag: f64, places: u32) -> String {
    let whole = mag.trunc();
    let frac = if places == 0 {
        0
    } else {
        let scale = pow10_u(places);
        ((mag - whole) * scale).round() as u128
    };
    // `mag` is finite and ≥ 1e15 here; `trunc` fits u128 for |n| < 1e38.
    let whole_u = if whole < 1e38 {
        whole as u128
    } else {
        return emit_fallback_string(neg, mag, places);
    };
    emit_parts(neg, whole_u, frac, places)
}

fn emit_naive(rounded: f64, decimals: i32) -> String {
    let neg = rounded < 0.0;
    let mag = rounded.abs();
    let places = decimals.max(0) as usize;
    let raw = if places == 0 {
        format!("{mag:.0}")
    } else {
        format!("{mag:.p$}", p = places)
    };
    wrap(neg, &insert_commas(&raw))
}

fn emit_fallback_string(neg: bool, mag: f64, places: u32) -> String {
    emit_naive(if neg { -mag } else { mag }, places as i32)
}

fn emit_parts(neg: bool, whole: u128, frac: u128, places: u32) -> String {
    // '(' + '$' + 40 digits/commas + '.' + 20 frac + ')'
    let mut buf = [0u8; 80];
    let mut end = 80usize;
    if places > 0 {
        let mut x = frac;
        for _ in 0..places {
            end -= 1;
            buf[end] = b'0' + (x % 10) as u8;
            x /= 10;
        }
        end -= 1;
        buf[end] = b'.';
    }
    if whole == 0 {
        end -= 1;
        buf[end] = b'0';
    } else {
        let mut x = whole;
        let mut digits = 0u32;
        while x > 0 {
            if digits > 0 && digits % 3 == 0 {
                end -= 1;
                buf[end] = b',';
            }
            end -= 1;
            buf[end] = b'0' + (x % 10) as u8;
            x /= 10;
            digits += 1;
        }
    }
    end -= 1;
    buf[end] = b'$';
    if neg {
        end -= 1;
        buf[end] = b'(';
        let mut s = String::with_capacity(80 - end + 1);
        s.push_str(unsafe { std::str::from_utf8_unchecked(&buf[end..]) });
        s.push(')');
        s
    } else {
        String::from(unsafe { std::str::from_utf8_unchecked(&buf[end..]) })
    }
}

fn wrap(neg: bool, grouped: &str) -> String {
    if neg {
        let mut s = String::with_capacity(grouped.len() + 3);
        s.push('(');
        s.push('$');
        // grouped already includes no '$'
        if let Some(rest) = grouped.strip_prefix('$') {
            s.push_str(rest);
        } else {
            s.push_str(grouped);
        }
        s.push(')');
        s
    } else if grouped.starts_with('$') {
        grouped.to_string()
    } else {
        let mut s = String::with_capacity(grouped.len() + 1);
        s.push('$');
        s.push_str(grouped);
        s
    }
}

fn insert_commas(raw: &str) -> String {
    let (int_part, frac) = match raw.split_once('.') {
        Some((a, b)) => (a, Some(b)),
        None => (raw, None),
    };
    let bytes = int_part.as_bytes();
    let n = bytes.len();
    let commas = if n <= 3 { 0 } else { (n - 1) / 3 };
    let mut out = String::with_capacity(n + commas + frac.map(|f| f.len() + 1).unwrap_or(0));
    for (i, &c) in bytes.iter().enumerate() {
        if i > 0 && (n - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c as char);
    }
    if let Some(f) = frac {
        out.push('.');
        out.push_str(f);
    }
    out
}

#[inline]
fn pow10_u(e: u32) -> f64 {
    let i = e as usize;
    if i < POW10.len() {
        POW10[i]
    } else {
        10f64.powi(e as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn both(n: f64, d: i32) -> String {
        let fast = dollar(n, d).unwrap();
        let slow = dollar_naive(n, d).unwrap();
        assert_eq!(
            fast, slow,
            "dollar mismatch n={n} d={d}: fast={fast} naive={slow}"
        );
        fast
    }

    #[test]
    fn microsoft_examples() {
        assert_eq!(both(1234.567, 2), "$1,234.57");
        assert_eq!(both(1234.567, -2), "$1,200");
        assert_eq!(both(-1234.567, -2), "($1,200)");
        assert_eq!(both(-0.123, 4), "($0.1230)");
        assert_eq!(both(99.888, 2), "$99.89");
    }

    #[test]
    fn default_two_decimals_and_zero() {
        assert_eq!(both(1234.567, 2), "$1,234.57");
        assert_eq!(both(1234.567, 0), "$1,235");
        assert_eq!(both(0.0, 2), "$0.00");
        assert_eq!(both(-0.0, 2), "$0.00");
        assert_eq!(both(0.4, 0), "$0");
        assert_eq!(both(0.5, 0), "$1");
    }

    #[test]
    fn parentheses_not_leading_minus() {
        assert_eq!(both(-1234.567, 2), "($1,234.57)");
        assert_eq!(both(-2.5, 0), "($3)");
        assert_eq!(both(-0.006, 2), "($0.01)");
        // Sign comes from the rounded value.
        assert_eq!(both(-0.001, 2), "$0.00");
        assert_eq!(both(-0.004, 2), "$0.00");
    }

    #[test]
    fn half_away_and_ieee_leftover() {
        assert_eq!(both(2.5, 0), "$3");
        assert_eq!(both(2.15, 1), "$2.2");
        assert_eq!(both(1.995, 2), "$2.00");
        assert_eq!(both(1234.5, 0), "$1,235");
    }

    #[test]
    fn negative_decimals_to_zero() {
        assert_eq!(both(11.24, -2), "$0");
        assert_eq!(both(123.598, -2), "$100");
        assert_eq!(both(2563112.0, -3), "$2,563,000");
        assert_eq!(both(1234.567, -1), "$1,230");
    }

    #[test]
    fn thousands_and_pad() {
        assert_eq!(both(1_234_567.89, 2), "$1,234,567.89");
        assert_eq!(both(12.34, 4), "$12.3400");
        assert_eq!(both(1.0, 2), "$1.00");
        assert_eq!(both(1.234, 5), "$1.23400");
    }

    #[test]
    fn value_hot_path_and_coerce() {
        assert_eq!(
            dollar_value(&ExcelValue::Number(1234.567), None),
            ExcelValue::Text("$1,234.57".into())
        );
        assert_eq!(
            dollar_value(&ExcelValue::Empty, None),
            ExcelValue::Text("$0.00".into())
        );
        assert_eq!(
            dollar_value(&ExcelValue::Bool(true), None),
            ExcelValue::Text("$1.00".into())
        );
        assert_eq!(
            dollar_value(&ExcelValue::Bool(false), None),
            ExcelValue::Text("$0.00".into())
        );
        assert_eq!(
            dollar_value(&ExcelValue::Text("1234.5".into()), None),
            ExcelValue::Text("$1,234.50".into())
        );
        assert_eq!(
            dollar_value(&ExcelValue::Text("  12.3  ".into()), None),
            ExcelValue::Text("$12.30".into())
        );
        assert_eq!(
            dollar_value(&ExcelValue::Text("1E3".into()), None),
            ExcelValue::Text("$1,000.00".into())
        );
        assert_eq!(
            dollar_value(&ExcelValue::Text("x".into()), None),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            dollar_value(&ExcelValue::Text("".into()), None),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            dollar_value(&ExcelValue::Text("$5".into()), None),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            dollar_value(&ExcelValue::Text("1,000".into()), None),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            dollar_value(&ExcelValue::Error(ExcelError::Div0), None),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            dollar_value(
                &ExcelValue::Number(1234.567),
                Some(&ExcelValue::Number(2.9))
            ),
            ExcelValue::Text("$1,234.57".into())
        );
        assert_eq!(
            dollar_value(
                &ExcelValue::Number(1234.567),
                Some(&ExcelValue::Number(-2.9))
            ),
            ExcelValue::Text("$1,200".into())
        );
        assert_eq!(
            dollar_value(&ExcelValue::Number(1234.567), Some(&ExcelValue::Bool(true))),
            ExcelValue::Text("$1,234.6".into())
        );
        assert_eq!(
            dollar_value(&ExcelValue::Number(1234.567), Some(&ExcelValue::Empty)),
            ExcelValue::Text("$1,235".into())
        );
    }

    #[test]
    fn value_naive_agrees() {
        let cases = [
            (ExcelValue::Number(-7.0), None),
            (ExcelValue::Number(1234.567), Some(ExcelValue::Number(2.0))),
            (ExcelValue::Empty, None),
            (ExcelValue::Bool(true), None),
            (ExcelValue::Text("-1E3".into()), None),
            (ExcelValue::Text("x".into()), None),
            (ExcelValue::Error(ExcelError::Na), None),
        ];
        for (v, d) in &cases {
            assert_eq!(
                dollar_value(v, d.as_ref()),
                dollar_value_naive(v, d.as_ref()),
                "{v:?} {d:?}"
            );
        }
    }

    #[test]
    fn naive_matches_fast_over_grid() {
        let digits = [-4, -3, -2, -1, 0, 1, 2, 3, 4];
        for i in -200i32..=200 {
            let n = i as f64 * 13.7 + 0.15;
            for &d in &digits {
                both(n, d);
                both(-n, d);
            }
        }
    }

    #[test]
    fn packed_slice_matches() {
        let src: Vec<f64> = (0..64)
            .map(|i| {
                if i % 2 == 0 {
                    i as f64 + 0.125
                } else {
                    -(i as f64) - 0.375
                }
            })
            .collect();
        let mut fast = vec![String::new(); src.len()];
        let mut slow = vec![String::new(); src.len()];
        dollar_slice(&src, 2, &mut fast);
        dollar_slice_naive(&src, 2, &mut slow);
        assert_eq!(fast, slow);
        assert_eq!(fast[0], "$0.13");
    }
}
