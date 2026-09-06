//! Excel `MROUND(number, multiple)` — nearest multiple, half away from zero.
//!
//! Shared kernel so `calc-core` and `seed-compliant` stay aligned. Does not
//! read fixture goldens — callers pass coerced `f64`s.
//!
//! Desktop Excel (Microsoft `MROUND` help):
//! - Rounds `number` to the nearest multiple of `multiple`.
//! - Remainder ≥ half of `|multiple|` rounds **away from zero** (not
//!   banker's / half-to-even). `MROUND(1.5, 1)` is `2`; `MROUND(2.5, 5)`
//!   is `5`, not `0`.
//! - Number and multiple must have the **same sign**. Opposite signs →
//!   `#NUM!` (`MROUND(5, -2)`). Zero number is not a sign clash
//!   (`MROUND(0, -3)` is `0`).
//! - Multiple `0` returns `0` (including `MROUND(10, 0)` and
//!   `MROUND(0, 0)`). That is not classic `FLOOR` / `CEILING` `#DIV/0!`.
//! - Microsoft examples: `MROUND(10, 3)` is `9`; `MROUND(-10, -3)` is
//!   `-9`; `MROUND(1.3, 0.2)` is `1.4`.
//! - Known leftover midpoints (Microsoft): `MROUND(6.05, 0.1)` is `6.0`
//!   while `MROUND(7.05, 0.1)` is `7.1`. Production uses raw half-away of
//!   `n/m` (IEEE leftover), **not** [`excel_round`](crate::excel_round)'s
//!   15-digit snap-to-half — that path would turn `6.05/0.1` into `6.1`.
//!
//! Production: sign/zero checks, then [`excel_round`](crate::excel_round)
//! when `|multiple| == 1` (same leftover snap as `ROUND`), else a
//! safe-integer `i64` path or a cheap nearly-integer multiple probe.
//! The naive path always runs `excel_round_15` (`log10` / `powi`) on both
//! args so benches can print before/after.

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
    // `|multiple| == 1` is `ROUND(n, 0)` — share the leftover-snap kernel.
    if m == 1.0 || m == -1.0 {
        return Ok(excel_round(n, 0));
    }
    if let Some(v) = try_int_path(n, m) {
        return Ok(v);
    }
    Ok(round_nearest_multiple(n, m))
}

/// First-draft kernel: snap both args to 15 digits, then rem-based
/// half-away. Same sign / zero rules and Excel results; `log10` / `powi`
/// on every call.
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
    Ok(excel_round_15(m * half_away_quot(n / m)))
}

