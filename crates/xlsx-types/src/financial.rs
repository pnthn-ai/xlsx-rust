//! Time-value-of-money helpers used by worksheet financial functions.
//!
//! Pure math: no workbook, no fixture goldens. `PMT` and `RRI` are closed
//! forms; `PV` / `FV` / `NPER` are expected to reuse [`pow_term`] later.

use crate::error::ExcelError;

/// Excel / OpenFormula `PMT(rate, nper, pv, [fv], [type])`.
///
/// OpenFormula 6.12.36 (cash-flow sign convention matches Excel: money paid
/// out is negative):
///
/// ```text
/// rate = 0  →  -(pv + fv) / nper
/// else      →  -(pv·(1+r)^n + fv) · r
///              / ((1 + r·type) · ((1+r)^n − 1))
/// ```
///
/// `type` is the OpenFormula PayType multiplier (0 = end of period, 1 =
/// beginning), used as a real in `(1 + rate * type)` rather than a boolean.
///
/// Domain errors:
/// - `rate = 0` and `nper = 0` → `#DIV/0!` (explicit `/(nper)` path)
/// - other zero denominators, `0^0`, negative^non-integer, overflow → `#NUM!`
#[inline]
pub fn pmt(rate: f64, nper: f64, pv: f64, fv: f64, typ: f64) -> Result<f64, ExcelError> {
    if !rate.is_finite()
        || !nper.is_finite()
        || !pv.is_finite()
        || !fv.is_finite()
        || !typ.is_finite()
    {
        return Err(ExcelError::Num);
    }

    if rate == 0.0 {
        if nper == 0.0 {
            return Err(ExcelError::Div0);
        }
        return finite(-(pv + fv) / nper);
    }

    let one_plus = 1.0 + rate;
    let type_scale = 1.0 + rate * typ;

    // rate == -1 → (1+rate)^nper = 0^nper.
    if one_plus == 0.0 {
        if nper <= 0.0 {
            return Err(ExcelError::Num);
        }
        if type_scale == 0.0 {
            return Err(ExcelError::Num);
        }
        // term = 0 →  -(fv)·rate / (type_scale · (0-1)) = fv * rate / type_scale
        return finite(fv * rate / type_scale);
    }

    if one_plus < 0.0 && nper.fract() != 0.0 {
        return Err(ExcelError::Num);
    }

    let (term, term_m1) = pow_term(one_plus, rate, nper)?;
    if !term.is_finite() || !term_m1.is_finite() || term_m1 == 0.0 || type_scale == 0.0 {
        return Err(ExcelError::Num);
    }

    finite(-(pv * term + fv) * rate / (type_scale * term_m1))
}

/// Excel / OpenFormula `RRI(nper, pv, fv)`.
///
/// OpenFormula 6.12.45 / Microsoft RRI:
///
/// ```text
/// (fv / pv) ^ (1 / nper) − 1
/// ```
///
/// Production path uses `expm1(ln1p((fv−pv)/pv) / nper)` so a tiny growth
/// (`fv ≈ pv`) does not cancel. Integer `nper == 1` is the simple return
/// `(fv − pv) / pv`.
///
/// Domain errors (`#NUM!`):
/// - non-finite arguments
/// - `nper ≤ 0` (Microsoft / LibreOffice: periods must be positive)
/// - `pv = 0` (division by present value)
/// - `pv` and `fv` have opposite signs and `nper ≠ 1` (LibreOffice Excel
///   compat; `nper = 1` is just `fv/pv − 1`)
/// - overflow of the power (`#NUM!`, same as `POWER`)
#[inline]
pub fn rri(nper: f64, pv: f64, fv: f64) -> Result<f64, ExcelError> {
    rri_checked(nper, pv, fv, rri_expm1)
}

/// Quadratic-ish baseline: `(fv/pv).powf(1/nper) − 1`.
///
/// Same Excel domain as [`rri`]; slower / less accurate when `fv ≈ pv`.
#[inline]
pub fn rri_naive(nper: f64, pv: f64, fv: f64) -> Result<f64, ExcelError> {
    rri_checked(nper, pv, fv, rri_powf)
}

#[inline]
fn rri_checked(
    nper: f64,
    pv: f64,
    fv: f64,
    eval: fn(f64, f64, f64) -> Result<f64, ExcelError>,
) -> Result<f64, ExcelError> {
    if !nper.is_finite() || !pv.is_finite() || !fv.is_finite() {
        return Err(ExcelError::Num);
    }
    if nper <= 0.0 || pv == 0.0 {
        return Err(ExcelError::Num);
    }
    let opposite = (pv > 0.0 && fv < 0.0) || (pv < 0.0 && fv > 0.0);
    if opposite && nper != 1.0 {
        return Err(ExcelError::Num);
    }
    eval(nper, pv, fv)
}

