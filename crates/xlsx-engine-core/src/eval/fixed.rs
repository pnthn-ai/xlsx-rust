//! Excel `FIXED(number, [decimals], [no_commas])` — fixed-point text.
//!
//! Desktop Excel / Microsoft FIXED help (no golden-reading):
//! - Rounds `number` with Excel `ROUND` (half **away from zero**, 15-digit
//!   leftover snap) and returns **text**, not a number.
//! - Omitted `decimals` defaults to **2**. An empty slot (`FIXED(n,)`) is
//!   `0`, same as `ROUND(n,)`.
//! - `decimals < 0` rounds left of the decimal (`FIXED(1234.567, -1)` is
//!   `"1,230"`). Display then has no fractional part.
//! - `decimals` may be as large as **127** (Microsoft). Beyond f64's ~15
//!   significant digits the extra places are trailing zeros.
//! - `no_commas` omitted / `FALSE` / 0 / blank → thousands commas (en-US
//!   groups of 3). `TRUE` / nonzero → no commas.
//! - Sign is taken from the **rounded** value (`FIXED(-0.001, 2)` is
//!   `"0.00"`).
//! - Arithmetic coerce on `number` / `decimals`; `no_commas` uses `IF`
//!   logical coerce (text is `#VALUE!`). Wrong arity is `#VALUE!`.
//!
//! Production writes digits into a stack buffer and specialises the common
//! `decimals` (`0`, `1`, `2`, `3`, and the omitted-default 2). The naive
//! path uses `excel_round_naive` + `format!` + an allocating comma insert
//! so benches can print before/after. Does **not** read fixture goldens.
//!
//! `DOLLAR` / `TEXT` / `VALUE` / the ROUND family are unchanged — this
//! kernel only *calls* [`excel_round`](xlsx_types::excel_round).

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{excel_round, excel_round_naive, EvalError, ExcelError, ExcelValue};

/// Microsoft-documented maximum `decimals` (display places).
pub const FIXED_MAX_DECIMALS: i32 = 127;

/// `10^e` for `e` in `0..=22` (exact integer f64).
const POW10: [f64; 23] = [
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
    1_000_000_000_000_000_000_000.0,
    10_000_000_000_000_000_000_000.0,
];

/// Production `FIXED` on an already-coerced number.
#[inline]
pub fn fixed(n: f64, decimals: i32, no_commas: bool) -> String {
    match decimals {
        2 => emit_rounded(excel_round(n, 2), 2, no_commas),
        0 => emit_rounded(excel_round(n, 0), 0, no_commas),
        1 => emit_rounded(excel_round(n, 1), 1, no_commas),
        3 => emit_rounded(excel_round(n, 3), 3, no_commas),
        d => {
            let (round_d, places) = clamp_decimals(d);
            emit_rounded(excel_round(n, round_d), places, no_commas)
        }
    }
}

/// First-draft baseline: `excel_round_naive` + `format!` + allocating
/// comma insert. Same Excel results as [`fixed`] on the documented cases.
pub fn fixed_naive(n: f64, decimals: i32, no_commas: bool) -> String {
    let (round_d, places) = clamp_decimals(decimals);
    let r = excel_round_naive(n, round_d);
    emit_naive(r, places, no_commas)
}

/// Packed walk. Hot path for column-shaped work.
pub fn fixed_slice(n: &[f64], decimals: i32, no_commas: bool, out: &mut [String]) {
    let len = n.len().min(out.len());
    for i in 0..len {
        out[i] = fixed(n[i], decimals, no_commas);
    }
}

/// Naive slice matching [`fixed_naive`].
pub fn fixed_slice_naive(n: &[f64], decimals: i32, no_commas: bool, out: &mut [String]) {
    let len = n.len().min(out.len());
    for i in 0..len {
        out[i] = fixed_naive(n[i], decimals, no_commas);
    }
}

