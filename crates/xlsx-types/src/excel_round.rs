//! Excel `ROUND(number, [num_digits])` — half away from zero.
//!
//! Shared kernel so `calc-core` and `seed-compliant` stay aligned. Does not
//! read fixture goldens — callers pass a coerced `f64` and a truncated
//! `num_digits`.
//!
//! Desktop Excel (Microsoft ROUND help):
//! - Ties on `.5` round **away from zero** (commercial rounding), not
//!   banker's / half-to-even. `ROUND(2.5, 0)` is `3`; `ROUND(-1.5, 0)`
//!   is `-2`.
//! - Omitted `num_digits` defaults to `0` (nearest integer).
//! - `num_digits > 0` rounds to that many decimal places;
//!   `num_digits < 0` rounds to the left of the decimal (tens, hundreds, …).
//! - A signed input is converted to its absolute value, rounded, then the
//!   sign is reapplied.
//!
//! Production specialises the common `num_digits` (`0`, `±1`, `±2`, `±3`)
//! and otherwise uses a table of exact `10^e` (e ≤ 22). Negative
//! `num_digits` divide by the integer `10^|d|` (never multiply by inexact
//! `0.1`). A 15-significant-digit snap-to-half keeps `ROUND(2.15, 1)` at
//! `2.2` and `ROUND(1.1, 2)` at `1.1`. The naive path issues two `powi`
//! calls and has no specialised digits so benches can print a before/after.

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

/// Production Excel `ROUND` kernel.
#[inline]
pub fn excel_round(n: f64, digits: i32) -> f64 {
    match digits {
        0 => half_away_int(n),
        1 => scale_half(n, 10.0, true),
        2 => scale_half(n, 100.0, true),
        3 => scale_half(n, 1_000.0, true),
        -1 => scale_half(n, 10.0, false),
        -2 => scale_half(n, 100.0, false),
        -3 => scale_half(n, 1_000.0, false),
        d => excel_round_digits(n, d),
    }
}

/// Textbook baseline used by the hill-climb bench: two `powi` calls, no
/// specialised digit paths. Same 15-digit snap as production so results match.
#[inline]
pub fn excel_round_naive(n: f64, digits: i32) -> f64 {
    if !n.is_finite() {
        return n;
    }
    if n == 0.0 {
        return 0.0;
    }
    let e = digits.unsigned_abs() as i32;
    let p = 10f64.powi(e);
    let unscale = 10f64.powi(e);
    let sign = if n < 0.0 { -1.0 } else { 1.0 };
    let mag = n.abs();
    let scaled = if digits >= 0 { mag * p } else { mag / p };
    let rounded = half_away_mag(scaled);
    if digits >= 0 {
        sign * rounded / unscale
    } else {
        sign * rounded * unscale
    }
}

/// Apply [`excel_round`] to every `n[i]` with a constant `digits`.
/// Hot path for column-shaped work.
pub fn excel_round_slice(n: &[f64], digits: i32, out: &mut [f64]) {
    let len = n.len().min(out.len());
    for i in 0..len {
        out[i] = excel_round(n[i], digits);
    }
}

/// Naive slice baseline matching [`excel_round_naive`].
pub fn excel_round_slice_naive(n: &[f64], digits: i32, out: &mut [f64]) {
    let len = n.len().min(out.len());
    for i in 0..len {
        out[i] = excel_round_naive(n[i], digits);
    }
}

fn excel_round_digits(n: f64, digits: i32) -> f64 {
    if !n.is_finite() {
        return n;
    }
    if n == 0.0 {
        return 0.0;
    }
    let e = digits.unsigned_abs();
    let p = pow10_u(e);
    scale_half(n, p, digits > 0)
}

/// Integer `num_digits`: snap 15-digit leftovers (`7 + 1e-15` stays 7)
/// then half away from zero.
#[inline]
fn half_away_int(n: f64) -> f64 {
    if !n.is_finite() {
        return n;
    }
    if n == 0.0 {
        return 0.0;
    }
    let sign = if n < 0.0 { -1.0 } else { 1.0 };
    sign * half_away_mag(n.abs())
}

/// Scale by an exact integer power of ten (`p = 10^|digits|`).
/// `mul` is true for positive `num_digits` (multiply then divide).
#[inline]
fn scale_half(n: f64, p: f64, mul: bool) -> f64 {
    if n == 0.0 {
        return 0.0;
    }
    let sign = if n < 0.0 { -1.0 } else { 1.0 };
    let mag = n.abs();
    let scaled = if mul { mag * p } else { mag / p };
    let rounded = half_away_mag(scaled);
    if mul {
        sign * rounded / p
    } else {
        sign * rounded * p
    }
}

/// Half away from zero on a non-negative magnitude, after a 15-digit snap
/// to the nearest half so IEEE leftovers of `.5` still tie-away.
#[inline]
fn half_away_mag(x: f64) -> f64 {
    (snap_15_half(x) + 0.5).floor()
}

