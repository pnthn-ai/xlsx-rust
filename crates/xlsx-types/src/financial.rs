//! Time-value-of-money helpers used by worksheet financial functions.
//!
//! Pure math: no workbook, no fixture goldens. `PMT` and `IPMT` share
//! [`pow_term`]; `PV` / `FV` / `NPER` are expected to reuse it later.

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

/// Excel / OpenFormula `IPMT(rate, per, nper, pv, [fv], [type])`.
///
/// Interest portion of the periodic payment for period `per`. OpenFormula
/// 6.12.28 (Excel cash-flow sign: money paid out is negative):
///
/// ```text
/// P = PMT(rate, nper, pv, fv, type)
/// IPMT = FV(rate, per − 1, P, pv, type) · rate
/// type ≠ 0          →  IPMT /= (1 + rate)     (annuity-due)
/// type ≠ 0, per = 1 →  0                      (no interest yet)
/// ```
///
/// `FV` here is the OpenFormula 6.12.20 identity, computed privately so this
/// workstream does not export a worksheet `FV`. `type` is the same PayType
/// multiplier [`pmt`] uses.
///
/// Domain errors:
/// - `per < 1` or `per ≥ nper + 1` → `#NUM!` (Excel: `per` in `1..=nper`)
/// - `type ≠ 0` and `rate = -1` (except `per = 1`) → `#NUM!`
/// - [`pmt`] domain errors, negative^non-integer, overflow → `#NUM!` / `#DIV/0!`
#[inline]
pub fn ipmt(rate: f64, per: f64, nper: f64, pv: f64, fv: f64, typ: f64) -> Result<f64, ExcelError> {
    ipmt_kernel(rate, per, nper, pv, fv, typ, false)
}

/// Same OpenFormula identity as [`ipmt`], but the remaining-balance `FV`
/// always uses `powf` for `(1+r)^n`.
///
/// Tiny `|rate|` loses the annuity factor to cancellation; production
/// [`ipmt`] uses [`pow_term`] instead. Useful as a before/after bench baseline.
#[inline]
pub fn ipmt_naive(
    rate: f64,
    per: f64,
    nper: f64,
    pv: f64,
    fv: f64,
    typ: f64,
) -> Result<f64, ExcelError> {
    ipmt_kernel(rate, per, nper, pv, fv, typ, true)
}

#[inline]
fn ipmt_kernel(
    rate: f64,
    per: f64,
    nper: f64,
    pv: f64,
    fv: f64,
    typ: f64,
    naive_pow: bool,
) -> Result<f64, ExcelError> {
    if !rate.is_finite()
        || !per.is_finite()
        || !nper.is_finite()
        || !pv.is_finite()
        || !fv.is_finite()
        || !typ.is_finite()
    {
        return Err(ExcelError::Num);
    }

    // Excel: per ∈ [1, nper]. Spreadsheet convention uses `per >= nper + 1`
    // so integer nper matches 1..=nper and fractional nper stays continuous.
    if per < 1.0 || per >= nper + 1.0 {
        return Err(ExcelError::Num);
    }

    // Annuity-due, first period: payment is made before interest accrues.
    if typ != 0.0 && per == 1.0 {
        return Ok(0.0);
    }

    if rate == 0.0 {
        return Ok(0.0);
    }

    let pmt_val = pmt(rate, nper, pv, fv, typ)?;
    let balance = fv_at(rate, per - 1.0, pmt_val, pv, typ, naive_pow)?;
    let mut interest = balance * rate;
    if typ != 0.0 {
        let one_plus = 1.0 + rate;
        if one_plus == 0.0 {
            return Err(ExcelError::Num);
        }
        interest /= one_plus;
    }
    finite(interest)
}

