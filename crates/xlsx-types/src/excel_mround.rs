//! Excel `MROUND(number, multiple)` — nearest multiple, half away from zero.
//!
//! Shared kernel so `calc-core` and `seed-compliant` stay aligned. Does not
//! read fixture goldens — callers pass coerced `f64`s.
//!
//! Desktop Excel / Microsoft MROUND help:
//! - Rounds `number` to the nearest multiple of `multiple`. Ties (remainder
//!   ≥ half of `|multiple|`) go **away from zero**.
//! - Opposite signs → `#NUM!` (`MROUND(5, -2)`). Zero number is not a sign
//!   clash (`MROUND(0, -3)` is `0`).
//! - Multiple `0` → `0` (including `MROUND(10, 0)` and `MROUND(0, 0)`).
//!   That is not classic `FLOOR` / `CEILING` zero-significance `#DIV/0!`.
//! - IEEE nearly-multiples such as `MROUND(1.2, 0.1)` stay `1.2`. IEEE
//!   nearly-halves such as `MROUND(1.25, 0.1)` still tie-away to `1.3`.
//!
//! Production: sign/zero checks, `ROUND` when `|multiple| == 1`, a
//! safe-integer `i64` path, then a cheap 15-digit snap-to-half on the
//! quotient. The naive path always runs `excel_round_15` (`log10` /
//! `powi`) on both args and the quotient so benches can print before/after.

use crate::error::ExcelError;
use crate::excel_round::excel_round;
use crate::value::excel_round_15;

/// Largest magnitude still exactly representable as an integer in `f64`.
const SAFE_INT: f64 = (1i64 << 53) as f64;

/// Production Excel `MROUND` kernel.
#[inline]
pub fn excel_mround(n: f64, m: f64) -> Result<f64, ExcelError> {
    check_mround(n, m)?;
    if n == 0.0 || m == 0.0 {
        return Ok(0.0);
    }
    // Same-sign unit multiple is `ROUND(n, 0)` (half away from zero).
    if m == 1.0 || m == -1.0 {
        return Ok(excel_round(n, 0));
    }
    if let Some(v) = try_int_path(n, m) {
        return Ok(v);
    }
    Ok(round_nearest(n, m))
}

/// First-draft kernel: 15-digit-snap both args and the quotient, then
/// half-away. Same sign / zero rules; `log10` / `powi` on every call.
#[inline]
pub fn excel_mround_naive(n: f64, m: f64) -> Result<f64, ExcelError> {
    check_mround(n, m)?;
    if n == 0.0 || m == 0.0 {
        return Ok(0.0);
    }
    let n = excel_round_15(n);
    let m = excel_round_15(m);
    if n == 0.0 || m == 0.0 {
        return Ok(0.0);
    }
    if (n > 0.0) != (m > 0.0) {
        return Err(ExcelError::Num);
    }
    let q = excel_round_15(n / m);
    Ok(excel_round_15(m * (q + 0.5).floor()))
}

/// IEEE `m * (n/m + 0.5).floor()` with no leftover snap. Contrast tests
/// only (`MROUND(1.25, 0.1)` may miss the half-tie).
#[inline]
pub fn excel_mround_ieee(n: f64, m: f64) -> Result<f64, ExcelError> {
    check_mround(n, m)?;
    if n == 0.0 || m == 0.0 {
        return Ok(0.0);
    }
    Ok(m * (n / m + 0.5).floor())
}