#[inline]
fn rri_expm1(nper: f64, pv: f64, fv: f64) -> Result<f64, ExcelError> {
    if nper == 1.0 {
        return finite((fv - pv) / pv);
    }
    if fv == pv {
        return Ok(0.0);
    }
    if fv == 0.0 {
        return Ok(-1.0);
    }
    // (1 + rel)^(1/nper) − 1 with rel = fv/pv − 1.
    let rel = (fv - pv) / pv;
    let log_term = rel.ln_1p() / nper;
    if !log_term.is_finite() || log_term.abs() > 700.0 {
        return Err(ExcelError::Num);
    }
    finite(log_term.exp_m1())
}

#[inline]
fn rri_powf(nper: f64, pv: f64, fv: f64) -> Result<f64, ExcelError> {
    if nper == 1.0 {
        return finite((fv - pv) / pv);
    }
    let ratio = fv / pv;
    if ratio < 0.0 {
        return Err(ExcelError::Num);
    }
    finite(ratio.powf(1.0 / nper) - 1.0)
}

/// `( (1+rate)^nper , (1+rate)^nper - 1 )` without allocating.
///
/// Small `|rate|` uses `expm1(nper * ln1p(rate))` so the annuity factor does
/// not cancel. Integer `nper` with a non-tiny rate uses `powi` (faster than
/// `powf`, no `ln`/`exp`). Overflow / non-finite follows Excel `POWER` (`#NUM!`).
#[inline]
pub fn pow_term(one_plus: f64, rate: f64, nper: f64) -> Result<(f64, f64), ExcelError> {
    if one_plus > 0.0 {
        let integer_nper = nper.fract() == 0.0 && nper.abs() <= i32::MAX as f64;
        // Accuracy: tiny rates make `(1+r)^n - 1` cancel in the powi path.
        if integer_nper && rate.abs() >= 1e-5 {
            let term = one_plus.powi(nper as i32);
            return Ok((term, term - 1.0));
        }
        let log_term = nper * rate.ln_1p();
        if !log_term.is_finite() || log_term.abs() > 700.0 {
            return Err(ExcelError::Num);
        }
        let term_m1 = log_term.exp_m1();
        Ok((1.0 + term_m1, term_m1))
    } else {
        let term = if nper.fract() == 0.0 && nper.abs() <= i32::MAX as f64 {
            one_plus.powi(nper as i32)
        } else {
            one_plus.powf(nper)
        };
        Ok((term, term - 1.0))
    }
}