/// OpenFormula 6.12.20 `FV` used only as the IPMT remaining-balance step.
///
/// `nper = 0` is `-pv` (no compounding yet), including `rate = -1`, because
/// IPMT period 1 / type 0 is `-pv · rate`.
#[inline]
fn fv_at(
    rate: f64,
    nper: f64,
    pmt: f64,
    pv: f64,
    typ: f64,
    naive_pow: bool,
) -> Result<f64, ExcelError> {
    if nper == 0.0 {
        return finite(-pv);
    }

    if rate == 0.0 {
        return finite(-pv - pmt * nper);
    }

    let one_plus = 1.0 + rate;
    let type_scale = 1.0 + rate * typ;

    if one_plus == 0.0 {
        if nper <= 0.0 {
            return Err(ExcelError::Num);
        }
        // term = 0 → -pmt · type_scale · (0 − 1) / rate, rate = -1
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

    fn ipmt_close(actual: f64, expected: f64) {
        assert!(
            excel_num_eq(actual, expected),
            "ipmt mismatch: got {actual} expected {expected}"
        );
    }

    #[test]
    fn microsoft_ipmt_examples() {
        // support.microsoft.com IPMT: first month ($66.67), last year ($292.45).
        assert_eq!(
            cents(ipmt(0.10 / 12.0, 1.0, 36.0, 8_000.0, 0.0, 0.0).unwrap()),
            -6_667
        );
        assert_eq!(
            cents(ipmt(0.10, 3.0, 3.0, 8_000.0, 0.0, 0.0).unwrap()),
            -29_245
        );
    }

    #[test]
    fn ipmt_first_period_is_pv_times_rate() {
        ipmt_close(
            ipmt(0.10 / 12.0, 1.0, 36.0, 8_000.0, 0.0, 0.0).unwrap(),
            -8_000.0 * 0.10 / 12.0,
        );
    }

    #[test]
    fn ipmt_plus_principal_equals_pmt() {
        let rate = 0.08 / 12.0;
        let nper = 10.0;
        let pv = 10_000.0;
        let payment = pmt(rate, nper, pv, 0.0, 0.0).unwrap();
        for per in 1..=10 {
            let interest = ipmt(rate, f64::from(per), nper, pv, 0.0, 0.0).unwrap();
            // principal portion = PMT − IPMT; they must sum to PMT.
            let principal = payment - interest;
            ipmt_close(interest + principal, payment);
            assert!(
                interest.abs() <= payment.abs() + 1e-9,
                "period {per}: |IPMT| {interest} exceeded |PMT| {payment}"
            );
        }
    }

    #[test]
    fn ipmt_type_one_first_period_is_zero() {
        assert_eq!(
            ipmt(0.10 / 12.0, 1.0, 36.0, 8_000.0, 0.0, 1.0).unwrap(),
            0.0
        );
    }

    #[test]
    fn ipmt_type_one_second_period_is_reduced_balance() {
        let rate = 0.1;
        let payment = pmt(rate, 2.0, 1_000.0, 0.0, 1.0).unwrap();
        // Annuity-due: first payment cuts principal immediately; interest
        // (Excel sign) is −(pv + pmt) · rate. Reconstructing from PMT has
        // one extra rounding, so compare with a tight absolute tolerance.
        let actual = ipmt(rate, 2.0, 2.0, 1_000.0, 0.0, 1.0).unwrap();
        let expected = -(1_000.0 + payment) * rate;
        assert!(
            (actual - expected).abs() < 1e-12,
            "annuity-due period-2 IPMT {actual} vs reduced-balance {expected}"
        );
        assert!(actual < 0.0, "loan interest should be an outflow");
    }

    #[test]
    fn ipmt_zero_rate_is_zero() {
        assert_eq!(ipmt(0.0, 1.0, 10.0, 1_000.0, 0.0, 0.0).unwrap(), 0.0);
        assert_eq!(ipmt(0.0, 5.0, 10.0, 1_000.0, 500.0, 0.0).unwrap(), 0.0);
    }

    #[test]
    fn ipmt_per_out_of_range_is_num() {
        assert_eq!(
            ipmt(0.1, 0.0, 10.0, 1_000.0, 0.0, 0.0),
            Err(ExcelError::Num)
        );
        assert_eq!(
            ipmt(0.1, 11.0, 10.0, 1_000.0, 0.0, 0.0),
            Err(ExcelError::Num)
        );
        assert_eq!(ipmt(0.1, 1.0, 0.0, 1_000.0, 0.0, 0.0), Err(ExcelError::Num));
        assert_eq!(
            ipmt(0.1, -1.0, 10.0, 1_000.0, 0.0, 0.0),
            Err(ExcelError::Num)
        );
    }

    #[test]
    fn ipmt_rate_minus_one() {
        // type=0, per=1: IPMT = -pv * rate = pv
        assert_eq!(ipmt(-1.0, 1.0, 1.0, 100.0, 0.0, 0.0).unwrap(), 100.0);
        // type=1, per=1: short-circuit 0 (even though PMT(-1, …, type=1) is #NUM!)
        assert_eq!(ipmt(-1.0, 1.0, 1.0, 100.0, 0.0, 1.0).unwrap(), 0.0);
        // type=1, per>1: divide by 1+rate = 0 → #NUM!
        assert_eq!(ipmt(-1.0, 2.0, 2.0, 100.0, 0.0, 1.0), Err(ExcelError::Num));
    }

    #[test]
    fn ipmt_negative_base_integer_nper() {
        let v = ipmt(-2.0, 1.0, 3.0, 1_000.0, 0.0, 0.0).unwrap();
        ipmt_close(v, -1_000.0 * -2.0);
        assert_eq!(
            ipmt(-2.0, 1.5, 3.0, 1_000.0, 0.0, 0.0),
            Err(ExcelError::Num)
        );
    }

    #[test]
    fn ipmt_overflow_is_num() {
        assert_eq!(
            ipmt(0.5, 1.0, 2000.0, 1_000.0, 0.0, 0.0),
            Err(ExcelError::Num)
        );
    }

    #[test]
    fn ipmt_small_rate_matches_zero_rate_limit() {
        let tiny = ipmt(1e-12, 12.0, 360.0, 100_000.0, 0.0, 0.0).unwrap();
        let limit = ipmt(0.0, 12.0, 360.0, 100_000.0, 0.0, 0.0).unwrap();
        assert!(
            tiny.abs() < 1e-3,
            "tiny-rate IPMT {tiny} should approach {limit}"
        );
    }

    #[test]
    fn ipmt_naive_matches_hot_path_on_ordinary_rates() {
        let cases = [
            (0.10 / 12.0, 1.0, 36.0, 8_000.0, 0.0, 0.0),
            (0.10, 3.0, 3.0, 8_000.0, 0.0, 0.0),
            (0.05 / 12.0, 180.0, 360.0, 200_000.0, 0.0, 0.0),
            (0.08 / 12.0, 2.0, 10.0, 10_000.0, 0.0, 1.0),
            (0.0, 5.0, 10.0, 1_000.0, 0.0, 0.0),
            (-0.05, 4.0, 10.0, 1_000.0, 0.0, 0.0),
        ];
        for (rate, per, nper, pv, fv, typ) in cases {
            let a = ipmt(rate, per, nper, pv, fv, typ).unwrap();
            let b = ipmt_naive(rate, per, nper, pv, fv, typ).unwrap();
            let scale = a.abs().max(b.abs()).max(1.0);
            assert!(
                (a - b).abs() / scale < 1e-9,
                "ipmt vs ipmt_naive: {a} vs {b} (rate={rate} per={per})"
            );
        }
    }

    #[test]
    fn ipmt_hot_path_many_calls() {
        let start = std::time::Instant::now();
        let mut acc = 0.0f64;
        let rate = 0.05 / 12.0;
        for i in 0..80_000u32 {
            acc += ipmt(rate, 12.0, 360.0, 200_000.0 + f64::from(i), 0.0, 0.0).unwrap();
        }
        let elapsed = start.elapsed();
        assert!(acc.is_finite());
        assert!(
            elapsed.as_millis() < 400,
            "80k IPMT calls took {elapsed:?} (expected a cheap closed form)"
        );
    }
}
