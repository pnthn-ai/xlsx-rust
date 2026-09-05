//! Time-value-of-money helpers used by worksheet financial functions.
//!
//! Pure math: no workbook, no fixture goldens. `PMT` and `PPMT` share
//! [`pow_term`]; `PV` / `FV` / `NPER` are later workstreams.

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

/// Excel / OpenFormula `PPMT(rate, per, nper, pv, [fv], [type])`.
///
/// OpenFormula 6.12.37 (principal portion of a period payment). Cash-flow
/// sign matches Excel: money paid out is negative.
///
/// ```text
/// PPMT = PMT(rate, nper, pv, fv, type) − IPMT(rate, per, nper, pv, fv, type)
///
/// IPMT (OpenFormula 6.12.26 / LibreOffice GetZw remaining balance):
///   type ≠ 0 and per = 1  →  0   (due immediately; no interest yet)
///   else                  →  FV(rate, per−1, PMT, pv, type) · rate
///                            / (1 + rate·type)
///
/// per < 1 or per > nper  →  #NUM!
/// ```
///
/// `type` is the OpenFormula PayType multiplier (same as [`pmt`]). Remaining
/// balance uses [`pow_term`] so tiny rates do not cancel. Worksheet `IPMT` /
/// `FV` are later workstreams — they are not dispatched from here.
///
/// Domain errors follow [`pmt`] (`#DIV/0!` never surfaces: a zero `nper`
/// fails the `per ∈ [1, nper]` check first) plus `#NUM!` when `per` is
/// outside `1…nper`, overflow, `0^k` with `k < 0`, or negative^non-integer.
#[inline]
pub fn ppmt(rate: f64, per: f64, nper: f64, pv: f64, fv: f64, typ: f64) -> Result<f64, ExcelError> {
    ppmt_kernel(rate, per, nper, pv, fv, typ, false)
}

/// `powf` baseline of [`ppmt`] for the kernel bench (no `expm1` / `ln1p`).
#[inline]
pub fn ppmt_naive(
    rate: f64,
    per: f64,
    nper: f64,
    pv: f64,
    fv: f64,
    typ: f64,
) -> Result<f64, ExcelError> {
    ppmt_kernel(rate, per, nper, pv, fv, typ, true)
}

#[inline]
fn ppmt_kernel(
    rate: f64,
    per: f64,
    nper: f64,
    pv: f64,
    fv: f64,
    typ: f64,
    naive: bool,
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
    // Excel: Per must be in the range 1 to nper.
    if per < 1.0 || per > nper {
        return Err(ExcelError::Num);
    }

    let payment = pmt_kernel(rate, nper, pv, fv, typ, naive)?;
    if typ != 0.0 && per == 1.0 {
        return Ok(payment);
    }
    let interest = ipmt_from_payment(rate, per, payment, pv, typ, naive)?;
    finite(payment - interest)
}

/// Interest on the remaining balance after `per - 1` payments.
#[inline]
fn ipmt_from_payment(
    rate: f64,
    per: f64,
    payment: f64,
    pv: f64,
    typ: f64,
    naive: bool,
) -> Result<f64, ExcelError> {
    let type_scale = 1.0 + rate * typ;
    if type_scale == 0.0 {
        return Err(ExcelError::Num);
    }
    let remaining = tvm_fv(rate, per - 1.0, payment, pv, typ, naive)?;
    finite(remaining * rate / type_scale)
}

/// Excel `FV(rate, nper, pmt, pv, type)` used only as a remaining-balance
/// helper for `PPMT`. Not a worksheet export.
#[inline]
fn tvm_fv(
    rate: f64,
    nper: f64,
    pmt_val: f64,
    pv: f64,
    typ: f64,
    naive: bool,
) -> Result<f64, ExcelError> {
    if rate == 0.0 {
        return finite(-(pv + pmt_val * nper));
    }

    let one_plus = 1.0 + rate;
    let type_scale = 1.0 + rate * typ;

    if one_plus == 0.0 {
        if nper < 0.0 {
            return Err(ExcelError::Num);
        }
        if nper == 0.0 {
            return finite(-pv);
        }
        return finite(pmt_val * type_scale / rate);
    }

    if one_plus < 0.0 && nper.fract() != 0.0 {
        return Err(ExcelError::Num);
    }

    let (term, term_m1) = if naive {
        pow_term_powf(one_plus, nper)
    } else {
        pow_term(one_plus, rate, nper)?
    };
    if !term.is_finite() || !term_m1.is_finite() {
        return Err(ExcelError::Num);
    }
    finite(-(pv * term + pmt_val * type_scale * term_m1 / rate))
}

#[inline]
fn pmt_kernel(
    rate: f64,
    nper: f64,
    pv: f64,
    fv: f64,
    typ: f64,
    naive: bool,
) -> Result<f64, ExcelError> {
    if !naive {
        return pmt(rate, nper, pv, fv, typ);
    }
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
    if one_plus == 0.0 {
        if nper <= 0.0 || type_scale == 0.0 {
            return Err(ExcelError::Num);
        }
        return finite(fv * rate / type_scale);
    }
    if one_plus < 0.0 && nper.fract() != 0.0 {
        return Err(ExcelError::Num);
    }
    let (term, term_m1) = pow_term_powf(one_plus, nper);
    if !term.is_finite() || !term_m1.is_finite() || term_m1 == 0.0 || type_scale == 0.0 {
        return Err(ExcelError::Num);
    }
    finite(-(pv * term + fv) * rate / (type_scale * term_m1))
}

