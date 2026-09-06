//! Excel classic `CEILING(number, significance)`.
//!
//! Shared kernel so `calc-core` and `seed-compliant` stay aligned. Does not
//! read fixture goldens — callers pass coerced `f64`s.
//!
//! Desktop Excel / Microsoft CEILING help:
//! - Rounds `number` away from zero to the nearest multiple of
//!   `significance`. If `number` is already an exact multiple, the value
//!   is unchanged.
//! - Both arguments negative: rounds away from zero
//!   (`CEILING(-2.5, -2)` is `-4`).
//! - Negative number + positive significance (Excel 2010+): rounds toward
//!   zero (`CEILING(-2.5, 2)` is `-2`).
//! - Positive number + negative significance: `#NUM!`.
//! - Significance `0` and number `≠ 0` → `#DIV/0!`. `CEILING(0, 0)` is `0`.
//!   Zero number with any nonzero significance is `0` (not a sign clash).
//! - IEEE nearly-multiples such as `CEILING(1.2, 0.1)` stay `1.2` (15-digit
//!   multiple test), not IEEE `ceil` drift to `1.3`.
//!
//! Production is a safe-integer path plus a cheap "already a multiple"
//! probe. The naive path always runs `excel_round_15` (`log10` / `powi`)
//! after IEEE `ceil`, so benches can print before/after.
//!
//! Classic `FLOOR` / `FLOOR.MATH` / `CEILING.MATH` stay in
//! [`crate::floor_ceiling`].

use crate::error::ExcelError;
use crate::value::excel_round_15;

/// Largest magnitude still exactly representable as an integer in `f64`.
const SAFE_INT: f64 = (1i64 << 53) as f64;

/// Production Excel classic `CEILING` kernel.
#[inline]
pub fn excel_ceiling(n: f64, s: f64) -> Result<f64, ExcelError> {
    check_classic(n, s)?;
    if n == 0.0 {
        return Ok(0.0);
    }
    if let Some(v) = try_int_path(n, s) {
        return Ok(v);
    }
    Ok(round_multiple(n, s))
}

/// First-draft kernel: snap both args to 15 digits, IEEE `ceil`, snap again.
/// Same sign / zero rules and Excel results; `log10` / `powi` on every call.
#[inline]
pub fn excel_ceiling_naive(n: f64, s: f64) -> Result<f64, ExcelError> {
    check_classic(n, s)?;
    if n == 0.0 {
        return Ok(0.0);
    }
    let n = excel_round_15(n);
    let s = excel_round_15(s);
    if s == 0.0 {
        return if n == 0.0 {
            Ok(0.0)
        } else {
            Err(ExcelError::Div0)
        };
    }
    Ok(excel_round_15(s * (n / s).ceil()))
}

/// IEEE `s * (n/s).ceil()` with no leftover snap. Contrast tests only
/// (`CEILING(1.2, 0.1)` may drift to `1.3` here).
#[inline]
pub fn excel_ceiling_ieee(n: f64, s: f64) -> Result<f64, ExcelError> {
    check_classic(n, s)?;
    if n == 0.0 {
        return Ok(0.0);
    }
    Ok(s * (n / s).ceil())
}

/// Apply [`excel_ceiling`] to every `n[i]` with a constant significance.
///
/// Returns the number of `#DIV/0!` / `#NUM!` inputs (those slots are left
/// unchanged). Hot path for column-shaped work.
pub fn excel_ceiling_slice(n: &[f64], s: f64, out: &mut [f64]) -> usize {
    let len = n.len().min(out.len());
    let mut errs = 0usize;
    if s == 0.0 {
        for i in 0..len {
            if n[i] == 0.0 {
                out[i] = 0.0;
            } else {
                errs += 1;
            }
        }
        return errs;
    }
    if s < 0.0 {
        for i in 0..len {
            match excel_ceiling(n[i], s) {
                Ok(v) => out[i] = v,
                Err(_) => errs += 1,
            }
        }
        return errs;
    }
    if is_safe_int(s) && s > 0.0 {
        let si = s as i64;
        for i in 0..len {
            let ni = n[i];
            if is_safe_int(ni) {
                let iv = ni as i64;
                if let Some(prod) = i64_div_ceil(iv, si).and_then(|q| q.checked_mul(si)) {
                    out[i] = prod as f64;
                    continue;
                }
            }
            match excel_ceiling(ni, s) {
                Ok(v) => out[i] = v,
                Err(_) => errs += 1,
            }
        }
        return errs;
    }
    // Decimal significance already passed the classic sign/zero checks.
    // Skip the integer probe and `Result` wrap on every element.
    if s.is_finite() && s > 0.0 {
        for i in 0..len {
            let ni = n[i];
            if !ni.is_finite() {
                errs += 1;
                continue;
            }
            if ni == 0.0 {
                out[i] = 0.0;
            } else {
                out[i] = round_multiple(ni, s);
            }
        }
        return errs;
    }
    for i in 0..len {
        match excel_ceiling(n[i], s) {
            Ok(v) => out[i] = v,
            Err(_) => errs += 1,
        }
    }
    errs
}

