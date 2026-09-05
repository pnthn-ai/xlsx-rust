//! Time-value-of-money helpers used by worksheet financial functions.
//!
//! Pure math: no workbook, no fixture goldens. `PMT` and `PV` share
//! [`pow_term`]; `FV` / `NPER` are expected to reuse it later.

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

/// Excel / OpenFormula `PV(rate, nper, pmt, [fv], [type])`.
///
/// OpenFormula 6.12.41 (cash-flow sign convention matches Excel: money paid
/// out is negative):
///
/// ```text
/// rate = 0  →  -(fv + pmt · nper)
/// else      →  -(fv + pmt · (1 + r·type) · ((1+r)^n − 1) / r)
///              / (1+r)^n
/// ```
///
/// `type` is the OpenFormula PayType multiplier (0 = end of period, 1 =
/// beginning), used as a real in `(1 + rate * type)` rather than a boolean.
///
/// Domain errors:
/// - `rate = -1` (divide by `(1+rate)^nper = 0`) → `#NUM!`
/// - `nper = 0` is **finite** (`-fv`), unlike [`pmt`]
/// - `rate = 0` and `nper = 0` is **finite** (`-fv`), unlike [`pmt`]
/// - negative^non-integer, overflow, non-finite → `#NUM!`
#[inline]
pub fn pv(rate: f64, nper: f64, pmt: f64, fv: f64, typ: f64) -> Result<f64, ExcelError> {
    pv_inner(rate, nper, pmt, fv, typ, false)
}

/// Baseline `PV`: same domain rules, but always `powf` so `(1+r)^n − 1`
/// cancels on tiny rates. Used only for the hill-climb bench.
#[inline]
pub fn pv_naive(rate: f64, nper: f64, pmt: f64, fv: f64, typ: f64) -> Result<f64, ExcelError> {
    pv_inner(rate, nper, pmt, fv, typ, true)
}

#[inline]
fn pv_inner(
    rate: f64,
    nper: f64,
    pmt: f64,
    fv: f64,
    typ: f64,
    naive: bool,
) -> Result<f64, ExcelError> {
    if !rate.is_finite()
        || !nper.is_finite()
        || !pmt.is_finite()
        || !fv.is_finite()
        || !typ.is_finite()
    {
        return Err(ExcelError::Num);
    }

    if rate == 0.0 {
        return finite(-(fv + pmt * nper));
    }

    let one_plus = 1.0 + rate;
    let type_scale = 1.0 + rate * typ;

    // rate == -1 → (1+rate)^nper = 0^nper → divide-by-zero in the closed form.
    if one_plus == 0.0 {
        return Err(ExcelError::Num);
    }

    if one_plus < 0.0 && nper.fract() != 0.0 {
        return Err(ExcelError::Num);
    }

    let (term, term_m1) = if naive {
        let term = one_plus.powf(nper);
        (term, term - 1.0)
    } else {
        pow_term(one_plus, rate, nper)?
    };
    if !term.is_finite() || !term_m1.is_finite() || term == 0.0 {
        return Err(ExcelError::Num);
    }

    finite(-(fv + pmt * type_scale * term_m1 / rate) / term)
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
            "tvm mismatch: got {actual} expected {expected}"
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
    fn microsoft_annuity_example() {
        // support.microsoft.com PV: ($59,777.15) for 8%/12, 20*12, 500.
        assert_eq!(
            cents(pv(0.08 / 12.0, 20.0 * 12.0, 500.0, 0.0, 0.0).unwrap()),
            -5_977_715
        );
        assert_eq!(
            cents(pv(0.08 / 12.0, 20.0 * 12.0, 500.0, 0.0, 1.0).unwrap()),
            -6_017_566
        );
    }

    #[test]
    fn zero_rate_is_straight_line_pv() {
        assert_eq!(pv(0.0, 10.0, 100.0, 0.0, 0.0).unwrap(), -1000.0);
        assert_eq!(pv(0.0, 10.0, 100.0, 500.0, 0.0).unwrap(), -1500.0);
        // Unlike PMT, nper=0 is finite: PV = -fv.
        assert_eq!(pv(0.0, 0.0, 100.0, 500.0, 0.0).unwrap(), -500.0);
        assert_eq!(pv(0.1, 0.0, 100.0, 500.0, 0.0).unwrap(), -500.0);
    }

    #[test]
    fn rate_minus_one_is_num() {
        assert_eq!(pv(-1.0, 1.0, 100.0, 0.0, 0.0), Err(ExcelError::Num));
        assert_eq!(pv(-1.0, 1.0, 100.0, 50.0, 0.0), Err(ExcelError::Num));
        assert_eq!(pv(-1.0, 1.0, 100.0, 0.0, 1.0), Err(ExcelError::Num));
        assert_eq!(pv(-1.0, 0.0, 100.0, 0.0, 0.0), Err(ExcelError::Num));
        assert_eq!(pv(-1.0, -1.0, 100.0, 0.0, 0.0), Err(ExcelError::Num));
    }

    #[test]
    fn negative_base_integer_nper_pv() {
        close(pv(-2.0, 3.0, 100.0, 0.0, 0.0).unwrap(), 100.0);
        assert_eq!(pv(-2.0, 1.5, 100.0, 0.0, 0.0), Err(ExcelError::Num));
    }

    #[test]
    fn overflow_is_num_pv() {
        assert_eq!(pv(0.5, 2000.0, 100.0, 0.0, 0.0), Err(ExcelError::Num));
    }

    #[test]
    fn small_rate_matches_zero_rate_limit_pv() {
        let tiny = pv(1e-12, 360.0, 100.0, 0.0, 0.0).unwrap();
        let limit = pv(0.0, 360.0, 100.0, 0.0, 0.0).unwrap();
        assert!(
            (tiny - limit).abs() < 1e-6,
            "tiny-rate PV {tiny} should approach {limit}"
        );
        let naive = pv_naive(1e-12, 360.0, 100.0, 0.0, 0.0).unwrap();
        assert!(
            (tiny - limit).abs() <= (naive - limit).abs() + 1e-9,
            "pow_term path should not lose to naive cancel: tiny={tiny} naive={naive} limit={limit}"
        );
    }

    #[test]
    fn pmt_and_pv_are_inverses() {
        let rate = 0.05 / 12.0;
        let loan = 200_000.0;
        let payment = pmt(rate, 360.0, loan, 0.0, 0.0).unwrap();
        close(pv(rate, 360.0, payment, 0.0, 0.0).unwrap(), loan);
        let begin = pmt(rate, 360.0, loan, 0.0, 1.0).unwrap();
        close(pv(rate, 360.0, begin, 0.0, 1.0).unwrap(), loan);
    }

    #[test]
    fn pv_hot_path_many_calls() {
        let start = std::time::Instant::now();
        let mut acc = 0.0f64;
        let rate = 0.05 / 12.0;
        for i in 0..80_000u32 {
            acc += pv(rate, 360.0, 1_000.0 + f64::from(i), 0.0, 0.0).unwrap();
        }
        let elapsed = start.elapsed();
        assert!(acc.is_finite());
        assert!(
            elapsed.as_millis() < 400,
            "80k PV calls took {elapsed:?} (expected a cheap closed form)"
        );
    }
}