/// Apply [`excel_mround`] to every `n[i]` with a constant multiple.
///
/// Returns the number of `#NUM!` inputs (those slots are left unchanged).
/// Hot path for column-shaped work.
pub fn excel_mround_slice(n: &[f64], m: f64, out: &mut [f64]) -> usize {
    let len = n.len().min(out.len());
    let mut errs = 0usize;
    if m == 0.0 {
        for i in 0..len {
            if n[i].is_finite() {
                out[i] = 0.0;
            } else {
                errs += 1;
            }
        }
        return errs;
    }
    if !m.is_finite() {
        return len;
    }
    if m == 1.0 || m == -1.0 {
        for i in 0..len {
            let ni = n[i];
            if !ni.is_finite() {
                errs += 1;
            } else if ni == 0.0 {
                out[i] = 0.0;
            } else if (ni > 0.0) != (m > 0.0) {
                errs += 1;
            } else {
                out[i] = excel_round(ni, 0);
            }
        }
        return errs;
    }
    if is_safe_int(m) {
        let mi = m as i64;
        for i in 0..len {
            let ni = n[i];
            if is_safe_int(ni) {
                if ni == 0.0 {
                    out[i] = 0.0;
                    continue;
                }
                if (ni > 0.0) != (m > 0.0) {
                    errs += 1;
                    continue;
                }
                if let Some(v) = i64_mround(ni as i64, mi) {
                    out[i] = v as f64;
                    continue;
                }
            }
            match excel_mround(ni, m) {
                Ok(v) => out[i] = v,
                Err(_) => errs += 1,
            }
        }
        return errs;
    }
    if m > 0.0 {
        for i in 0..len {
            let ni = n[i];
            if !ni.is_finite() {
                errs += 1;
            } else if ni == 0.0 {
                out[i] = 0.0;
            } else if ni < 0.0 {
                errs += 1;
            } else {
                out[i] = round_nearest(ni, m);
            }
        }
        return errs;
    }
    for i in 0..len {
        match excel_mround(n[i], m) {
            Ok(v) => out[i] = v,
            Err(_) => errs += 1,
        }
    }
    errs
}

/// Naive slice baseline matching [`excel_mround_naive`].
pub fn excel_mround_slice_naive(n: &[f64], m: f64, out: &mut [f64]) -> usize {
    let len = n.len().min(out.len());
    let mut errs = 0usize;
    for i in 0..len {
        match excel_mround_naive(n[i], m) {
            Ok(v) => out[i] = v,
            Err(_) => errs += 1,
        }
    }
    errs
}

#[inline]
fn check_mround(n: f64, m: f64) -> Result<(), ExcelError> {
    if !n.is_finite() || !m.is_finite() {
        return Err(ExcelError::Num);
    }
    if n == 0.0 || m == 0.0 {
        return Ok(());
    }
    if (n > 0.0) != (m > 0.0) {
        return Err(ExcelError::Num);
    }
    Ok(())
}

#[inline]
fn try_int_path(n: f64, m: f64) -> Option<f64> {
    if !is_safe_int(m) || !is_safe_int(n) {
        return None;
    }
    Some(i64_mround(n as i64, m as i64)? as f64)
}

#[inline]
fn is_safe_int(x: f64) -> bool {
    x.is_finite() && x == x.trunc() && x.abs() <= SAFE_INT
}

/// Same-sign integer nearest-multiple. Half (`2*|r| >= |m|`) away from zero.
#[inline]
fn i64_mround(n: i64, m: i64) -> Option<i64> {
    if m == 0 {
        return Some(0);
    }
    let q = n / m;
    let r = n % m;
    let q = if r.unsigned_abs() * 2 >= m.unsigned_abs() {
        q.checked_add(1)?
    } else {
        q
    };
    q.checked_mul(m)
}

#[inline]
fn round_nearest(n: f64, m: f64) -> f64 {
    let q = n / m;
    // Snap IEEE leftovers of `.0` / `.5` so `1.25 / 0.1` still ties away
    // and `1.2 / 0.1` stays a multiple (15-digit model).
    let qs = snap_15_half(q);
    excel_round_15(m * (qs + 0.5).floor())
}

