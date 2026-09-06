//! Excel `ROUNDUP(number, [num_digits])` — always away from zero.
//!
//! Shared kernel so `calc-core` and `seed-compliant` stay aligned. Does **not**
//! read fixture goldens — callers pass a coerced `f64` (and optional digits).
//!
//! Desktop Excel / Microsoft ROUNDUP help:
//! - Always rounds **away from zero** (`ROUNDUP(-3.2, 0)` is `-4`; that is
//!   not `ROUNDDOWN` / `TRUNC`, which go toward zero).
//! - A signed input is converted to its absolute value, rounded, then the
//!   sign is reapplied (`ROUNDUP(-0.5, 0)` is `-1`).
//! - `num_digits` omitted defaults to `0` (nearest integer).
//! - `num_digits > 0` keeps that many decimals; `num_digits < 0` rounds to
//!   the left of the decimal (`ROUNDUP(123, -1)` is `130`).
//! - Fractional `num_digits` truncate toward zero (`1.9` → `1`, `-1.9` → `-1`).
//! - Arithmetic coerce: empty → `0`, `TRUE` → `1`, `FALSE` → `0`, numeric
//!   text parsed; other text is `#VALUE!`.
//! - Errors evaluate left-to-right (number, then digits). Wrong arity
//!   (`ROUNDUP()` / three+ args) is `#VALUE!`.
//!
//! Production specialises the common `num_digits` (`0`, `±1`, `±2`, `±3`)
//! and otherwise uses a table of exact `10^e` (e ≤ 22). Negative
//! `num_digits` divide by the integer `10^|d|` (never multiply by inexact
//! `0.1`). A 15-significant-digit snap keeps `ROUNDUP(1.1, 2)` at `1.1`.
//! The naive path issues two `powi` calls and has no specialised digits so
//! benches can print a before/after.

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

/// Exact `10^e` for `e` in `0..=22` (all representable as f64 integers).
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

/// Production `ROUNDUP` kernel (away from zero). `digits == 0` is the
/// omitted-`num_digits` path.
#[inline]
pub fn roundup(n: f64, digits: i32) -> f64 {
    match digits {
        0 => away_int(n),
        1 => scale_away(n, 10.0, true),
        2 => scale_away(n, 100.0, true),
        3 => scale_away(n, 1_000.0, true),
        -1 => scale_away(n, 10.0, false),
        -2 => scale_away(n, 100.0, false),
        -3 => scale_away(n, 1_000.0, false),
        d => excel_roundup(n, d),
    }
}

/// Textbook baseline used by the hill-climb bench: two `powi` calls, no
/// specialized digit paths. Same 15-digit snap as production so results match.
pub fn roundup_naive(n: f64, digits: i32) -> f64 {
    excel_roundup_naive(n, digits)
}

/// Apply [`roundup`] to every `n[i]`. Hot path for column-shaped work.
pub fn roundup_slice(n: &[f64], digits: &[i32], out: &mut [f64]) {
    let len = n.len().min(digits.len()).min(out.len());
    for i in 0..len {
        out[i] = roundup(n[i], digits[i]);
    }
}

/// Naive slice baseline matching [`roundup_naive`].
pub fn roundup_slice_naive(n: &[f64], digits: &[i32], out: &mut [f64]) {
    let len = n.len().min(digits.len()).min(out.len());
    for i in 0..len {
        out[i] = roundup_naive(n[i], digits[i]);
    }
}

/// Integer `num_digits` (including omitted): snap 15-digit leftovers
/// (`7 + 1e-15` stays 7) then `ceil` away from zero.
#[inline]
fn away_int(n: f64) -> f64 {
    if !n.is_finite() {
        return n;
    }
    if n == 0.0 {
        return 0.0;
    }
    let sign = if n < 0.0 { -1.0 } else { 1.0 };
    sign * snap_15(n.abs()).ceil()
}

/// Scale by an exact integer power of ten (`p = 10^|digits|`).
/// `mul` is true for positive `num_digits` (multiply then divide).
#[inline]
fn scale_away(n: f64, p: f64, mul: bool) -> f64 {
    if n == 0.0 {
        return 0.0;
    }
    let sign = if n < 0.0 { -1.0 } else { 1.0 };
    let mag = n.abs();
    let scaled = if mul { mag * p } else { mag / p };
    let rounded = snap_15(scaled).ceil();
    if mul {
        sign * rounded / p
    } else {
        sign * rounded * p
    }
}

fn excel_roundup(n: f64, digits: i32) -> f64 {
    if !n.is_finite() {
        return n;
    }
    if n == 0.0 {
        return 0.0;
    }
    let e = digits.unsigned_abs();
    let p = pow10_u(e);
    scale_away(n, p, digits > 0)
}

fn excel_roundup_naive(n: f64, digits: i32) -> f64 {
    if !n.is_finite() {
        return n;
    }
    if n == 0.0 {
        return 0.0;
    }
    // Two `powi` calls, no table / digit specialisation. Negative
    // `num_digits` still divide by the integer `10^|d|` so `* 0.1`
    // leftovers cannot diverge from production.
    let e = digits.unsigned_abs() as i32;
    let p = 10f64.powi(e);
    let unscale = 10f64.powi(e);
    let sign = if n < 0.0 { -1.0 } else { 1.0 };
    let mag = n.abs();
    let scaled = if digits >= 0 { mag * p } else { mag / p };
    let rounded = snap_15(scaled).ceil();
    if digits >= 0 {
        sign * rounded / unscale
    } else {
        sign * rounded * unscale
    }
}

