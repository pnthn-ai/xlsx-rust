//! Excel classic `FLOOR(number, significance)`.
//!
//! Shared kernel so `calc-core` and `seed-compliant` stay aligned. Does not
//! read fixture goldens — callers pass coerced `f64`s.
//!
//! Desktop Excel (Microsoft `FLOOR`, Excel 2010+):
//! - Rounds `number` down (toward −∞ of `number/significance`) to a multiple
//!   of `significance`.
//! - Positive number + negative significance → `#NUM!`.
//! - Zero significance → `#DIV/0!`, except `FLOOR(0, 0)` → `0`.
//! - Negative number + positive significance is allowed (Excel 2010+):
//!   away from zero (toward −∞). Both negative: toward zero.
//! - Zero number is not a sign clash (`FLOOR(0, -1)` is `0`).
//! - `FLOOR(n, 1)` matches [`excel_int`](crate::excel_int) leftover snap
//!   (ten `+ 0.1` → `1`; `0.3-0.1-0.2` → `0`).
//!
//! Production: sign/zero checks, then `excel_int` when significance is `1`,
//! else a safe-integer `i64` path or a cheap nearly-integer multiple test.
//! The naive path always runs IEEE `s * (n/s).floor()` then `excel_round_15`
//! (`log10` / `powi`) so benches can print before/after.

use crate::error::ExcelError;
use crate::excel_int::excel_int;
use crate::value::excel_round_15;

/// Largest magnitude still exactly representable as an integer in `f64`.
const SAFE_INT: f64 = (1i64 << 53) as f64;

/// Production Excel classic `FLOOR` kernel.
#[inline]
pub fn excel_floor(n: f64, s: f64) -> Result<f64, ExcelError> {
    check_classic(n, s)?;
    if n == 0.0 {
        return Ok(0.0);
    }
    // Significance 1 is `INT` — share the leftover-bump kernel.
    if s == 1.0 {
        return Ok(excel_int(n));
    }
    if let Some(v) = try_int_path(n, s) {
        return Ok(v);
    }
    Ok(round_multiple(n, s))
}

/// First-draft kernel: sign/zero checks, then always `excel_round_15` after
/// IEEE `s * (n/s).floor()`. Same results on clean inputs; `log10` / `powi`
/// on every call.
#[inline]
pub fn excel_floor_naive(n: f64, s: f64) -> Result<f64, ExcelError> {
    check_classic(n, s)?;
    if n == 0.0 {
        return Ok(0.0);
    }
    Ok(excel_round_15(s * (n / s).floor()))
}

/// Apply [`excel_floor`] to every `n[i]` with a constant significance.
///
/// Returns the number of `#DIV/0!` / `#NUM!` inputs (those slots are left
/// unchanged). Hot path for column-shaped work.
pub fn excel_floor_slice(n: &[f64], s: f64, out: &mut [f64]) -> usize {
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
    if s == 1.0 {
        // Packed `INT` walk; non-finite numbers are classic `#NUM!`.
        for i in 0..len {
            if n[i].is_finite() {
                out[i] = excel_int(n[i]);
            } else {
                errs += 1;
            }
        }
        return errs;
    }
    if s < 0.0 {
        for i in 0..len {
            match excel_floor(n[i], s) {
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
                if let Some(prod) = i64_div_floor(iv, si).and_then(|q| q.checked_mul(si)) {
                    out[i] = prod as f64;
                    continue;
                }
            }
            match excel_floor(ni, s) {
                Ok(v) => out[i] = v,
                Err(_) => errs += 1,
            }
        }
        return errs;
    }
    // Decimal significance: `s` already passed the classic checks. Skip the
    // integer probe and `Result` wrap on every element.
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
        match excel_floor(n[i], s) {
            Ok(v) => out[i] = v,
            Err(_) => errs += 1,
        }
    }
    errs
}

/// Naive slice baseline matching [`excel_floor_naive`].
pub fn excel_floor_slice_naive(n: &[f64], s: f64, out: &mut [f64]) -> usize {
    let len = n.len().min(out.len());
    let mut errs = 0usize;
    for i in 0..len {
        match excel_floor_naive(n[i], s) {
            Ok(v) => out[i] = v,
            Err(_) => errs += 1,
        }
    }
    errs
}

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

fn try_int_path(n: f64, s: f64) -> Option<f64> {
    // Probe significance first — decimal `0.1` / `0.01` exit before touching `n`.
    if !is_safe_int(s) || !is_safe_int(n) {
        return None;
    }
    let ni = n as i64;
    let si = s as i64;
    let q = i64_div_floor(ni, si)?;
    Some(q.checked_mul(si)? as f64)
}

fn is_safe_int(x: f64) -> bool {
    x.is_finite() && x == x.trunc() && x.abs() <= SAFE_INT
}

/// Rust `/` truncates toward zero; Excel `FLOOR` uses toward −∞ of `n/s`.
fn i64_div_floor(n: i64, s: i64) -> Option<i64> {
    if s == 0 {
        return None;
    }
    let q = n / s;
    let r = n % s;
    if r != 0 && (n < 0) != (s < 0) {
        q.checked_sub(1)
    } else {
        Some(q)
    }
}

fn round_multiple(n: f64, s: f64) -> f64 {
    let q = n / s;
    // Cheap 15-digit "already a multiple" test — avoids `excel_num_eq`'s
    // two `log10` snaps on the hot decimal path (`FLOOR(1.2, 0.1)`).
    if nearly_int(q) {
        return excel_round_15(s * q.round());
    }
    s * q.floor()
}

