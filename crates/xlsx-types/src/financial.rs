//! Time-value-of-money helpers used by worksheet financial functions.
//!
//! Pure math: no workbook, no fixture goldens. `PMT` and `FV` share
//! [`pow_term`]; `PV` / `NPER` are expected to reuse it later.

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

/// Excel / OpenFormula `FV(rate, nper, pmt, [pv], [type])`.
///
/// OpenFormula 6.12.20 (cash-flow sign convention matches Excel: money paid
/// out is negative):
///
/// ```text
/// rate = 0  →  -pv − pmt · nper
/// else      →  -pv · (1+r)^n − pmt · (1 + r·type) · ((1+r)^n − 1) / r
/// ```
///
/// `type` is the OpenFormula PayType multiplier (0 = end of period, 1 =
/// beginning), used as a real in `(1 + rate * type)` rather than a boolean.
///
/// Unlike [`pmt`], `nper = 0` is not a domain error: the result is `-pv`
/// (and `rate = 0` never divides, so that path is also `-pv`).
///
/// Domain errors:
/// - `rate = -1` and `nper ≤ 0` → `#NUM!` (`0^0` / `1/0`, same as `POWER`)
/// - negative^non-integer, overflow, non-finite → `#NUM!`
#[inline]
pub fn fv(rate: f64, nper: f64, pmt: f64, pv: f64, typ: f64) -> Result<f64, ExcelError> {
    fv_kernel(rate, nper, pmt, pv, typ, false)
}

/// Same OpenFormula identity as [`fv`], but always uses `powf` for `(1+r)^n`.
///
/// Useful as a before/after bench baseline. Tiny `|rate|` loses the annuity
/// factor to cancellation; production [`fv`] uses [`pow_term`] instead.
#[inline]
pub fn fv_naive(rate: f64, nper: f64, pmt: f64, pv: f64, typ: f64) -> Result<f64, ExcelError> {
    fv_kernel(rate, nper, pmt, pv, typ, true)
}