#[inline]
fn pow_term_powf(one_plus: f64, nper: f64) -> (f64, f64) {
    let term = one_plus.powf(nper);
    (term, term - 1.0)
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

    #[test]
    fn ppmt_microsoft_examples() {
        // support.microsoft.com PPMT examples are published to cents.
        assert_eq!(
            cents(ppmt(0.10 / 12.0, 1.0, 24.0, 2_000.0, 0.0, 0.0).unwrap()),
            -7_562
        );
        assert_eq!(
            cents(ppmt(0.08, 10.0, 10.0, 200_000.0, 0.0, 0.0).unwrap()),
            -2_759_805
        );
    }

    #[test]
    fn ppmt_plus_ipmt_is_pmt() {
        let rate = 0.05 / 12.0;
        let payment = pmt(rate, 360.0, 200_000.0, 0.0, 0.0).unwrap();
        for per in [1.0, 2.0, 180.0, 360.0] {
            let principal = ppmt(rate, per, 360.0, 200_000.0, 0.0, 0.0).unwrap();
            let interest = payment - principal;
            close(principal + interest, payment);
        }
        let begin = pmt(rate, 360.0, 200_000.0, 0.0, 1.0).unwrap();
        assert_eq!(
            ppmt(rate, 1.0, 360.0, 200_000.0, 0.0, 1.0).unwrap(),
            begin,
            "type=1 period 1 is all principal"
        );
    }

    #[test]
    fn ppmt_zero_rate_is_straight_line() {
        assert_eq!(ppmt(0.0, 3.0, 10.0, 1000.0, 0.0, 0.0).unwrap(), -100.0);
        assert_eq!(ppmt(0.0, 1.0, 10.0, 1000.0, 500.0, 0.0).unwrap(), -150.0);
    }

    #[test]
    fn ppmt_per_out_of_range_is_num() {
        assert_eq!(ppmt(0.1, 0.0, 10.0, 1000.0, 0.0, 0.0), Err(ExcelError::Num));
        assert_eq!(ppmt(0.1, 0.5, 10.0, 1000.0, 0.0, 0.0), Err(ExcelError::Num));
        assert_eq!(
            ppmt(0.1, 11.0, 10.0, 1000.0, 0.0, 0.0),
            Err(ExcelError::Num)
        );
        assert_eq!(ppmt(0.1, 1.0, 0.0, 1000.0, 0.0, 0.0), Err(ExcelError::Num));
        assert_eq!(ppmt(0.0, 1.0, 0.0, 1000.0, 0.0, 0.0), Err(ExcelError::Num));
    }

    #[test]
    fn ppmt_rate_minus_one() {
        assert_eq!(ppmt(-1.0, 1.0, 1.0, 100.0, 0.0, 0.0).unwrap(), -100.0);
        assert_eq!(ppmt(-1.0, 1.0, 1.0, 100.0, 0.0, 1.0), Err(ExcelError::Num));
    }

    #[test]
    fn ppmt_negative_base_integer_nper() {
        close(ppmt(-2.0, 1.0, 3.0, 1000.0, 0.0, 0.0).unwrap(), -1000.0);
        assert_eq!(ppmt(-2.0, 1.0, 1.5, 1000.0, 0.0, 0.0), Err(ExcelError::Num));
    }

    #[test]
    fn ppmt_overflow_is_num() {
        assert_eq!(
            ppmt(0.5, 1.0, 2000.0, 1000.0, 0.0, 0.0),
            Err(ExcelError::Num)
        );
    }

    #[test]
    fn ppmt_matches_naive_on_ordinary_rates() {
        let cases = [
            (0.10 / 12.0, 1.0, 24.0, 2_000.0, 0.0, 0.0),
            (0.08, 10.0, 10.0, 200_000.0, 0.0, 0.0),
            (0.05 / 12.0, 180.0, 360.0, 200_000.0, 0.0, 1.0),
            (0.0, 3.0, 10.0, 1_000.0, 500.0, 0.0),
        ];
        for (rate, per, nper, pv, fv, typ) in cases {
            let a = ppmt(rate, per, nper, pv, fv, typ).unwrap();
            let b = ppmt_naive(rate, per, nper, pv, fv, typ).unwrap();
            let scale = a.abs().max(b.abs()).max(1.0);
            assert!(
                (a - b).abs() / scale < 1e-12,
                "ppmt vs naive: {a} vs {b} rate={rate} per={per}"
            );
        }
    }

    #[test]
    fn ppmt_small_rate_stays_near_zero_rate_limit() {
        let tiny = ppmt(1e-12, 1.0, 360.0, 100_000.0, 0.0, 0.0).unwrap();
        let limit = ppmt(0.0, 1.0, 360.0, 100_000.0, 0.0, 0.0).unwrap();
        assert!(
            (tiny - limit).abs() < 1e-6,
            "tiny-rate PPMT {tiny} should approach {limit}"
        );
    }

    #[test]
    fn ppmt_hot_path_many_calls() {
        let start = std::time::Instant::now();
        let mut acc = 0.0f64;
        let rate = 0.05 / 12.0;
        for i in 0..80_000u32 {
            let per = 1.0 + f64::from(i % 360);
            acc += ppmt(rate, per, 360.0, 200_000.0 + f64::from(i), 0.0, 0.0).unwrap();
        }
        let elapsed = start.elapsed();
        assert!(acc.is_finite());
        assert!(
            elapsed.as_millis() < 400,
            "80k PPMT calls took {elapsed:?} (expected a cheap closed form)"
        );
    }
}