fn nearly_int(q: f64) -> bool {
    let r = q.round();
    (q - r).abs() <= 5e-15 * r.abs().max(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::excel_int::excel_int;
    use crate::value::excel_num_eq;

    fn n(v: Result<f64, ExcelError>) -> f64 {
        v.expect("number")
    }

    #[test]
    fn microsoft_floor_examples() {
        assert_eq!(n(excel_floor(3.7, 2.0)), 2.0);
        assert_eq!(n(excel_floor(-2.5, -2.0)), -2.0);
        assert_eq!(excel_floor(2.5, -2.0), Err(ExcelError::Num));
        assert!(excel_num_eq(n(excel_floor(1.58, 0.1)), 1.5));
        assert!(excel_num_eq(n(excel_floor(0.234, 0.01)), 0.23));
    }

    #[test]
    fn sign_and_zero_significance() {
        assert_eq!(excel_floor(15.0, 0.0), Err(ExcelError::Div0));
        assert_eq!(excel_floor(-15.0, 0.0), Err(ExcelError::Div0));
        assert_eq!(n(excel_floor(0.0, 0.0)), 0.0);
        assert_eq!(n(excel_floor(0.0, 1.0)), 0.0);
        assert_eq!(n(excel_floor(0.0, -1.0)), 0.0);
        assert_eq!(n(excel_floor(-1.5, 1.0)), -2.0);
        assert_eq!(n(excel_floor(-2.5, 2.0)), -4.0);
        assert_eq!(n(excel_floor(-1.99, -1.0)), -1.0);
        assert_eq!(n(excel_floor(45.0, 50.0)), 0.0);
    }

    #[test]
    fn already_multiple_and_nickel() {
        assert_eq!(n(excel_floor(6.0, 3.0)), 6.0);
        assert_eq!(n(excel_floor(-6.0, 3.0)), -6.0);
        assert!(excel_num_eq(n(excel_floor(1.2, 0.1)), 1.2));
        assert!(excel_num_eq(n(excel_floor(8.26, 0.05)), 8.25));
    }

    #[test]
    fn significance_one_shares_int() {
        let samples = [
            -20.0,
            -8.9,
            -1.5,
            -0.01,
            0.0,
            0.5,
            1.9,
            8.9,
            40909.75,
            1.0 / 3.0,
            (0..10).fold(0.0, |a, _| a + 0.1),
            0.3 - 0.1 - 0.2,
        ];
        for n in samples {
            let a = excel_int(n);
            let b = excel_floor(n, 1.0).expect("FLOOR(n,1)");
            assert!(excel_num_eq(a, b), "INT({n})={a} vs FLOOR({n},1)={b}");
        }
    }

    #[test]
    fn fifteen_digit_leftover_snaps() {
        let tenths = (0..10).fold(0.0, |a, _| a + 0.1);
        assert!(
            tenths < 1.0,
            "IEEE 0.1×10 leftover should sit below 1: {tenths}"
        );
        assert_eq!(n(excel_floor(tenths, 1.0)), 1.0);
        assert_eq!(n(excel_floor(tenths, 0.1)), 1.0);

        let sub = 0.3 - 0.1 - 0.2;
        assert!(sub < 0.0, "IEEE leftover should be negative: {sub}");
        assert_eq!(n(excel_floor(sub, 1.0)), 0.0);
        assert_eq!(n(excel_floor(sub, 0.1)), 0.0);
        // Naive IEEE floor of a tiny negative leftover is −significance.
        assert_eq!(n(excel_floor_naive(sub, 1.0)), -1.0);
    }

    #[test]
    fn integer_path_matches_ieee_on_ints() {
        for n in [-20i64, -7, -5, -4, -1, 0, 1, 4, 5, 7, 20] {
            for s in [-7i64, -3, -2, -1, 1, 2, 3, 7] {
                let nf = n as f64;
                let sf = s as f64;
                let a = excel_floor(nf, sf);
                let b = excel_floor_naive(nf, sf);
                assert_eq!(a, b, "FLOOR({n},{s})");
            }
        }
    }

    #[test]
    fn slice_matches_scalar() {
        let ns: Vec<f64> = (0..64).map(|i| i as f64 * 1.7 - 20.0).collect();
        for s in [1.0, 3.0, 0.1, -2.0] {
            let mut out = vec![0.0; ns.len()];
            let errs = excel_floor_slice(&ns, s, &mut out);
            let expect_err = ns.iter().filter(|n| excel_floor(**n, s).is_err()).count();
            assert_eq!(errs, expect_err, "slice errs s={s}");
            for (n, got) in ns.iter().zip(out.iter()) {
                if let Ok(want) = excel_floor(*n, s) {
                    assert_eq!(*got, want, "slice FLOOR({n},{s})");
                }
            }
        }
        let mut naive = vec![0.0; ns.len()];
        excel_floor_slice_naive(&ns, 3.0, &mut naive);
        for (n, got) in ns.iter().zip(naive.iter()) {
            assert_eq!(*got, excel_floor_naive(*n, 3.0).unwrap());
        }
    }

    #[test]
    fn large_magnitude_integers() {
        assert_eq!(n(excel_floor(SAFE_INT, 1.0)), SAFE_INT);
        let big = 1e20;
        assert_eq!(n(excel_floor(big, 1.0)), big);
    }

    #[test]
    fn nonfinite_is_num() {
        assert_eq!(excel_floor(f64::INFINITY, 1.0), Err(ExcelError::Num));
        assert_eq!(excel_floor(1.0, f64::NAN), Err(ExcelError::Num));
    }
}