/// Snap binary leftovers that agree to Excel's 15 significant digits,
/// including half-integers. Same idea as [`crate::excel_round`].
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::excel_num_eq;

    fn n(v: Result<f64, ExcelError>) -> f64 {
        v.expect("number")
    }

    fn both(num: f64, mul: f64) -> Result<f64, ExcelError> {
        let fast = excel_mround(num, mul);
        let slow = excel_mround_naive(num, mul);
        match (fast, slow) {
            (Ok(a), Ok(b)) => {
                assert!(
                    excel_num_eq(a, b),
                    "MROUND({num},{mul}) mismatch: fast={a} naive={b}"
                );
                Ok(a)
            }
            (a, b) => {
                assert_eq!(a, b, "MROUND({num},{mul}) mismatch: fast={a:?} naive={b:?}");
                a
            }
        }
    }

    #[test]
    fn microsoft_mround_examples() {
        assert_eq!(n(both(10.0, 3.0)), 9.0);
        assert_eq!(n(both(-10.0, -3.0)), -9.0);
        assert!(excel_num_eq(n(both(1.3, 0.2)), 1.4));
        assert_eq!(both(5.0, -2.0), Err(ExcelError::Num));
    }

    #[test]
    fn sign_and_zero_multiple() {
        assert_eq!(both(10.0, -3.0), Err(ExcelError::Num));
        assert_eq!(both(-10.0, 3.0), Err(ExcelError::Num));
        assert_eq!(n(both(10.0, 0.0)), 0.0);
        assert_eq!(n(both(-10.0, 0.0)), 0.0);
        assert_eq!(n(both(0.0, 0.0)), 0.0);
        assert_eq!(n(both(0.0, 3.0)), 0.0);
        assert_eq!(n(both(0.0, -3.0)), 0.0);
    }

    #[test]
    fn midpoints_away_from_zero() {
        assert_eq!(n(both(1.5, 1.0)), 2.0);
        assert_eq!(n(both(2.5, 1.0)), 3.0);
        assert_eq!(n(both(-1.5, -1.0)), -2.0);
        assert_eq!(n(both(-2.5, -1.0)), -3.0);
        assert_eq!(n(both(21.0, 14.0)), 28.0);
        assert_eq!(n(both(1.5, 3.0)), 3.0);
        assert_eq!(n(both(4.5, 3.0)), 6.0);
        assert_eq!(n(both(22.5, 5.0)), 25.0);
        assert_eq!(n(both(-22.5, -5.0)), -25.0);
    }

    #[test]
    fn below_and_above_half() {
        assert_eq!(n(both(10.0, 3.0)), 9.0);
        assert_eq!(n(both(11.0, 3.0)), 12.0);
        assert_eq!(n(both(-11.0, -3.0)), -12.0);
        assert_eq!(n(both(21.0, 5.0)), 20.0);
        assert_eq!(n(both(23.0, 5.0)), 25.0);
        assert!(excel_num_eq(n(both(1.23, 0.05)), 1.25));
        assert!(excel_num_eq(n(both(1.22, 0.05)), 1.20));
    }

    #[test]
    fn already_multiple_is_identity() {
        assert_eq!(n(both(9.0, 3.0)), 9.0);
        assert_eq!(n(both(-9.0, -3.0)), -9.0);
        assert!(excel_num_eq(n(both(1.2, 0.1)), 1.2));
        assert!(excel_num_eq(n(both(2.4, 0.2)), 2.4));
        assert!(excel_num_eq(n(both(0.3, 0.1)), 0.3));
    }

    #[test]
    fn unit_multiple_shares_round() {
        assert_eq!(n(both(2.15, 1.0)), 2.0);
        assert_eq!(n(both(2.5, 1.0)), 3.0);
        assert_eq!(n(both(7.000000000000001, 1.0)), 7.0);
        assert_eq!(n(both(6.999999999999999, 1.0)), 7.0);
    }

    #[test]
    fn ieee_nearly_half_ties_away() {
        // 2.15 / 0.1 is 21.4999… in IEEE; without a snap, half-away would
        // incorrectly yield 2.1 instead of Microsoft's 2.2.
        let ieee = excel_mround_ieee(2.15, 0.1).unwrap();
        assert!(
            (ieee - 2.1).abs() < 1e-12,
            "IEEE MROUND(2.15, 0.1) should miss the half-tie, got {ieee}"
        );
        assert!(excel_num_eq(n(excel_mround(2.15, 0.1)), 2.2));
        assert!(excel_num_eq(n(excel_mround_naive(2.15, 0.1)), 2.2));
        assert!(excel_num_eq(n(excel_mround(1.25, 0.1)), 1.3));
    }

    #[test]
    fn ieee_nearly_multiple_stays() {
        let ieee = excel_mround_ieee(1.2, 0.1).unwrap();
        assert_ne!(
            ieee.to_bits(),
            1.2f64.to_bits(),
            "IEEE MROUND(1.2, 0.1) should not be bitwise 1.2, got {ieee}"
        );
        assert_eq!(n(excel_mround(1.2, 0.1)), 1.2);
        assert_eq!(n(excel_mround_naive(1.2, 0.1)), 1.2);
    }

    #[test]
    fn tenths_leftover_snaps() {
        let tenths = (0..10).fold(0.0, |a, _| a + 0.1);
        assert!(tenths < 1.0);
        assert_eq!(n(both(tenths, 1.0)), 1.0);
        assert!(excel_num_eq(n(both(tenths, 0.1)), 1.0));
    }

    #[test]
    fn nonfinite_is_num() {
        assert_eq!(excel_mround(f64::INFINITY, 1.0), Err(ExcelError::Num));
        assert_eq!(excel_mround(1.0, f64::NAN), Err(ExcelError::Num));
        assert_eq!(excel_mround(f64::NEG_INFINITY, -1.0), Err(ExcelError::Num));
    }

    #[test]
    fn integer_path_matches_naive_on_ints() {
        for num in [-20i64, -11, -10, -7, -5, -4, -1, 0, 1, 4, 5, 7, 10, 11, 20] {
            for mul in [-7i64, -5, -3, -2, -1, 1, 2, 3, 5, 7] {
                both(num as f64, mul as f64).ok();
            }
        }
    }

    #[test]
    fn naive_matches_fast_over_clean_grid() {
        for i in -80i32..=80 {
            let num = i as f64 * 0.137 + 0.15;
            for mul in [1.0, 2.0, 3.0, 7.0, 0.05, 0.1, 0.25, 0.5] {
                if num > 0.0 {
                    let _ = both(num, mul);
                } else if num < 0.0 {
                    let _ = both(num, -mul);
                    assert_eq!(both(num, mul), Err(ExcelError::Num));
                } else {
                    let _ = both(num, mul);
                    let _ = both(num, -mul);
                }
            }
        }
    }

    #[test]
    fn slice_matches_scalar() {
        let ns: Vec<f64> = (0..64).map(|i| i as f64 * 1.7 - 20.0).collect();
        for m in [1.0, 3.0, 0.1, -2.0, 0.0] {
            let mut out = vec![0.0; ns.len()];
            let errs = excel_mround_slice(&ns, m, &mut out);
            let expect_err = ns.iter().filter(|n| excel_mround(**n, m).is_err()).count();
            assert_eq!(errs, expect_err, "slice errs m={m}");
            for (num, got) in ns.iter().zip(out.iter()) {
                if let Ok(want) = excel_mround(*num, m) {
                    assert_eq!(*got, want, "slice MROUND({num},{m})");
                }
            }
        }
        let mut naive = vec![0.0; ns.len()];
        excel_mround_slice_naive(&ns, 3.0, &mut naive);
        for (num, got) in ns.iter().zip(naive.iter()) {
            if let Ok(want) = excel_mround_naive(*num, 3.0) {
                assert_eq!(*got, want);
            }
        }
    }

    #[test]
    fn slice_zero_multiple_is_zero() {
        let ns = [0.0, 1.0, -2.0, 7.5];
        let mut out = [9.0; 4];
        let errs = excel_mround_slice(&ns, 0.0, &mut out);
        assert_eq!(errs, 0);
        assert_eq!(out, [0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn slice_sign_clash_counts_errors() {
        let ns = [1.0, 2.5, 0.0, -1.0];
        let mut out = [9.0; 4];
        let errs = excel_mround_slice(&ns, -2.0, &mut out);
        assert_eq!(errs, 2);
        assert_eq!(out[2], 0.0);
        assert_eq!(out[3], excel_mround(-1.0, -2.0).unwrap());
    }
}
