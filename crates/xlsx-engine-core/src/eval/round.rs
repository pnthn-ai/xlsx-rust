//! Excel `ROUNDUP` / `ROUNDDOWN` kernel.
//!
//! Desktop Excel (Microsoft):
//! - `ROUNDUP` always rounds **away from zero**.
//! - `ROUNDDOWN` always rounds **toward zero** (same direction as `TRUNC`).
//! - Negative `num_digits` rounds to the left of the decimal (tens, hundreds, …).
//! - A signed input is converted to its absolute value, rounded, then the sign
//!   is reapplied.
//!
//! Production specialises the common `num_digits` (`0`, `±1`, `±2`, `±3`)
//! and otherwise uses a table of exact `10^e` (e ≤ 22). Negative
//! `num_digits` divide by the integer `10^|d|` (never multiply by inexact
//! `0.1`). A 15-significant-digit snap keeps `ROUNDUP(1.1, 2)` at `1.1`.
//! The naive path issues two `powi` calls and has no specialised digits so
//! benches can print a before/after.

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
#[inline]
pub fn roundup(n: f64, digits: i32) -> f64 {
    match digits {
        0 => toward_or_away_int(n, Mode::Up),
        1 => scale_dir(n, 10.0, true, Mode::Up),
        2 => scale_dir(n, 100.0, true, Mode::Up),
        3 => scale_dir(n, 1_000.0, true, Mode::Up),
        -1 => scale_dir(n, 10.0, false, Mode::Up),
        -2 => scale_dir(n, 100.0, false, Mode::Up),
        -3 => scale_dir(n, 1_000.0, false, Mode::Up),
        d => excel_round_dir(n, d, Mode::Up),
    }
}

/// Production `ROUNDDOWN` kernel.
#[inline]
pub fn rounddown(n: f64, digits: i32) -> f64 {
    match digits {
        0 => toward_or_away_int(n, Mode::Down),
        1 => scale_dir(n, 10.0, true, Mode::Down),
        2 => scale_dir(n, 100.0, true, Mode::Down),
        3 => scale_dir(n, 1_000.0, true, Mode::Down),
        -1 => scale_dir(n, 10.0, false, Mode::Down),
        -2 => scale_dir(n, 100.0, false, Mode::Down),
        -3 => scale_dir(n, 1_000.0, false, Mode::Down),
        d => excel_round_dir(n, d, Mode::Down),
    }
}

/// Textbook baseline used by the hill-climb bench: two `powi` calls, no
/// specialized digit paths. Same 15-digit snap as production so results match.
pub fn roundup_naive(n: f64, digits: i32) -> f64 {
    excel_round_dir_naive(n, digits, Mode::Up)
}

/// Textbook baseline used by the hill-climb bench.
pub fn rounddown_naive(n: f64, digits: i32) -> f64 {
    excel_round_dir_naive(n, digits, Mode::Down)
}

/// Integer `num_digits`: snap 15-digit leftovers (`7 + 1e-15` stays 7) then
/// `ceil` (away) or `trunc` (toward).
#[inline]
fn toward_or_away_int(n: f64, mode: Mode) -> f64 {
    if !n.is_finite() {
        return n;
    }
    if n == 0.0 {
        return 0.0;
    }
    let sign = if n < 0.0 { -1.0 } else { 1.0 };
    sign * apply(snap_15(n.abs()), mode)
}

/// Scale by an exact integer power of ten (`p = 10^|digits|`).
/// `mul` is true for positive `num_digits` (multiply then divide).
#[inline]
fn scale_dir(n: f64, p: f64, mul: bool, mode: Mode) -> f64 {
    if n == 0.0 {
        return 0.0;
    }
    let sign = if n < 0.0 { -1.0 } else { 1.0 };
    let mag = n.abs();
    let scaled = if mul { mag * p } else { mag / p };
    let rounded = apply(snap_15(scaled), mode);
    if mul {
        sign * rounded / p
    } else {
        sign * rounded * p
    }
}

fn excel_round_dir(n: f64, digits: i32, mode: Mode) -> f64 {
    if !n.is_finite() {
        return n;
    }
    if n == 0.0 {
        return 0.0;
    }
    let e = digits.unsigned_abs();
    let p = pow10_u(e);
    scale_dir(n, p, digits > 0, mode)
}

fn excel_round_dir_naive(n: f64, digits: i32, mode: Mode) -> f64 {
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
    let rounded = apply(snap_15(scaled), mode);
    if digits >= 0 {
        sign * rounded / unscale
    } else {
        sign * rounded * unscale
    }
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
        // 15-digit leftover on the integer path must not bump.
        assert_eq!(both_up(7.000000000000001, 0), 7.0);
        assert_eq!(both_down(6.999999999999999, 0), 7.0);
    }

    #[test]
    fn zero_and_nonfinite() {
        assert_eq!(both_up(0.0, 5), 0.0);
        assert_eq!(both_down(-0.0, 2), 0.0);
        assert!(roundup(f64::INFINITY, 0).is_infinite());
        assert!(rounddown(f64::NAN, 0).is_nan());
    }

    #[test]
    fn naive_matches_fast_over_grid() {
        let digits = [-4, -3, -2, -1, 0, 1, 2, 3, 4];
        for i in -200i32..=200 {
            let n = i as f64 * 0.137 + 0.15;
            for &d in &digits {
                both_up(n, d);
                both_down(n, d);
                both_up(-n, d);
                both_down(-n, d);
            }
        }
    }
}
