//! Time-value-of-money helpers used by worksheet financial functions.
//!
//! Pure math: no workbook, no fixture goldens. `PMT` and `PDURATION` live
//! here; `PV` / `FV` / `NPER` are expected to reuse [`pow_term`] later.

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

/// Excel / OpenFormula `PDURATION(rate, pv, fv)`.
///
/// OpenFormula 6.12.32 / Microsoft:
///
/// ```text
/// PDURATION = (log(fv) − log(pv)) / log(1 + rate)
///           = log(fv / pv) / log(1 + rate)
/// ```
///
/// Domain (support.microsoft.com PDURATION):
/// - all three arguments must be **positive**
/// - non-finite inputs, zeros, negatives → `#NUM!`
/// - overflow / non-finite result → `#NUM!`
///
/// `fv < pv` is allowed (signed periods). Production path:
/// - `pv == fv` is `0` (no logs)
/// - `fv == pv·(1+rate)` is `1` (no logs)
/// - tiny `|fv/pv − 1|` uses `ln1p((fv−pv)/pv)` so the numerator does not cancel
/// - denominator is always `ln1p(rate)` (accurate for small rates)
#[inline]
pub fn pduration(rate: f64, pv: f64, fv: f64) -> Result<f64, ExcelError> {
    check_pduration_domain(rate, pv, fv)?;
    if pv == fv {
        return Ok(0.0);
    }
    // Exact one-period growth: fv = pv * (1+rate).
    let grown = pv * (1.0 + rate);
    if grown == fv {
        return Ok(1.0);
    }
    let log_rate = rate.ln_1p();
    if !log_rate.is_finite() || log_rate == 0.0 {
        return Err(ExcelError::Num);
    }
    let rel = (fv - pv) / pv;
    let log_ratio = if rel > -0.5 && rel < 1.0 {
        rel.ln_1p()
    } else {
        (fv / pv).ln()
    };
    if !log_ratio.is_finite() {
        return Err(ExcelError::Num);
    }
    finite(log_ratio / log_rate)
}

/// Textbook `(ln(fv) − ln(pv)) / ln(1+rate)` baseline (same domain as
/// [`pduration`]). Used as the microbench naive path.
#[inline]
pub fn pduration_naive(rate: f64, pv: f64, fv: f64) -> Result<f64, ExcelError> {
    check_pduration_domain(rate, pv, fv)?;
    let den = (1.0 + rate).ln();
    if !den.is_finite() || den == 0.0 {
        return Err(ExcelError::Num);
    }
    finite((fv.ln() - pv.ln()) / den)
}

#[inline]
fn check_pduration_domain(rate: f64, pv: f64, fv: f64) -> Result<(), ExcelError> {
    if !rate.is_finite() || !pv.is_finite() || !fv.is_finite() {
        return Err(ExcelError::Num);
    }
    if rate <= 0.0 || pv <= 0.0 || fv <= 0.0 {
        return Err(ExcelError::Num);
    }
    Ok(())
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
            "financial mismatch: got {actual} expected {expected}"
        );
    }

    fn close_rel(actual: f64, expected: f64) {
        let scale = actual.abs().max(expected.abs()).max(1.0);
        assert!(
            (actual - expected).abs() / scale < 1e-12,
            "financial rel mismatch: got {actual} expected {expected}"
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

    #[test]
    fn pduration_microsoft_examples() {
        // support.microsoft.com PDURATION(2.5%, 2000, 2200) prints 3.86
        close_rel(pduration(0.025, 2000.0, 2200.0).unwrap(), 3.859866162622648);
        close_rel(
            pduration(0.025 / 12.0, 1000.0, 1200.0).unwrap(),
            87.60547641937562,
        );
        close_rel(
            pduration_naive(0.025, 2000.0, 2200.0).unwrap(),
            pduration(0.025, 2000.0, 2200.0).unwrap(),
        );
    }

    #[test]
    fn pduration_same_values_is_zero() {
        assert_eq!(pduration(0.08, 1000.0, 1000.0).unwrap(), 0.0);
    }

    #[test]
    fn pduration_one_period_is_identity() {
        assert_eq!(pduration(0.1, 100.0, 110.0).unwrap(), 1.0);
        assert_eq!(pduration(0.05, 1000.0, 1050.0).unwrap(), 1.0);
    }

    #[test]
    fn pduration_integer_periods_match_power() {
        close(pduration(0.1, 100.0, 121.0).unwrap(), 2.0);
        let fv10 = 1000.0 * 1.05f64.powi(10);
        close(pduration(0.05, 1000.0, fv10).unwrap(), 10.0);
    }

    #[test]
    fn pduration_doubling() {
        close_rel(
            pduration(0.05, 100.0, 200.0).unwrap(),
            std::f64::consts::LN_2 / 0.05f64.ln_1p(),
        );
        close_rel(
            pduration(0.05, 100.0, 200.0).unwrap(),
            pduration_naive(0.05, 100.0, 200.0).unwrap(),
        );
    }

    #[test]
    fn pduration_shrink_is_signed() {
        close_rel(
            pduration(0.05, 2000.0, 1000.0).unwrap(),
            -pduration(0.05, 1000.0, 2000.0).unwrap(),
        );
    }

    #[test]
    fn pduration_domain_errors() {
        assert_eq!(pduration(0.0, 1000.0, 2000.0), Err(ExcelError::Num));
        assert_eq!(pduration(-0.05, 1000.0, 2000.0), Err(ExcelError::Num));
        assert_eq!(pduration(0.05, 0.0, 2000.0), Err(ExcelError::Num));
        assert_eq!(pduration(0.05, -1000.0, 2000.0), Err(ExcelError::Num));
        assert_eq!(pduration(0.05, 1000.0, 0.0), Err(ExcelError::Num));
        assert_eq!(pduration(0.05, 1000.0, -2000.0), Err(ExcelError::Num));
        assert_eq!(
            pduration(f64::INFINITY, 1000.0, 2000.0),
            Err(ExcelError::Num)
        );
        assert_eq!(pduration(0.05, f64::NAN, 2000.0), Err(ExcelError::Num));
    }

    #[test]
    fn pduration_tiny_rate_does_not_cancel() {
        let tiny = pduration(1e-16, 100_000.0, 100_001.0).unwrap();
        assert!(
            tiny.is_finite() && tiny > 0.0,
            "tiny-rate PDURATION should stay finite, got {tiny}"
        );
        // ln(1+ε) cancels to 0 in IEEE; naive is the contrast.
        assert_eq!(
            pduration_naive(1e-16, 100_000.0, 100_001.0),
            Err(ExcelError::Num)
        );
    }

    #[test]
    fn pduration_near_equal_matches_naive_when_logs_survive() {
        close_rel(
            pduration(0.05, 1000.0, 1001.0).unwrap(),
            pduration_naive(0.05, 1000.0, 1001.0).unwrap(),
        );
        close_rel(
            pduration(0.08, 10_000.0, 20_000.0).unwrap(),
            pduration_naive(0.08, 10_000.0, 20_000.0).unwrap(),
        );
    }

    #[test]
    fn pduration_hot_path_many_calls() {
        let start = std::time::Instant::now();
        let mut acc = 0.0f64;
        for i in 0..80_000u32 {
            let fv = 1100.0 + f64::from(i);
            acc += pduration(0.025, 1000.0, fv).unwrap();
        }
        let elapsed = start.elapsed();
        assert!(acc.is_finite());
        assert!(
            elapsed.as_millis() < 400,
            "80k PDURATION calls took {elapsed:?} (expected a cheap closed form)"
        );
    }
}
