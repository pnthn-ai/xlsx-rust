//! Time-value-of-money helpers used by worksheet financial functions.
//!
//! Pure math: no workbook, no fixture goldens. `PMT` and `NPER` share
//! [`pow_term`] / the OpenFormula PayType multiplier; `PV` / `FV` are later.

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

/// Excel / OpenFormula `NPER(rate, pmt, pv, [fv], [type])`.
///
/// OpenFormula 6.12.29 (cash-flow sign convention matches Excel: money paid
/// out is negative):
///
/// ```text
/// rate = 0  →  -(pv + fv) / pmt
/// else      →  ln( (pmt·(1+r·type) − fv·r)
///                  / (pmt·(1+r·type) + pv·r) ) / ln(1+r)
/// ```
///
/// The production path uses `ln1p` so tiny rates do not cancel:
/// `ln(ratio) = ln1p(ratio−1)` with `ratio−1 = −r·(pv+fv) / den`.
/// `type` is the OpenFormula PayType multiplier (same as [`pmt`]).
///
/// Domain errors:
/// - `rate = 0` and `pmt = 0` → `#DIV/0!` (explicit `/(pmt)` path)
/// - `rate ≤ -1`, non-positive log argument, zero denominator,
///   overflow / non-finite → `#NUM!`
#[inline]
pub fn nper(rate: f64, pmt: f64, pv: f64, fv: f64, typ: f64) -> Result<f64, ExcelError> {
    if !rate.is_finite()
        || !pmt.is_finite()
        || !pv.is_finite()
        || !fv.is_finite()
        || !typ.is_finite()
    {
        return Err(ExcelError::Num);
    }

    if rate == 0.0 {
        if pmt == 0.0 {
            return Err(ExcelError::Div0);
        }
        return signed_zero(finite(-(pv + fv) / pmt));
    }

    // ln(1+rate) is defined only for rate > -1.
    if rate <= -1.0 {
        return Err(ExcelError::Num);
    }

    let type_scale = 1.0 + rate * typ;
    let den = pmt * type_scale + pv * rate;
    if den == 0.0 {
        return Err(ExcelError::Num);
    }

    // ratio - 1 = -r*(pv+fv) / den. ratio <= 0 ⇔ ratio_m1 <= -1.
    let ratio_m1 = -rate * (pv + fv) / den;
    if !ratio_m1.is_finite() || ratio_m1 <= -1.0 {
        return Err(ExcelError::Num);
    }

    let log_ratio = ratio_m1.ln_1p();
    let log_one_plus = rate.ln_1p();
    if log_one_plus == 0.0 {
        // |rate| underflowed ln1p; the zero-rate limit is the honest answer.
        if pmt == 0.0 {
            return Err(ExcelError::Num);
        }
        return signed_zero(finite(-(pv + fv) / pmt));
    }
    signed_zero(finite(log_ratio / log_one_plus))
}

/// Baseline `NPER`: `ln(num/den) / ln(1+rate)` without `ln1p`.
///
/// Same domain errors as [`nper`]. Tiny rates cancel; used only as the
/// before/after bench opponent.
#[inline]
pub fn nper_naive(rate: f64, pmt: f64, pv: f64, fv: f64, typ: f64) -> Result<f64, ExcelError> {
    if !rate.is_finite()
        || !pmt.is_finite()
        || !pv.is_finite()
        || !fv.is_finite()
        || !typ.is_finite()
    {
        return Err(ExcelError::Num);
    }

    if rate == 0.0 {
        if pmt == 0.0 {
            return Err(ExcelError::Div0);
        }
        return signed_zero(finite(-(pv + fv) / pmt));
    }

    let one_plus = 1.0 + rate;
    if one_plus <= 0.0 {
        return Err(ExcelError::Num);
    }

    let type_scale = 1.0 + rate * typ;
    let num = pmt * type_scale - fv * rate;
    let den = pmt * type_scale + pv * rate;
    let ratio = num / den;
    if !ratio.is_finite() || ratio <= 0.0 {
        return Err(ExcelError::Num);
    }
    signed_zero(finite(ratio.ln() / one_plus.ln()))
}