/// `multiple * ROUND(n/multiple, 0)` — contrast only.
///
/// Matches clean cases, but Microsoft's leftover `MROUND(6.05, 0.1)` is
/// `6.0` here would become `6.1` because [`excel_round`] snaps `60.499…`
/// to `60.5`.
#[inline]
pub fn excel_mround_via_round(n: f64, m: f64) -> Result<f64, ExcelError> {
    check_mround(n, m)?;
    if n == 0.0 || m == 0.0 {
        return Ok(0.0);
    }
    Ok(m * excel_round(n / m, 0))
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
    if m == 1.0 || m == -1.0 {
        for i in 0..len {
            let ni = n[i];
            if !ni.is_finite() {
                errs += 1;
                continue;
            }
            if ni == 0.0 {
                out[i] = 0.0;
            } else if (ni > 0.0) != (m > 0.0) {
                errs += 1;
            } else {
                out[i] = excel_round(ni, 0);
            }
        }
        return errs;
    }
    if m < 0.0 {
        for i in 0..len {
            match excel_mround(n[i], m) {
                Ok(v) => out[i] = v,
                Err(_) => errs += 1,
            }
        }
        return errs;
    }
    if is_safe_int(m) && m > 0.0 {
        let mi = m as i64;
        for i in 0..len {
            let ni = n[i];
            if is_safe_int(ni) {
                if ni < 0.0 {
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
    if m.is_finite() && m > 0.0 {
        for i in 0..len {
            let ni = n[i];
            if !ni.is_finite() {
                errs += 1;
                continue;
            }
            if ni == 0.0 {
                out[i] = 0.0;
            } else if ni < 0.0 {
                errs += 1;
            } else {
                out[i] = round_nearest_multiple(ni, m);
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
    // Probe multiple first — decimal `0.1` / `0.2` exit before touching `n`.
    if !is_safe_int(m) || !is_safe_int(n) {
        return None;
    }
    i64_mround(n as i64, m as i64).map(|v| v as f64)
}

#[inline]
fn is_safe_int(x: f64) -> bool {
    x.is_finite() && x == x.trunc() && x.abs() <= SAFE_INT
}

/// Same-sign integer half-away: remainder ≥ half of `|m|` increments `|q|`.
#[inline]
fn i64_mround(n: i64, m: i64) -> Option<i64> {
    if m == 0 || n == 0 {
        return Some(0);
    }
    let q = n / m;
    let r = n % m;
    let q2 = if r.unsigned_abs().saturating_mul(2) >= m.unsigned_abs() {
        q.checked_add(1)?
    } else {
        q
    };
    q2.checked_mul(m)
}

#[inline]
fn round_nearest_multiple(n: f64, m: f64) -> f64 {
    let q = n / m;
    // Cheap 15-digit "already a multiple" test (`MROUND(1.2, 0.1)`).
    if nearly_int(q) {
        return excel_round_15(m * q.round());
    }
    m * half_away_quot(q)
}

/// Half-away of `n/m`. After the sign check the quotient is ≥ 0.
#[inline]
fn half_away_quot(q: f64) -> f64 {
    if q <= 0.0 {
        return 0.0;
    }
    (q + 0.5).floor()
}

#[inline]
fn nearly_int(q: f64) -> bool {
    let r = q.round();
    (q - r).abs() <= 5e-15 * r.abs().max(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::excel_num_eq;
    use crate::excel_round::excel_round;

    fn n(v: Result<f64, ExcelError>) -> f64 {
        v.expect("number")
    }

    fn both(num: f64, mult: f64) -> Result<f64, ExcelError> {
        let fast = excel_mround(num, mult);
        let slow = excel_mround_naive(num, mult);
        match (fast, slow) {
            (Ok(a), Ok(b)) => {
                assert!(
                    excel_num_eq(a, b),
                    "MROUND({num},{mult}) mismatch: fast={a} naive={b}"
                );
                Ok(a)
            }
            (a, b) => {
                assert_eq!(
                    a, b,
                    "MROUND({num},{mult}) mismatch: fast={a:?} naive={b:?}"
                );
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
    fn half_away_not_banker() {
        assert_eq!(n(both(1.5, 1.0)), 2.0);
        assert_eq!(n(both(2.5, 1.0)), 3.0);
        assert_eq!(n(both(-1.5, -1.0)), -2.0);
        assert_eq!(n(both(-2.5, -1.0)), -3.0);
        assert_eq!(n(both(2.5, 5.0)), 5.0);
        assert_eq!(n(both(7.5, 5.0)), 10.0);
        assert_eq!(n(both(1.25, 0.5)), 1.5);
        assert_eq!(n(both(5.0, 2.0)), 6.0);
    }

    #[test]
    fn sign_and_zero_multiple() {
        assert_eq!(both(-10.0, 3.0), Err(ExcelError::Num));
        assert_eq!(both(10.0, -3.0), Err(ExcelError::Num));
        assert_eq!(both(1.3, -0.2), Err(ExcelError::Num));
        assert_eq!(n(both(0.0, 3.0)), 0.0);
        assert_eq!(n(both(0.0, -3.0)), 0.0);
        assert_eq!(n(both(10.0, 0.0)), 0.0);
        assert_eq!(n(both(-10.0, 0.0)), 0.0);
        assert_eq!(n(both(0.0, 0.0)), 0.0);
    }

    #[test]
    fn already_multiple_is_identity() {
        assert_eq!(n(both(9.0, 3.0)), 9.0);
        assert_eq!(n(both(-9.0, -3.0)), -9.0);
        assert_eq!(n(both(12.0, 4.0)), 12.0);
        assert!(excel_num_eq(n(both(1.2, 0.1)), 1.2));
        assert!(excel_num_eq(n(both(1.4, 0.2)), 1.4));
    }

    #[test]
    fn microsoft_leftover_midpoints() {
        // Documented Microsoft limitation: 6.05/0.1 sits at 60.499… so
        // raw half-away stays 60 → 6.0. 7.05/0.1 is an exact 70.5 → 7.1.
        assert_eq!(n(excel_mround(6.05, 0.1)), 6.0);
        assert_eq!(n(excel_mround_naive(6.05, 0.1)), 6.0);
        assert!(excel_num_eq(n(excel_mround(7.05, 0.1)), 7.1));
        assert!(excel_num_eq(n(excel_mround_naive(7.05, 0.1)), 7.1));
        let via = excel_mround_via_round(6.05, 0.1).unwrap();
        assert!(
            excel_num_eq(via, 6.1),
            "ROUND-share of 6.05/0.1 should be 6.1, got {via}"
        );
        assert_ne!(
            n(excel_mround(6.05, 0.1)),
            via,
            "MROUND must not follow ROUND-share on the 6.05 leftover"
        );
    }

    #[test]
    fn shares_round_at_unit_multiple() {
        for n in [-7.5, -2.5, -1.4, 0.0, 1.4, 2.5, 6.05, 7.5] {
            if n >= 0.0 {
                assert_eq!(
                    excel_mround(n, 1.0).unwrap(),
                    excel_round(n, 0),
                    "MROUND({n}, 1) should share ROUND"
                );
            }
            if n <= 0.0 {
                assert_eq!(
                    excel_mround(n, -1.0).unwrap(),
                    excel_round(n, 0),
                    "MROUND({n}, -1) should share ROUND"
                );
            }
        }
    }

    #[test]
    fn more_nearest() {
        assert_eq!(n(both(11.0, 3.0)), 12.0);
        assert_eq!(n(both(10.0, 4.0)), 12.0);
        assert_eq!(n(both(119.0, 25.0)), 125.0);
        assert_eq!(n(both(1.1, 1.0)), 1.0);
        assert_eq!(n(both(0.9, 1.0)), 1.0);
        assert_eq!(n(both(1.1, 3.0)), 0.0);
        assert_eq!(n(both(1.5, 3.0)), 3.0);
        assert_eq!(n(both(-13.0, -2.0)), -14.0);
        assert!(excel_num_eq(n(both(4.42, 0.05)), 4.4));
    }

    #[test]
    fn nonfinite_is_num() {
        assert_eq!(excel_mround(f64::INFINITY, 1.0), Err(ExcelError::Num));
        assert_eq!(excel_mround(1.0, f64::NAN), Err(ExcelError::Num));
        assert_eq!(excel_mround(f64::NEG_INFINITY, -1.0), Err(ExcelError::Num));
    }

    #[test]
    fn integer_path_matches_naive_on_ints() {
        for num in [-20i64, -13, -10, -7, -5, -4, -1, 0, 1, 4, 5, 7, 10, 11, 20] {
            for mult in [-7i64, -5, -3, -2, -1, 1, 2, 3, 5, 7] {
                both(num as f64, mult as f64).ok();
            }
        }
    }

    #[test]
    fn naive_matches_fast_over_clean_grid() {
        for i in -80i32..=80 {
            let num = i as f64 * 0.137 + 0.15;
            for mult in [1.0, 2.0, 5.0, 0.25, 0.5] {
                if num > 0.0 {
                    let _ = both(num, mult);
                } else if num < 0.0 {
                    let _ = both(num, -mult);
                    assert_eq!(both(num, mult), Err(ExcelError::Num));
                } else {
                    let _ = both(num, mult);
                    let _ = both(num, -mult);
                }
            }
        }
    }

    #[test]
    fn slice_matches_scalar() {
        let ns: Vec<f64> = (0..64).map(|i| i as f64 * 1.7 + 1.0).collect();
        let mut out = vec![0.0; ns.len()];
        let errs = excel_mround_slice(&ns, 3.0, &mut out);
        assert_eq!(errs, 0);
        for (num, got) in ns.iter().zip(out.iter()) {
            assert_eq!(*got, excel_mround(*num, 3.0).unwrap());
        }
        let mut naive = vec![0.0; ns.len()];
        excel_mround_slice_naive(&ns, 3.0, &mut naive);
        for (num, got) in ns.iter().zip(naive.iter()) {
            assert_eq!(*got, excel_mround_naive(*num, 3.0).unwrap());
        }
    }

    #[test]
    fn slice_zero_multiple_is_zero() {
        let ns = [0.0, 1.0, -2.0, 10.0];
        let mut out = [7.0; 4];
        let errs = excel_mround_slice(&ns, 0.0, &mut out);
        assert_eq!(errs, 0);
        assert_eq!(out, [0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn slice_opposite_sign_is_num() {
        let ns = [1.0, 2.5, 0.0, -1.0];
        let mut out = [9.0; 4];
        let errs = excel_mround_slice(&ns, -2.0, &mut out);
        assert_eq!(errs, 2);
        assert_eq!(out[2], 0.0);
        assert_eq!(out[3], excel_mround(-1.0, -2.0).unwrap());
    }

    #[test]
    fn slice_unit_multiple_shares_round() {
        let ns = [1.5, 2.5, 6.05, 0.0];
        let mut out = [0.0; 4];
        assert_eq!(excel_mround_slice(&ns, 1.0, &mut out), 0);
        assert_eq!(out, [2.0, 3.0, 6.0, 0.0]);
    }
}