/// Snap binary leftovers that agree to Excel's 15 significant digits.
///
/// `1.1 * 100` is `110.00000000000001` in IEEE; without a snap, `ROUNDUP`
/// would incorrectly yield `1.11`.
#[inline]
fn snap_15(x: f64) -> f64 {
    let r = x.round();
    let tol = x.abs() * 1e-14 + 1e-14;
    if (x - r).abs() <= tol {
        r
    } else {
        x
    }
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

/// `ROUNDUP(number, [num_digits])` — 1 or 2 args; omitted digits → 0.
pub(crate) fn fn_roundup(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.is_empty() || args.len() > 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let n = match coerce::to_number(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let digits = if args.len() == 2 {
        match coerce::to_number(&ev.eval_scalar(&args[1], ctx)?) {
            Ok(d) => d.trunc() as i32,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        0
    };
    Ok(ExcelValue::Number(roundup(n, digits)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn both(n: f64, d: i32) -> f64 {
        let fast = roundup(n, d);
        let slow = roundup_naive(n, d);
        assert_eq!(
            fast, slow,
            "roundup mismatch n={n} d={d}: fast={fast} naive={slow}"
        );
        fast
    }

    #[test]
    fn microsoft_roundup_examples() {
        assert_eq!(both(3.2, 0), 4.0);
        assert_eq!(both(76.9, 0), 77.0);
        assert_eq!(both(3.14159, 3), 3.142);
        assert_eq!(both(-3.14159, 1), -3.2);
        assert_eq!(both(31415.92654, -2), 31500.0);
    }

    #[test]
    fn omitted_num_digits_is_zero() {
        assert_eq!(both(3.2, 0), 4.0);
        assert_eq!(both(-3.2, 0), -4.0);
        assert_eq!(both(76.9, 0), 77.0);
        assert_eq!(both(0.1, 0), 1.0);
        assert_eq!(both(-0.1, 0), -1.0);
    }

    #[test]
    fn signed_away_from_zero() {
        assert_eq!(both(-3.2, 0), -4.0);
        assert_eq!(both(-0.5, 0), -1.0);
        assert_eq!(both(0.5, 0), 1.0);
        assert_eq!(both(-76.9, 0), -77.0);
        assert_eq!(both(1.1, 0), 2.0);
    }

    #[test]
    fn negative_num_digits() {
        assert_eq!(both(123.0, -1), 130.0);
        assert_eq!(both(-123.0, -1), -130.0);
        assert_eq!(both(4.0, -1), 10.0);
        assert_eq!(both(2_345_678.0, -4), 2_350_000.0);
        assert_eq!(both(31415.92654, -2), 31500.0);
    }

    #[test]
    fn already_at_precision_is_identity() {
        assert_eq!(both(3.2, 1), 3.2);
        assert_eq!(both(1.1, 2), 1.1);
        assert_eq!(both(1.0, 0), 1.0);
        assert_eq!(both(100.0, -2), 100.0);
        assert_eq!(both(-3.2, 1), -3.2);
    }

    #[test]
    fn ieee_leftover_does_not_bump() {
        // 1.1 * 100 is 110.00000000000001; snap keeps 1.10.
        assert_eq!(both(1.1, 2), 1.1);
        assert_eq!(both(2.2, 2), 2.2);
        assert_eq!(both(76.9, 2), 76.9);
        assert_eq!(both(1.15, 2), 1.15);
        // 15-digit leftover on the integer path must not bump.
        assert_eq!(both(7.000000000000001, 0), 7.0);
    }

    #[test]
    fn zero_and_nonfinite() {
        assert_eq!(both(0.0, 5), 0.0);
        assert_eq!(both(-0.0, 2), 0.0);
        assert_eq!(both(0.0, 0), 0.0);
        assert!(roundup(f64::INFINITY, 0).is_infinite());
        assert!(roundup(f64::NAN, 0).is_nan());
    }

    #[test]
    fn slice_matches_scalar() {
        let ns: Vec<f64> = (0..64).map(|i| i as f64 * 1.7 - 20.0).collect();
        let ds: Vec<i32> = (0..64)
            .map(|i| match i % 7 {
                0 => 0,
                1 => 1,
                2 => 2,
                3 => 3,
                4 => -1,
                5 => -2,
                _ => -3,
            })
            .collect();
        let mut out = vec![0.0; ns.len()];
        roundup_slice(&ns, &ds, &mut out);
        for i in 0..ns.len() {
            assert_eq!(out[i], roundup(ns[i], ds[i]));
        }
        let mut naive = vec![0.0; ns.len()];
        roundup_slice_naive(&ns, &ds, &mut naive);
        assert_eq!(out, naive);
    }

    #[test]
    fn naive_matches_fast_over_grid() {
        let digits = [-4, -3, -2, -1, 0, 1, 2, 3, 4];
        for i in -200i32..=200 {
            let n = i as f64 * 0.137 + 0.15;
            for &d in &digits {
                both(n, d);
                both(-n, d);
            }
        }
    }
}