/// Naive slice baseline matching [`excel_ceiling_naive`].
pub fn excel_ceiling_slice_naive(n: &[f64], s: f64, out: &mut [f64]) -> usize {
    let len = n.len().min(out.len());
    let mut errs = 0usize;
    for i in 0..len {
        match excel_ceiling_naive(n[i], s) {
            Ok(v) => out[i] = v,
            Err(_) => errs += 1,
        }
    }
    errs
}

#[inline]
fn check_classic(n: f64, s: f64) -> Result<(), ExcelError> {
    if !n.is_finite() || !s.is_finite() {
        return Err(ExcelError::Num);
    }
    if s == 0.0 {
        return if n == 0.0 {
            Ok(())
        } else {
            Err(ExcelError::Div0)
        };
    }
    if n > 0.0 && s < 0.0 {
        return Err(ExcelError::Num);
    }
    Ok(())
}

#[inline]
fn try_int_path(n: f64, s: f64) -> Option<f64> {
    // Probe significance first — decimal `0.1` / `0.01` exit before touching `n`.
    if !is_safe_int(s) || !is_safe_int(n) {
        return None;
    }
    let ni = n as i64;
    let si = s as i64;
    let q = i64_div_ceil(ni, si)?;
    Some(q.checked_mul(si)? as f64)
}

#[inline]
fn is_safe_int(x: f64) -> bool {
    x.is_finite() && x == x.trunc() && x.abs() <= SAFE_INT
}

/// Rust `/` truncates toward zero; Excel `CEILING` uses toward +∞ of `n/s`.
#[inline]
fn i64_div_ceil(n: i64, s: i64) -> Option<i64> {
    if s == 0 {
        return None;
    }
    let q = n / s;
    let r = n % s;
    if r != 0 && (n < 0) == (s < 0) {
        q.checked_add(1)
    } else {
        Some(q)
    }
}