/// Snap binary leftovers that agree to Excel's 15 significant digits,
/// including half-integers.
///
/// `2.15 * 10` is `21.4999…` in IEEE; without a snap, `ROUND(2.15, 1)`
/// would incorrectly yield `2.1` instead of Microsoft's `2.2`.
/// `1.1 * 100` is `110.00000000000001`; the same snap keeps `1.1`.
#[inline]
fn snap_15_half(x: f64) -> f64 {
    let twice = x * 2.0;
    let r = twice.round();
    let tol = x.abs() * 1e-14 + 1e-14;
    if (twice - r).abs() <= tol * 2.0 {
        r * 0.5
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

    fn both(n: f64, d: i32) -> f64 {
        let fast = excel_round(n, d);
        let slow = excel_round_naive(n, d);
        assert_eq!(
            fast, slow,
            "round mismatch n={n} d={d}: fast={fast} naive={slow}"
        );
        fast
    }

    #[test]
    fn microsoft_round_examples() {
        assert_eq!(both(2.15, 1), 2.2);
        assert_eq!(both(2.149, 1), 2.1);
        assert_eq!(both(-1.475, 2), -1.48);
        assert_eq!(both(21.5, -1), 20.0);
        assert_eq!(both(626.3, -3), 1000.0);
        assert_eq!(both(1.98, -1), 0.0);
        assert_eq!(both(-50.55, -2), -100.0);
    }

    #[test]
    fn half_away_not_banker() {
        assert_eq!(both(2.5, 0), 3.0);
        assert_eq!(both(1.5, 0), 2.0);
        assert_eq!(both(3.5, 0), 4.0);
        assert_eq!(both(-1.5, 0), -2.0);
        assert_eq!(both(-2.5, 0), -3.0);
        assert_eq!(both(1.25, 1), 1.3);
        assert_eq!(both(-1.25, 1), -1.3);
    }

    #[test]
    fn omitted_digits_is_zero() {
        // Callers default omitted num_digits to 0; kernel matches that.
        assert_eq!(both(2.5, 0), 3.0);
        assert_eq!(both(-1.4, 0), -1.0);
        assert_eq!(both(21.5, 0), 22.0);
    }

    #[test]
    fn negative_num_digits() {
        assert_eq!(both(123.0, -1), 120.0);
        assert_eq!(both(125.0, -1), 130.0);
        assert_eq!(both(-123.0, -1), -120.0);
        assert_eq!(both(15.0, -1), 20.0);
        assert_eq!(both(25.0, -1), 30.0);
    }

    #[test]
    fn already_at_precision_is_identity() {
        assert_eq!(both(3.2, 1), 3.2);
        assert_eq!(both(1.1, 2), 1.1);
        assert_eq!(both(1.0, 0), 1.0);
        assert_eq!(both(123.0, 0), 123.0);
        assert_eq!(both(-3.2, 1), -3.2);
    }

    #[test]
    fn ieee_leftover_does_not_misround() {
        // 2.15 * 10 is 21.4999…; snap-to-half keeps Microsoft 2.2.
        assert_eq!(both(2.15, 1), 2.2);
        // 1.1 * 100 is 110.00…01; snap keeps 1.10.
        assert_eq!(both(1.1, 2), 1.1);
        assert_eq!(both(2.2, 2), 2.2);
        // 1.225 * 100 is 122.4999…; 15-digit model is 1.23.
        assert_eq!(both(1.225, 2), 1.23);
        // 15-digit leftover on the integer path must not bump past .5.
        assert_eq!(both(7.000000000000001, 0), 7.0);
        assert_eq!(both(6.999999999999999, 0), 7.0);
    }

    #[test]
    fn tenths_and_point_three_leftover() {
        let tenths = (0..10).fold(0.0, |a, _| a + 0.1);
        assert!(tenths < 1.0);
        assert_eq!(both(tenths, 0), 1.0);
        let sub = 0.3 - 0.1 - 0.2;
        assert!(sub < 0.0);
        assert_eq!(both(sub, 0), 0.0);
    }

    #[test]
    fn zero_and_nonfinite() {
        assert_eq!(both(0.0, 5), 0.0);
        assert_eq!(both(-0.0, 2), 0.0);
        assert_eq!(both(0.0, -2), 0.0);
        assert!(excel_round(f64::INFINITY, 0).is_infinite());
        assert!(excel_round(f64::NAN, 0).is_nan());
        assert!(excel_round_naive(f64::INFINITY, 1).is_infinite());
        assert!(excel_round_naive(f64::NAN, -1).is_nan());
    }

    #[test]
    fn large_magnitude_integers() {
        let safe = (1i64 << 53) as f64;
        assert_eq!(excel_round(safe, 0), safe);
        assert_eq!(excel_round(-safe, 0), -safe);
        let half = (1u64 << 51) as f64 + 0.5;
        assert_eq!(excel_round(half, 0), (1u64 << 51) as f64 + 1.0);
    }

    #[test]
    fn slice_matches_scalar() {
        let ns: Vec<f64> = (0..64).map(|i| i as f64 * 1.7 - 20.0).collect();
        let mut out = vec![0.0; ns.len()];
        excel_round_slice(&ns, 1, &mut out);
        for (n, got) in ns.iter().zip(out.iter()) {
            assert_eq!(*got, excel_round(*n, 1));
        }
        let mut naive = vec![0.0; ns.len()];
        excel_round_slice_naive(&ns, 1, &mut naive);
        for (n, got) in ns.iter().zip(naive.iter()) {
            assert_eq!(*got, excel_round_naive(*n, 1));
        }
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