/// `FIXED(number, [decimals], [no_commas])` — scalar context.
///
/// Arity 1..=3. Errors evaluate left-to-right. Omitted `decimals` is 2;
/// an empty `decimals` slot is 0. Omitted / empty `no_commas` is FALSE.
pub(crate) fn fn_fixed(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.is_empty() || args.len() > 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let n = match coerce::to_number(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let decimals = if args.len() >= 2 {
        match &args[1] {
            Expr::Missing => 0,
            other => match coerce::to_number(&ev.eval_scalar(other, ctx)?) {
                Ok(d) => trunc_decimals(d),
                Err(e) => return Ok(ExcelValue::Error(e)),
            },
        }
    } else {
        2
    };
    let no_commas = if args.len() >= 3 {
        match &args[2] {
            Expr::Missing => false,
            other => match coerce::to_logical(&ev.eval_scalar(other, ctx)?) {
                Ok(b) => b,
                Err(e) => return Ok(ExcelValue::Error(e)),
            },
        }
    } else {
        false
    };
    Ok(ExcelValue::Text(fixed(n, decimals, no_commas)))
}

/// Truncate a coerced `decimals` toward zero and keep it in i32.
fn trunc_decimals(d: f64) -> i32 {
    if !d.is_finite() {
        return 0;
    }
    let t = d.trunc();
    if t > i32::MAX as f64 {
        i32::MAX
    } else if t < i32::MIN as f64 {
        i32::MIN
    } else {
        t as i32
    }
}

/// `(round_digits, display_places)`.
///
/// Rounding uses Excel `ROUND` in `-22..=22` (exact `10^e` table). Display
/// places are `0` when `decimals <= 0`, else `min(decimals, 127)`.
fn clamp_decimals(decimals: i32) -> (i32, u32) {
    let round_d = decimals.clamp(-22, 22);
    let places = if decimals <= 0 {
        0
    } else {
        (decimals as u32).min(FIXED_MAX_DECIMALS as u32)
    };
    (round_d, places)
}

fn emit_rounded(rounded: f64, places: u32, no_commas: bool) -> String {
    if !rounded.is_finite() {
        return emit_naive(rounded, places, no_commas);
    }
    if let Some(s) = try_emit_int(rounded, places, no_commas) {
        return s;
    }
    emit_naive(rounded, places, no_commas)
}

/// Integer-scale emit for `|n| < 1e15` and `places <= 18`.
fn try_emit_int(rounded: f64, places: u32, no_commas: bool) -> Option<String> {
    if places > 18 || !rounded.is_finite() || rounded.abs() >= 1e15 {
        return None;
    }
    let mag = rounded.abs();
    let factor = pow10_u(places);
    // `rounded` is already on a decimal grid; `+ 0.5` snaps leftover.
    let scaled = (mag * factor + 0.5).floor();
    if !scaled.is_finite() || scaled < 0.0 || scaled >= 1e22 {
        return None;
    }
    let scaled = scaled as u128;
    let div = 10u128.pow(places);
    let int_part = if places == 0 { scaled } else { scaled / div };
    let frac_part = if places == 0 { 0 } else { scaled % div };
    let neg = rounded < 0.0 && (int_part != 0 || frac_part != 0);
    Some(emit_parts(neg, int_part, frac_part, places, no_commas))
}

fn emit_parts(
    neg: bool,
    int_part: u128,
    frac_part: u128,
    places: u32,
    no_commas: bool,
) -> String {
    // sign + 40 int digits + commas + '.' + 127 frac + pad
    let mut buf = [0u8; 200];
    let mut i = buf.len();
    if places > 0 {
        let mut frac = frac_part;
        for _ in 0..places {
            i -= 1;
            buf[i] = b'0' + (frac % 10) as u8;
            frac /= 10;
        }
        i -= 1;
        buf[i] = b'.';
    }
    if no_commas {
        push_u128_at(&mut buf, &mut i, int_part);
    } else {
        push_grouped_at(&mut buf, &mut i, int_part);
    }
    if neg {
        i -= 1;
        buf[i] = b'-';
    }
    String::from_utf8_lossy(&buf[i..]).into_owned()
}

fn push_u128_at(buf: &mut [u8], i: &mut usize, mut n: u128) {
    if n == 0 {
        *i -= 1;
        buf[*i] = b'0';
        return;
    }
    while n > 0 {
        *i -= 1;
        buf[*i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
}

fn push_grouped_at(buf: &mut [u8], i: &mut usize, n: u128) {
    if n == 0 {
        *i -= 1;
        buf[*i] = b'0';
        return;
    }
    let mut x = n;
    let mut digits = 0u32;
    while x > 0 {
        if digits > 0 && digits % 3 == 0 {
            *i -= 1;
            buf[*i] = b',';
        }
        *i -= 1;
        buf[*i] = b'0' + (x % 10) as u8;
        x /= 10;
        digits += 1;
    }
}

fn emit_naive(rounded: f64, places: u32, no_commas: bool) -> String {
    let r = if rounded == 0.0 { 0.0 } else { rounded };
    let body = format!("{r:.prec$}", prec = places as usize);
    if no_commas {
        return body;
    }
    insert_commas_alloc(&body)
}

/// Allocating thousands-separator insert (bench “before”).
fn insert_commas_alloc(s: &str) -> String {
    let bytes = s.as_bytes();
    let (neg, rest) = if bytes.first() == Some(&b'-') {
        (true, &bytes[1..])
    } else {
        (false, bytes)
    };
    let dot = rest.iter().position(|&b| b == b'.').unwrap_or(rest.len());
    let int = &rest[..dot];
    let frac = &rest[dot..];
    let groups = if int.is_empty() {
        0
    } else {
        (int.len() - 1) / 3
    };
    let mut out = String::with_capacity((neg as usize) + int.len() + groups + frac.len());
    if neg {
        out.push('-');
    }
    for (k, &b) in int.iter().enumerate() {
        if k > 0 && (int.len() - k) % 3 == 0 {
            out.push(',');
        }
        out.push(b as char);
    }
    for &b in frac {
        out.push(b as char);
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

    fn both(n: f64, d: i32, nc: bool) -> String {
        let fast = fixed(n, d, nc);
        let slow = fixed_naive(n, d, nc);
        assert_eq!(
            fast, slow,
            "FIXED mismatch n={n} d={d} nc={nc}: fast={fast} naive={slow}"
        );
        fast
    }

    #[test]
    fn microsoft_examples() {
        assert_eq!(both(1234.567, 1, false), "1,234.6");
        assert_eq!(both(1234.567, -1, false), "1,230");
        assert_eq!(both(-1234.567, -1, true), "-1230");
        assert_eq!(both(44.332, 2, false), "44.33");
    }

    #[test]
    fn default_two_decimals_and_commas() {
        assert_eq!(both(1000.0, 2, false), "1,000.00");
        assert_eq!(both(1000.0, 0, false), "1,000");
        assert_eq!(both(1000.0, 0, true), "1000");
        assert_eq!(both(1234.5, 2, false), "1,234.50");
    }

    #[test]
    fn half_away_and_ieee() {
        assert_eq!(both(2.5, 0, false), "3");
        assert_eq!(both(-1.5, 0, false), "-2");
        assert_eq!(both(2.15, 1, false), "2.2");
        assert_eq!(both(1.1, 2, false), "1.10");
        assert_eq!(both(1.225, 2, false), "1.23");
    }

    #[test]
    fn rounded_zero_drops_minus() {
        assert_eq!(both(-0.001, 2, false), "0.00");
        assert_eq!(both(-0.004, 2, false), "0.00");
        assert_eq!(both(-0.005, 2, false), "-0.01");
        assert_eq!(both(0.0, 2, false), "0.00");
        assert_eq!(both(-0.0, 2, false), "0.00");
    }

    #[test]
    fn thousands_and_no_commas() {
        assert_eq!(both(1_234_567.89, 3, false), "1,234,567.890");
        assert_eq!(both(1_234_567.89, 3, true), "1234567.890");
        assert_eq!(both(123.0, 2, false), "123.00");
        assert_eq!(both(-1_234.56, 2, false), "-1,234.56");
        assert_eq!(both(-1_234.56, 2, true), "-1234.56");
    }

    #[test]
    fn negative_decimals() {
        assert_eq!(both(21.5, -1, false), "20");
        assert_eq!(both(626.3, -3, false), "1,000");
        assert_eq!(both(1.98, -1, false), "0");
        assert_eq!(both(-50.55, -2, false), "-100");
    }

    #[test]
    fn pad_high_decimals() {
        assert_eq!(both(1.5, 5, false), "1.50000");
        assert_eq!(both(1.0, 10, true), "1.0000000000");
        let s = both(1.0, 20, true);
        assert!(s.starts_with("1."));
        assert_eq!(s.len(), 22);
    }

    #[test]
    fn naive_matches_fast_over_grid() {
        let digits = [-4, -3, -2, -1, 0, 1, 2, 3, 4, 8];
        for i in -200i32..=200 {
            let n = i as f64 * 137.0 + 0.15;
            for &d in &digits {
                both(n, d, false);
                both(n, d, true);
                both(-n, d, false);
            }
        }
    }

    #[test]
    fn slice_matches_scalar() {
        let ns: Vec<f64> = (0..64).map(|i| i as f64 * 17.3 - 200.0).collect();
        let mut out = vec![String::new(); ns.len()];
        let mut naive = vec![String::new(); ns.len()];
        fixed_slice(&ns, 2, false, &mut out);
        fixed_slice_naive(&ns, 2, false, &mut naive);
        assert_eq!(out, naive);
        for (n, got) in ns.iter().zip(out.iter()) {
            assert_eq!(got, &fixed(*n, 2, false));
        }
    }
}