#[inline]
fn signed_zero(r: Result<f64, ExcelError>) -> Result<f64, ExcelError> {
    match r {
        Ok(n) if n == 0.0 => Ok(0.0),
        other => other,
    }
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

    fn nper_close(actual: f64, expected: f64) {
        assert!(
            excel_num_eq(actual, expected),
            "nper mismatch: got {actual} expected {expected}"
        );
    }

    #[test]
    fn microsoft_nper_examples() {
        // support.microsoft.com NPER: 12%/12, pmt=-100, pv=-1000, fv=10000.
        // Published display digits: 59.6738657 / 60.0821229 / -9.57859404.
        nper_close(
            nper(0.12 / 12.0, -100.0, -1000.0, 10_000.0, 1.0).unwrap(),
            59.67386567429462,
        );
        nper_close(
            nper(0.12 / 12.0, -100.0, -1000.0, 10_000.0, 0.0).unwrap(),
            60.08212285376172,
        );
        nper_close(
            nper(0.12 / 12.0, -100.0, -1000.0, 0.0, 0.0).unwrap(),
            -9.578594039813167,
        );
    }

    #[test]
    fn nper_inverts_pmt() {
        let cases = [
            (0.05 / 12.0, 360.0, 200_000.0, 0.0, 0.0),
            (0.05 / 12.0, 360.0, 200_000.0, 0.0, 1.0),
            (0.08, 10.0, 10_000.0, 0.0, 0.0),
            (0.0, 10.0, 1000.0, 500.0, 0.0),
            (0.05, 5.0, 10_000.0, 1000.0, 0.0),
            (0.1, 10.5, 1000.0, 0.0, 0.0),
        ];
        for (rate, periods, pv, fv, typ) in cases {
            let payment = pmt(rate, periods, pv, fv, typ).unwrap();
            let back = nper(rate, payment, pv, fv, typ).unwrap();
            nper_close(back, periods);
        }
    }

    #[test]
    fn zero_rate_is_straight_line_nper() {
        assert_eq!(nper(0.0, -100.0, 1000.0, 0.0, 0.0).unwrap(), 10.0);
        assert_eq!(nper(0.0, -150.0, 1000.0, 500.0, 0.0).unwrap(), 10.0);
        assert_eq!(nper(0.0, 0.0, 1000.0, 0.0, 0.0), Err(ExcelError::Div0));
        assert_eq!(nper(0.0, -100.0, 1000.0, -1000.0, 0.0).unwrap(), 0.0);
    }

    #[test]
    fn nper_domain_errors() {
        // Payment equals interest: never reaches fv.
        assert_eq!(nper(0.1, -10.0, 100.0, 0.0, 0.0), Err(ExcelError::Num));
        // Same-sign cash flows that diverge.
        assert_eq!(nper(0.1, 50.0, 1000.0, 0.0, 0.0), Err(ExcelError::Num));
        // ln(1+rate) undefined.
        assert_eq!(nper(-1.0, -100.0, 1000.0, 0.0, 0.0), Err(ExcelError::Num));
        assert_eq!(nper(-2.0, -100.0, 1000.0, 0.0, 0.0), Err(ExcelError::Num));
        // pmt=0 cannot grow a positive pv toward a larger positive fv.
        assert_eq!(nper(0.1, 0.0, 1000.0, 2000.0, 0.0), Err(ExcelError::Num));
        // pmt=0, fv=0: log of 0.
        assert_eq!(nper(0.1, 0.0, 1000.0, 0.0, 0.0), Err(ExcelError::Num));
    }

    #[test]
    fn nper_pmt_zero_grows_opposite_signs() {
        // Compound 1000 to 2000 at 10% with no periodic payment.
        nper_close(
            nper(0.1, 0.0, -1000.0, 2000.0, 0.0).unwrap(),
            7.272540897341718,
        );
    }

    #[test]
    fn nper_already_at_target_is_zero() {
        assert_eq!(nper(0.1, -50.0, 1000.0, -1000.0, 0.0).unwrap(), 0.0);
    }

    #[test]
    fn nper_tiny_rate_matches_zero_rate_limit() {
        let tiny = nper(1e-12, -100_000.0 / 360.0, 100_000.0, 0.0, 0.0).unwrap();
        let limit = nper(0.0, -100_000.0 / 360.0, 100_000.0, 0.0, 0.0).unwrap();
        assert!(
            (tiny - limit).abs() < 1e-6,
            "tiny-rate NPER {tiny} should approach {limit}"
        );
    }

    #[test]
    fn nper_naive_matches_kernel_on_ordinary_rates() {
        let cases = [
            (0.12 / 12.0, -100.0, -1000.0, 10_000.0, 1.0),
            (0.05 / 12.0, -1073.6432460242763, 200_000.0, 0.0, 0.0),
            (0.1, 0.0, -1000.0, 2000.0, 0.0),
            (-0.05, -80.0, 1000.0, 0.0, 0.0),
        ];
        for (rate, pmt_v, pv, fv, typ) in cases {
            let a = nper(rate, pmt_v, pv, fv, typ).unwrap();
            let b = nper_naive(rate, pmt_v, pv, fv, typ).unwrap();
            nper_close(a, b);
        }
    }

    #[test]
    fn nper_hot_path_many_calls() {
        let start = std::time::Instant::now();
        let mut acc = 0.0f64;
        let rate = 0.05 / 12.0;
        for i in 0..80_000u32 {
            acc += nper(rate, -1_100.0, 200_000.0 + f64::from(i), 0.0, 0.0).unwrap();
        }
        let elapsed = start.elapsed();
        assert!(acc.is_finite());
        assert!(
            elapsed.as_millis() < 400,
            "80k NPER calls took {elapsed:?} (expected a cheap closed form)"
        );
    }
}