#[inline]
fn fv_kernel(
    rate: f64,
    nper: f64,
    pmt: f64,
    pv: f64,
    typ: f64,
    naive_pow: bool,
) -> Result<f64, ExcelError> {
    if !rate.is_finite()
        || !nper.is_finite()
        || !pmt.is_finite()
        || !pv.is_finite()
        || !typ.is_finite()
    {
        return Err(ExcelError::Num);
    }

    if rate == 0.0 {
        return finite(-pv - pmt * nper);
    }

    let one_plus = 1.0 + rate;
    let type_scale = 1.0 + rate * typ;

    // rate == -1 → (1+rate)^nper = 0^nper.
    if one_plus == 0.0 {
        if nper <= 0.0 {
            return Err(ExcelError::Num);
        }
        // term = 0 →  -pmt · type_scale · (0 − 1) / rate, rate = -1
        //           = -pmt · type_scale
        return finite(-pmt * type_scale);
    }

    if one_plus < 0.0 && nper.fract() != 0.0 {
        return Err(ExcelError::Num);
    }

    let (term, term_m1) = if naive_pow {
        let term = one_plus.powf(nper);
        (term, term - 1.0)
    } else {
        pow_term(one_plus, rate, nper)?
    };
    if !term.is_finite() || !term_m1.is_finite() {
        return Err(ExcelError::Num);
    }

    finite(-pv * term - pmt * type_scale * term_m1 / rate)
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

    fn fv_close(actual: f64, expected: f64) {
        assert!(
            excel_num_eq(actual, expected),
            "fv mismatch: got {actual} expected {expected}"
        );
    }

    #[test]
    fn microsoft_fv_examples() {
        // support.microsoft.com FV examples are published to cents.
        assert_eq!(
            cents(fv(0.06 / 12.0, 10.0, -200.0, -500.0, 1.0).unwrap()),
            258_140
        );
        assert_eq!(
            cents(fv(0.12 / 12.0, 12.0, -1000.0, 0.0, 0.0).unwrap()),
            1_268_250
        );
        assert_eq!(
            cents(fv(0.11 / 12.0, 35.0, -2000.0, 0.0, 1.0).unwrap()),
            8_284_625
        );
    }

    #[test]
    fn fv_zero_rate_is_straight_line() {
        assert_eq!(fv(0.0, 10.0, -100.0, 0.0, 0.0).unwrap(), 1000.0);
        assert_eq!(fv(0.0, 10.0, -100.0, -500.0, 0.0).unwrap(), 1500.0);
        // nper=0 is -pv, not a domain error (unlike PMT).
        assert_eq!(fv(0.0, 0.0, -100.0, 1000.0, 0.0).unwrap(), -1000.0);
        assert_eq!(fv(0.1, 0.0, -100.0, 1000.0, 0.0).unwrap(), -1000.0);
    }

    #[test]
    fn fv_rate_minus_one() {
        assert_eq!(fv(-1.0, 1.0, -100.0, 50.0, 0.0).unwrap(), 100.0);
        assert_eq!(fv(-1.0, 1.0, -100.0, 0.0, 1.0).unwrap(), 0.0);
        assert_eq!(fv(-1.0, 0.0, -100.0, 50.0, 0.0), Err(ExcelError::Num));
        assert_eq!(fv(-1.0, -1.0, -100.0, 50.0, 0.0), Err(ExcelError::Num));
    }

    #[test]
    fn fv_negative_base_integer_nper() {
        fv_close(fv(-2.0, 3.0, -100.0, 0.0, 0.0).unwrap(), 100.0);
        assert_eq!(fv(-2.0, 1.5, -100.0, 0.0, 0.0), Err(ExcelError::Num));
    }

    #[test]
    fn fv_overflow_is_num() {
        assert_eq!(fv(0.5, 2000.0, -1.0, 0.0, 0.0), Err(ExcelError::Num));
    }

    #[test]
    fn fv_small_rate_matches_zero_rate_limit() {
        let tiny = fv(1e-12, 360.0, -100.0, 0.0, 0.0).unwrap();
        let limit = fv(0.0, 360.0, -100.0, 0.0, 0.0).unwrap();
        // First-order term is n(n-1)/2 · rate · |pmt| ≈ 6.5e-6 here.
        assert!(
            (tiny - limit).abs() < 1e-5,
            "tiny-rate FV {tiny} should approach {limit}"
        );
    }

    #[test]
    fn fv_inverts_pmt_on_a_fully_amortized_loan() {
        let rate = 0.08 / 12.0;
        let payment = pmt(rate, 10.0, 10_000.0, 0.0, 0.0).unwrap();
        let residual = fv(rate, 10.0, payment, 10_000.0, 0.0).unwrap();
        assert!(
            residual.abs() < 1e-8,
            "FV after PMT should clear the loan, got {residual}"
        );
    }

    #[test]
    fn fv_naive_matches_hot_path_on_ordinary_rates() {
        let cases = [
            (0.06 / 12.0, 10.0, -200.0, -500.0, 1.0),
            (0.12 / 12.0, 12.0, -1000.0, 0.0, 0.0),
            (0.05 / 12.0, 360.0, -1_000.0, -200_000.0, 0.0),
            (0.0, 10.0, -100.0, -500.0, 0.0),
            (-0.05, 10.0, -100.0, -1000.0, 0.0),
        ];
        for (rate, nper, pmt_v, pv, typ) in cases {
            let a = fv(rate, nper, pmt_v, pv, typ).unwrap();
            let b = fv_naive(rate, nper, pmt_v, pv, typ).unwrap();
            let scale = a.abs().max(b.abs()).max(1.0);
            assert!(
                (a - b).abs() / scale < 1e-9,
                "fv vs fv_naive: {a} vs {b} (rate={rate})"
            );
        }
    }

    #[test]
    fn fv_hot_path_many_calls() {
        let start = std::time::Instant::now();
        let mut acc = 0.0f64;
        let rate = 0.05 / 12.0;
        for i in 0..80_000u32 {
            acc += fv(rate, 360.0, -1_000.0, -(200_000.0 + f64::from(i)), 0.0).unwrap();
        }
        let elapsed = start.elapsed();
        assert!(acc.is_finite());
        assert!(
            elapsed.as_millis() < 400,
            "80k FV calls took {elapsed:?} (expected a cheap closed form)"
        );
    }
}