#[inline]
fn finite(n: f64) -> Result<f64, ExcelError> {
    if n.is_finite() {
        Ok(n)
    } else {
        Err(ExcelError::Num)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::excel_num_eq;

    fn close(actual: f64, expected: f64) {
        assert!(
            excel_num_eq(actual, expected),
            "pmt mismatch: got {actual} expected {expected}"
        );
    }

    fn cents(n: f64) -> i64 {
        (n * 100.0).round() as i64
    }

    #[test]
    fn microsoft_loan_examples() {
        // support.microsoft.com PMT examples are published to cents.
        assert_eq!(
            cents(pmt(0.08 / 12.0, 10.0, 10_000.0, 0.0, 0.0).unwrap()),
            -103_703
        );
        assert_eq!(
            cents(pmt(0.08 / 12.0, 10.0, 10_000.0, 0.0, 1.0).unwrap()),
            -103_016
        );
        assert_eq!(
            cents(pmt(0.06 / 12.0, 18.0 * 12.0, 0.0, 50_000.0, 0.0).unwrap()),
            -12_908
        );
    }

    #[test]
    fn zero_rate_is_straight_line() {
        assert_eq!(pmt(0.0, 10.0, 1000.0, 0.0, 0.0).unwrap(), -100.0);
        assert_eq!(pmt(0.0, 10.0, 1000.0, 500.0, 0.0).unwrap(), -150.0);
        assert_eq!(pmt(0.0, 0.0, 1000.0, 0.0, 0.0), Err(ExcelError::Div0));
    }

    #[test]
    fn nper_zero_nonzero_rate_is_num() {
        assert_eq!(pmt(0.1, 0.0, 1000.0, 0.0, 0.0), Err(ExcelError::Num));
    }

    #[test]
    fn rate_minus_one() {
        assert_eq!(pmt(-1.0, 1.0, 100.0, 0.0, 0.0).unwrap(), 0.0);
        assert_eq!(pmt(-1.0, 1.0, 100.0, 50.0, 0.0).unwrap(), -50.0);
        assert_eq!(pmt(-1.0, 1.0, 100.0, 0.0, 1.0), Err(ExcelError::Num));
        assert_eq!(pmt(-1.0, 0.0, 100.0, 0.0, 0.0), Err(ExcelError::Num));
        assert_eq!(pmt(-1.0, -1.0, 100.0, 0.0, 0.0), Err(ExcelError::Num));
    }

    #[test]
    fn negative_base_integer_nper() {
        close(pmt(-2.0, 3.0, 1000.0, 0.0, 0.0).unwrap(), 1000.0);
        assert_eq!(pmt(-2.0, 1.5, 1000.0, 0.0, 0.0), Err(ExcelError::Num));
    }

    #[test]
    fn overflow_is_num() {
        assert_eq!(pmt(0.5, 2000.0, 1000.0, 0.0, 0.0), Err(ExcelError::Num));
    }

    #[test]
    fn small_rate_matches_zero_rate_limit() {
        let tiny = pmt(1e-12, 360.0, 100_000.0, 0.0, 0.0).unwrap();
        let limit = pmt(0.0, 360.0, 100_000.0, 0.0, 0.0).unwrap();
        assert!(
            (tiny - limit).abs() < 1e-6,
            "tiny-rate PMT {tiny} should approach {limit}"
        );
    }

    #[test]
    fn pmt_hot_path_many_calls() {
        let start = std::time::Instant::now();
        let mut acc = 0.0f64;
        let rate = 0.05 / 12.0;
        for i in 0..80_000u32 {
            acc += pmt(rate, 360.0, 200_000.0 + f64::from(i), 0.0, 0.0).unwrap();
        }
        let elapsed = start.elapsed();
        assert!(acc.is_finite());
        assert!(
            elapsed.as_millis() < 400,
            "80k PMT calls took {elapsed:?} (expected a cheap closed form)"
        );
    }

    fn both_rri(nper: f64, pv: f64, fv: f64) -> Result<f64, ExcelError> {
        let fast = rri(nper, pv, fv);
        let slow = rri_naive(nper, pv, fv);
        match (fast, slow) {
            (Ok(a), Ok(b)) => {
                let scale = a.abs().max(b.abs()).max(1.0);
                assert!(
                    (a - b).abs() / scale < 1e-12,
                    "naive/fast mismatch: {a} vs {b} for RRI({nper},{pv},{fv})"
                );
                Ok(a)
            }
            (Err(a), Err(b)) => {
                assert_eq!(a, b, "naive/fast error mismatch for RRI({nper},{pv},{fv})");
                Err(a)
            }
            other => panic!("naive/fast Result mismatch: {other:?}"),
        }
    }

    #[test]
    fn rri_microsoft_example() {
        // support.microsoft.com RRI: published 0.0009933 (7 digits).
        let n = both_rri(96.0, 10_000.0, 11_000.0).unwrap();
        assert!((n - 0.0009933).abs() < 5e-8, "got {n}");
        close(n, (0.1f64.ln_1p() / 96.0).exp_m1());
    }

    #[test]
    fn rri_simple_return_and_zeros() {
        close(both_rri(1.0, 100.0, 110.0).unwrap(), 0.1);
        close(both_rri(1.0, -100.0, 110.0).unwrap(), -2.1);
        assert_eq!(both_rri(10.0, 100.0, 100.0).unwrap(), 0.0);
        assert_eq!(both_rri(10.0, 100.0, 0.0).unwrap(), -1.0);
        close(both_rri(10.0, 1000.0, 2000.0).unwrap(), 0.07177346253629316);
    }

    #[test]
    fn rri_same_sign_negative() {
        close(
            both_rri(96.0, -10_000.0, -11_000.0).unwrap(),
            both_rri(96.0, 10_000.0, 11_000.0).unwrap(),
        );
    }

    #[test]
    fn rri_domain_errors() {
        assert_eq!(both_rri(0.0, 100.0, 110.0), Err(ExcelError::Num));
        assert_eq!(both_rri(-1.0, 100.0, 110.0), Err(ExcelError::Num));
        assert_eq!(both_rri(10.0, 0.0, 110.0), Err(ExcelError::Num));
        assert_eq!(both_rri(2.0, 100.0, -110.0), Err(ExcelError::Num));
        assert_eq!(both_rri(0.5, 100.0, -100.0), Err(ExcelError::Num));
        assert_eq!(both_rri(f64::NAN, 100.0, 110.0), Err(ExcelError::Num));
        assert_eq!(both_rri(1e-10, 1.0, 2.0), Err(ExcelError::Num));
    }

    #[test]
    fn rri_tiny_growth_stays_accurate() {
        // powf(1+ε, 1/n) − 1 cancels; ln1p/expm1 keeps the leading digits.
        let n = rri(360.0, 100_000.0, 100_001.0).unwrap();
        let identity = (1e-5f64.ln_1p() / 360.0).exp_m1();
        assert!(
            (n - identity).abs() <= 1e-18,
            "tiny-growth RRI {n} should match {identity}"
        );
    }

    #[test]
    fn rri_hot_path_many_calls() {
        let start = std::time::Instant::now();
        let mut acc = 0.0f64;
        for i in 0..80_000u32 {
            let fv = 11_000.0 + f64::from(i);
            acc += rri(96.0, 10_000.0, fv).unwrap();
        }
        let elapsed = start.elapsed();
        assert!(acc.is_finite());
        assert!(
            elapsed.as_millis() < 400,
            "80k RRI calls took {elapsed:?} (expected a cheap closed form)"
        );
    }
}