#[inline]
fn round_multiple(n: f64, s: f64) -> f64 {
    let q = n / s;
    // Cheap 15-digit "already a multiple" test — avoids `excel_num_eq`'s
    // two `log10` snaps on the hot decimal path (`CEILING(1.2, 0.1)`).
    if nearly_int(q) {
        return excel_round_15(s * q.round());
    }
    s * q.ceil()
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

    fn n(v: Result<f64, ExcelError>) -> f64 {
        v.expect("number")
    }

    fn both(num: f64, sig: f64) -> Result<f64, ExcelError> {
        let fast = excel_ceiling(num, sig);
        let slow = excel_ceiling_naive(num, sig);
        match (fast, slow) {
            (Ok(a), Ok(b)) => {
                assert!(
                    excel_num_eq(a, b),
                    "CEILING({num},{sig}) mismatch: fast={a} naive={b}"
                );
                Ok(a)
            }
            (a, b) => {
                assert_eq!(
                    a, b,
                    "CEILING({num},{sig}) mismatch: fast={a:?} naive={b:?}"
                );
                a
            }
        }
    }

    #[test]
    fn microsoft_ceiling_examples() {
        assert_eq!(n(both(2.5, 1.0)), 3.0);
        assert_eq!(n(both(-2.5, -2.0)), -4.0);
        assert_eq!(n(both(-2.5, 2.0)), -2.0);
        assert!(excel_num_eq(n(both(1.5, 0.1)), 1.5));
        assert!(excel_num_eq(n(both(0.234, 0.01)), 0.24));
        assert!(excel_num_eq(n(both(4.42, 0.05)), 4.45));
    }

    #[test]
    fn sign_and_zero_significance() {
        assert_eq!(both(2.5, -2.0), Err(ExcelError::Num));
        assert_eq!(both(1.0, -1.0), Err(ExcelError::Num));
        assert_eq!(n(both(0.0, -1.0)), 0.0);
        assert_eq!(n(both(0.0, 1.0)), 0.0);
        assert_eq!(n(both(0.0, 0.0)), 0.0);
        assert_eq!(both(15.0, 0.0), Err(ExcelError::Div0));
        assert_eq!(both(-15.0, 0.0), Err(ExcelError::Div0));
    }

    #[test]
    fn toward_zero_vs_away() {
        assert_eq!(n(both(-1.5, 1.0)), -1.0);
        assert_eq!(n(both(-1.99, -1.0)), -2.0);
        assert_eq!(n(both(-0.1, 1.0)), 0.0);
        assert_eq!(n(both(-0.1, -1.0)), -1.0);
        assert_eq!(n(both(-2.0, 5.0)), 0.0);
        assert_eq!(n(both(2.0, 5.0)), 5.0);
        assert_eq!(n(both(-5.4, 1.0)), -5.0);
        assert_eq!(n(both(10.0, 3.0)), 12.0);
        assert_eq!(n(both(36.0, 7.0)), 42.0);
    }

    #[test]
    fn already_multiple_is_identity() {
        assert_eq!(n(both(6.0, 3.0)), 6.0);
        assert_eq!(n(both(-6.0, -3.0)), -6.0);
        assert_eq!(n(both(-6.0, 3.0)), -6.0);
        assert!(excel_num_eq(n(both(1.5, 0.1)), 1.5));
    }

    #[test]
    fn leftover_above_integer_does_not_jump() {
        let leftover = 7.0 + f64::EPSILON * 8.0;
        assert!(leftover > 7.0);
        assert_eq!(n(excel_ceiling(leftover, 1.0)), 7.0);
        assert_eq!(n(excel_ceiling_naive(leftover, 1.0)), 7.0);
        assert_eq!(n(excel_ceiling_ieee(leftover, 1.0)), 8.0);
    }

    #[test]
    fn ieee_nearly_multiple_stays() {
        // 1.2 / 0.1 is 11.999… in IEEE; raw `0.1 * ceil(q)` is 1.200…02.
        // The 15-digit snap keeps the Excel value at 1.2 (and would also
        // catch the opposite leftover, where q sits just above 12 and
        // IEEE `ceil` would jump to 1.3).
        let ieee = excel_ceiling_ieee(1.2, 0.1).unwrap();
        assert_ne!(
            ieee.to_bits(),
            1.2f64.to_bits(),
            "IEEE CEILING(1.2, 0.1) should not be bitwise 1.2, got {ieee}"
        );
        assert_eq!(n(excel_ceiling(1.2, 0.1)), 1.2);
        assert_eq!(n(excel_ceiling_naive(1.2, 0.1)), 1.2);
        assert_eq!(n(excel_ceiling(2.4, 0.2)), 2.4);
        assert_eq!(n(excel_ceiling(0.3, 0.1)), 0.3);
    }

    #[test]
    fn nonfinite_is_num() {
        assert_eq!(excel_ceiling(f64::INFINITY, 1.0), Err(ExcelError::Num));
        assert_eq!(excel_ceiling(1.0, f64::NAN), Err(ExcelError::Num));
        assert_eq!(excel_ceiling(f64::NEG_INFINITY, 1.0), Err(ExcelError::Num));
    }

    #[test]
    fn integer_path_matches_naive_on_ints() {
        for num in [-20i64, -7, -5, -4, -1, 0, 1, 4, 5, 7, 20] {
            for sig in [-7i64, -3, -2, -1, 1, 2, 3, 7] {
                both(num as f64, sig as f64).ok();
            }
        }
    }

    #[test]
    fn naive_matches_fast_over_clean_grid() {
        for i in -80i32..=80 {
            let num = i as f64 * 0.137 + 0.15;
            for sig in [1.0, 2.0, 7.0, 0.25, 0.5] {
                if num > 0.0 {
                    let _ = both(num, sig);
                } else {
                    let _ = both(num, sig);
                    let _ = both(num, -sig);
                }
            }
        }
    }

    #[test]
    fn slice_matches_scalar() {
        let ns: Vec<f64> = (0..64).map(|i| i as f64 * 1.7 - 20.0).collect();
        let mut out = vec![0.0; ns.len()];
        let errs = excel_ceiling_slice(&ns, 3.0, &mut out);
        assert_eq!(errs, 0);
        for (num, got) in ns.iter().zip(out.iter()) {
            assert_eq!(*got, excel_ceiling(*num, 3.0).unwrap());
        }
        let mut naive = vec![0.0; ns.len()];
        excel_ceiling_slice_naive(&ns, 3.0, &mut naive);
        for (num, got) in ns.iter().zip(naive.iter()) {
            assert_eq!(*got, excel_ceiling_naive(*num, 3.0).unwrap());
        }
    }

    #[test]
    fn slice_zero_significance_counts_errors() {
        let ns = [0.0, 1.0, -2.0, 0.0];
        let mut out = [7.0; 4];
        let errs = excel_ceiling_slice(&ns, 0.0, &mut out);
        assert_eq!(errs, 2);
        assert_eq!(out[0], 0.0);
        assert_eq!(out[1], 7.0);
        assert_eq!(out[3], 0.0);
    }

    #[test]
    fn slice_pos_neg_sig_is_num() {
        let ns = [1.0, 2.5, 0.0, -1.0];
        let mut out = [9.0; 4];
        let errs = excel_ceiling_slice(&ns, -2.0, &mut out);
        assert_eq!(errs, 2);
        assert_eq!(out[2], 0.0);
        assert_eq!(out[3], excel_ceiling(-1.0, -2.0).unwrap());
    }
}
