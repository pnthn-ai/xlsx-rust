//! Excel `ROUNDUP` / `ROUNDDOWN` kernel.
//!
//! Desktop Excel (Microsoft):
//! - `ROUNDUP` always rounds **away from zero**.
//! - `ROUNDDOWN` always rounds **toward zero** (same direction as `TRUNC`).
//! - Negative `num_digits` rounds to the left of the decimal (tens, hundreds, …).
//! - A signed input is converted to its absolute value, rounded, then the sign
//!   is reapplied.
//!
//! Production uses a table of exact `10^e` (e ≤ 22), integer scaling for
//! negative `num_digits` (so we never multiply by inexact `0.1`), and a
//! 15-significant-digit snap so `ROUNDUP(1.1, 2)` stays `1.1` instead of
//! bumping a binary leftover. The naive path calls `powi` on every invocation
//! so benches can print a before/after.

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

#[derive(Clone, Copy)]
enum Mode {
    /// Away from zero (`ROUNDUP`).
    Up,
    /// Toward zero (`ROUNDDOWN`).
    Down,
}

/// Production `ROUNDUP` kernel.
pub fn roundup(n: f64, digits: i32) -> f64 {
    excel_round_dir(n, digits, Mode::Up)
}

/// Production `ROUNDDOWN` kernel.
pub fn rounddown(n: f64, digits: i32) -> f64 {
    excel_round_dir(n, digits, Mode::Down)
}

/// `powi`-every-call baseline used by the hill-climb bench.
///
/// Same Excel semantics as [`roundup`]; slower because it computes `10^digits`
/// with `f64::powi` instead of a table / integer scale.
pub fn roundup_naive(n: f64, digits: i32) -> f64 {
    excel_round_dir_naive(n, digits, Mode::Up)
}

/// `powi`-every-call baseline used by the hill-climb bench.
pub fn rounddown_naive(n: f64, digits: i32) -> f64 {
    excel_round_dir_naive(n, digits, Mode::Down)
}

fn excel_round_dir(n: f64, digits: i32, mode: Mode) -> f64 {
    if !n.is_finite() {
        return n;
    }
    if n == 0.0 {
        return 0.0;
    }
    let sign = if n.is_sign_negative() { -1.0 } else { 1.0 };
    let mag = n.abs();
    if digits == 0 {
        return sign * apply(snap_15(mag), mode);
    }
    let e = digits.unsigned_abs();
    let p = pow10_u(e);
    let scaled = if digits > 0 { mag * p } else { mag / p };
    let rounded = apply(snap_15(scaled), mode);
    if digits > 0 {
        sign * rounded / p
    } else {
        sign * rounded * p
    }
}

fn excel_round_dir_naive(n: f64, digits: i32, mode: Mode) -> f64 {
    if !n.is_finite() {
        return n;
    }
    if n == 0.0 {
        return 0.0;
    }
    let factor = 10f64.powi(digits);
    let sign = if n.is_sign_negative() { -1.0 } else { 1.0 };
    let rounded = apply(snap_15(n.abs() * factor), mode);
    sign * rounded / factor
}

#[inline]
fn apply(snapped: f64, mode: Mode) -> f64 {
    match mode {
        Mode::Up => snapped.ceil(),
        Mode::Down => snapped.trunc(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn both_up(n: f64, d: i32) -> f64 {
        let fast = roundup(n, d);
        let slow = roundup_naive(n, d);
        assert_eq!(
            fast, slow,
            "roundup mismatch n={n} d={d}: fast={fast} naive={slow}"
        );
        fast
    }

    fn both_down(n: f64, d: i32) -> f64 {
        let fast = rounddown(n, d);
        let slow = rounddown_naive(n, d);
        assert_eq!(
            fast, slow,
            "rounddown mismatch n={n} d={d}: fast={fast} naive={slow}"
        );
        fast
    }

    #[test]
    fn microsoft_roundup_examples() {
        assert_eq!(both_up(3.2, 0), 4.0);
        assert_eq!(both_up(76.9, 0), 77.0);
        assert_eq!(both_up(3.14159, 3), 3.142);
        assert_eq!(both_up(-3.14159, 1), -3.2);
        assert_eq!(both_up(31415.92654, -2), 31500.0);
    }

    #[test]
    fn microsoft_rounddown_examples() {
        assert_eq!(both_down(3.2, 0), 3.0);
        assert_eq!(both_down(76.9, 0), 76.0);
        assert_eq!(both_down(3.14159, 3), 3.141);
        assert_eq!(both_down(-3.14159, 1), -3.1);
        assert_eq!(both_down(31415.92654, -2), 31400.0);
    }

    #[test]
    fn signed_away_and_toward_zero() {
        assert_eq!(both_up(-3.2, 0), -4.0);
        assert_eq!(both_down(-3.2, 0), -3.0);
        assert_eq!(both_up(-0.5, 0), -1.0);
        assert_eq!(both_down(-0.5, 0), 0.0);
        assert_eq!(both_up(-76.9, 0), -77.0);
        assert_eq!(both_down(-76.9, 0), -76.0);
    }

    #[test]
    fn negative_num_digits() {
        assert_eq!(both_up(123.0, -1), 130.0);
        assert_eq!(both_down(123.0, -1), 120.0);
        assert_eq!(both_up(-123.0, -1), -130.0);
        assert_eq!(both_down(-123.0, -1), -120.0);
        assert_eq!(both_down(-889.0, -1), -880.0);
        assert_eq!(both_down(2_345_678.0, -4), 2_340_000.0);
        assert_eq!(both_up(2_345_678.0, -4), 2_350_000.0);
    }

    #[test]
    fn already_at_precision_is_identity() {
        assert_eq!(both_up(3.2, 1), 3.2);
        assert_eq!(both_down(3.2, 1), 3.2);
        assert_eq!(both_up(1.1, 2), 1.1);
        assert_eq!(both_down(1.1, 2), 1.1);
        assert_eq!(both_up(1.0, 0), 1.0);
        assert_eq!(both_down(123.0, 0), 123.0);
    }

    #[test]
    fn ieee_leftover_does_not_bump() {
        // 1.1 * 100 is 110.00000000000001; snap keeps 1.10.
        assert_eq!(both_up(1.1, 2), 1.1);
        assert_eq!(both_up(2.2, 2), 2.2);
        assert_eq!(both_up(76.9, 2), 76.9);
        // 1.15 * 100 is 114.999…; snap keeps 1.15 for ROUNDDOWN.
        assert_eq!(both_down(1.15, 2), 1.15);
        assert_eq!(both_up(1.15, 2), 1.15);
    }

    #[test]
    fn zero_and_nonfinite() {
        assert_eq!(both_up(0.0, 5), 0.0);
        assert_eq!(both_down(-0.0, 2), 0.0);
        assert!(roundup(f64::INFINITY, 0).is_infinite());
        assert!(rounddown(f64::NAN, 0).is_nan());
    }
}
