//! Time-value-of-money helpers used by worksheet financial functions.
//!
//! Pure math: no workbook, no fixture goldens. `PMT` and `CUMPRINC` share
//! [`pow_term`]; the `PV` / `FV` / `NPER` worksheet functions are later
//! workstreams. [`fv`] here is the TVM helper `CUMPRINC` needs, not `FV()`.

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

/// Excel / OpenFormula `FV(rate, nper, pmt, [pv], [type])` TVM helper.
///
/// ```text
/// rate = 0  →  -(pv + pmt · nper)
/// else      →  -(pv · (1+r)^n + pmt · (1 + r·type) · ((1+r)^n − 1) / r)
/// ```
///
/// Same domain rules as [`pmt`] (`#NUM!` on overflow / non-finite / `0^0` /
/// negative^non-integer). Not the worksheet `FV` function.
#[inline]
pub fn fv(rate: f64, nper: f64, pmt: f64, pv: f64, typ: f64) -> Result<f64, ExcelError> {
    if !rate.is_finite()
        || !nper.is_finite()
        || !pmt.is_finite()
        || !pv.is_finite()
        || !typ.is_finite()
    {
        return Err(ExcelError::Num);
    }

    if rate == 0.0 {
        return finite(-(pv + pmt * nper));
    }

    let one_plus = 1.0 + rate;
    let type_scale = 1.0 + rate * typ;

    if one_plus == 0.0 {
        if nper <= 0.0 {
            return Err(ExcelError::Num);
        }
        // term = 0 →  -pmt · type_scale · (0 − 1) / rate = pmt · type_scale / rate
        return finite(pmt * type_scale / rate);
    }

    if one_plus < 0.0 && nper.fract() != 0.0 {
        return Err(ExcelError::Num);
    }

    let (term, term_m1) = pow_term(one_plus, rate, nper)?;
    if !term.is_finite() || !term_m1.is_finite() {
        return Err(ExcelError::Num);
    }

    finite(-(pv * term + pmt * type_scale * term_m1 / rate))
}

/// Excel `CUMPRINC(rate, nper, pv, start_period, end_period, type)`.
///
/// Cumulative principal paid between `start_period` and `end_period` (cash-flow
/// sign: paying down a loan is negative). Identity (LibreOffice Analysis AddIn
/// / OpenFormula 6.12.10, `fv = 0`):
///
/// ```text
/// pmt = PMT(rate, nper, pv, 0, type)
/// type = 0 →  Σ_k PPMT = FV(rate, start−1, pmt, pv, 0) − FV(rate, end, pmt, pv, 0)
/// type = 1 →  PPMT(1) = pmt; later PPMT(k) = pmt − (FV(k−2, type=1) − pmt)·rate
///             (closed geometric sum of those FV terms)
/// ```
///
/// Domain (`#NUM!`):
/// - `rate ≤ 0`, `nper ≤ 0`, or `pv ≤ 0` (Microsoft CUMPRINC)
/// - `start_period` / `end_period` truncate toward 0; need `1 ≤ start ≤ end`
///   and `end ≤ nper`
/// - `type` truncates toward 0 and must be `0` or `1`
///
/// Closed form: two [`fv`] evaluations (`type = 0`) or one geometric sum
/// (`type = 1`). [`cumprinc_naive`] is the period loop for benches.
#[inline]
pub fn cumprinc(
    rate: f64,
    nper: f64,
    pv: f64,
    start_period: f64,
    end_period: f64,
    typ: f64,
) -> Result<f64, ExcelError> {
    let args = cumprinc_args(rate, nper, pv, start_period, end_period, typ)?;
    cumprinc_closed(args)
}

/// Period-by-period `Σ PPMT` baseline (same domain / result as [`cumprinc`]).
#[inline]
pub fn cumprinc_naive(
    rate: f64,
    nper: f64,
    pv: f64,
    start_period: f64,
    end_period: f64,
    typ: f64,
) -> Result<f64, ExcelError> {
    let args = cumprinc_args(rate, nper, pv, start_period, end_period, typ)?;
    cumprinc_loop(args)
}

#[derive(Clone, Copy)]
struct CumprincArgs {
    rate: f64,
    nper: f64,
    pv: f64,
    start: u32,
    end: u32,
    typ: u8,
}

#[inline]
fn cumprinc_args(
    rate: f64,
    nper: f64,
    pv: f64,
    start_period: f64,
    end_period: f64,
    typ: f64,
) -> Result<CumprincArgs, ExcelError> {
    if !rate.is_finite()
        || !nper.is_finite()
        || !pv.is_finite()
        || !start_period.is_finite()
        || !end_period.is_finite()
        || !typ.is_finite()
    {
        return Err(ExcelError::Num);
    }
    if rate <= 0.0 || nper <= 0.0 || pv <= 0.0 {
        return Err(ExcelError::Num);
    }
    let start = trunc_period(start_period)?;
    let end = trunc_period(end_period)?;
    if start > end || f64::from(end) > nper {
        return Err(ExcelError::Num);
    }
    let typ = trunc_pay_type(typ)?;
    Ok(CumprincArgs {
        rate,
        nper,
        pv,
        start,
        end,
        typ,
    })
}

#[inline]
fn trunc_period(n: f64) -> Result<u32, ExcelError> {
    let t = n.trunc();
    if t < 1.0 || t > f64::from(u32::MAX) {
        return Err(ExcelError::Num);
    }
    Ok(t as u32)
}

#[inline]
fn trunc_pay_type(n: f64) -> Result<u8, ExcelError> {
    let t = n.trunc();
    if t == 0.0 {
        Ok(0)
    } else if t == 1.0 {
        Ok(1)
    } else {
        Err(ExcelError::Num)
    }
}

#[inline]
fn cumprinc_closed(a: CumprincArgs) -> Result<f64, ExcelError> {
    let payment = pmt(a.rate, a.nper, a.pv, 0.0, f64::from(a.typ))?;
    if a.typ == 0 {
        let before = fv(a.rate, f64::from(a.start) - 1.0, payment, a.pv, 0.0)?;
        let after = fv(a.rate, f64::from(a.end), payment, a.pv, 0.0)?;
        finite(before - after)
    } else {
        cumprinc_due_closed(a.rate, a.pv, payment, a.start, a.end)
    }
}

/// Annuity-due (`type = 1`) closed form of the LibreOffice PPMT loop.
#[inline]
fn cumprinc_due_closed(
    rate: f64,
    pv: f64,
    payment: f64,
    start: u32,
    end: u32,
) -> Result<f64, ExcelError> {
    if start == 1 && end == 1 {
        return finite(payment);
    }
    let q = 1.0 + rate;
    let a = if start == 1 { 0 } else { start - 2 };
    let n_terms = f64::from(end - 2 - a + 1);
    let sum_g = sum_fv_due(rate, pv, payment, f64::from(a), n_terms)?;
    let mut acc = n_terms * payment * q - rate * sum_g;
    if start == 1 {
        acc += payment;
    }
    finite(acc)
}

/// `Σ_{j=a}^{a+n−1} FV(rate, j, pmt, pv, 1)` via a geometric series.
#[inline]
fn sum_fv_due(rate: f64, pv: f64, payment: f64, a: f64, n: f64) -> Result<f64, ExcelError> {
    let q = 1.0 + rate;
    let coeff = payment * q / rate;
    let (q_a, _) = pow_term(q, rate, a)?;
    let (_, q_n_m1) = pow_term(q, rate, n)?;
    if !q_a.is_finite() || !q_n_m1.is_finite() {
        return Err(ExcelError::Num);
    }
    finite(n * coeff - (pv + coeff) * q_a * q_n_m1 / rate)
}

/// LibreOffice `getCumprinc` period loop (semantic reference / bench baseline).
fn cumprinc_loop(a: CumprincArgs) -> Result<f64, ExcelError> {
    let payment = pmt(a.rate, a.nper, a.pv, 0.0, f64::from(a.typ))?;
    let mut acc = 0.0;
    let mut i = a.start;
    if i == 1 {
        acc = if a.typ == 0 {
            payment + a.pv * a.rate
        } else {
            payment
        };
        i = 2;
    }
    while i <= a.end {
        let ipmt = if a.typ == 0 {
            fv(a.rate, f64::from(i - 1), payment, a.pv, 0.0)? * a.rate
        } else {
            (fv(a.rate, f64::from(i - 2), payment, a.pv, 1.0)? - payment) * a.rate
        };
        acc += payment - ipmt;
        if !acc.is_finite() {
            return Err(ExcelError::Num);
        }
        i += 1;
    }
    finite(acc)
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
    fn cumprinc_microsoft_examples() {
        // support.microsoft.com CUMPRINC: 9% / 30y / $125,000, monthly.
        // Published to cents / 8 decimals; compare cents like PMT.
        assert_eq!(
            cents(cumprinc(0.09 / 12.0, 360.0, 125_000.0, 13.0, 24.0, 0.0).unwrap()),
            -93_411
        );
        assert_eq!(
            cents(cumprinc(0.09 / 12.0, 360.0, 125_000.0, 1.0, 1.0, 0.0).unwrap()),
            -6_828
        );
    }

    #[test]
    fn cumprinc_libreoffice_help() {
        // help.libreoffice.org: 5.5% / 36 months / 15000, periods 10–18, type=0.
        assert_eq!(
            cents(cumprinc(0.055 / 12.0, 36.0, 15_000.0, 10.0, 18.0, 0.0).unwrap()),
            -366_974
        );
    }

    #[test]
    fn cumprinc_full_term_is_minus_pv() {
        close(
            cumprinc(0.05 / 12.0, 60.0, 10_000.0, 1.0, 60.0, 0.0).unwrap(),
            -10_000.0,
        );
        close(
            cumprinc(0.05 / 12.0, 60.0, 10_000.0, 1.0, 60.0, 1.0).unwrap(),
            -10_000.0,
        );
    }

    #[test]
    fn cumprinc_closed_matches_naive() {
        let cases = [
            (0.09 / 12.0, 360.0, 125_000.0, 13.0, 24.0, 0.0),
            (0.09 / 12.0, 360.0, 125_000.0, 1.0, 1.0, 0.0),
            (0.09 / 12.0, 360.0, 125_000.0, 13.0, 24.0, 1.0),
            (0.09 / 12.0, 360.0, 125_000.0, 1.0, 1.0, 1.0),
            (0.09 / 12.0, 360.0, 125_000.0, 1.0, 12.0, 1.0),
            (0.09 / 12.0, 360.0, 125_000.0, 12.0, 24.0, 1.0),
            (0.05 / 12.0, 360.0, 200_000.0, 349.0, 360.0, 0.0),
            (0.08, 10.0, 10_000.0, 1.0, 3.0, 1.0),
        ];
        for (rate, nper, pv, start, end, typ) in cases {
            let fast = cumprinc(rate, nper, pv, start, end, typ).unwrap();
            let slow = cumprinc_naive(rate, nper, pv, start, end, typ).unwrap();
            let scale = fast.abs().max(slow.abs()).max(1.0);
            assert!(
                (fast - slow).abs() / scale < 1e-12,
                "closed {fast} vs naive {slow} ({rate},{nper},{pv},{start},{end},{typ})"
            );
        }
    }

    #[test]
    fn cumprinc_truncates_periods_and_type() {
        let a = cumprinc(0.09 / 12.0, 360.0, 125_000.0, 13.0, 24.0, 0.0).unwrap();
        let b = cumprinc(0.09 / 12.0, 360.0, 125_000.0, 13.9, 24.1, 0.4).unwrap();
        close(a, b);
        close(
            cumprinc(0.09 / 12.0, 360.0, 125_000.0, 1.0, 1.0, 1.9).unwrap(),
            cumprinc(0.09 / 12.0, 360.0, 125_000.0, 1.0, 1.0, 1.0).unwrap(),
        );
    }

    #[test]
    fn cumprinc_domain_errors() {
        assert_eq!(
            cumprinc(0.0, 360.0, 125_000.0, 1.0, 1.0, 0.0),
            Err(ExcelError::Num)
        );
        assert_eq!(
            cumprinc(-0.01, 360.0, 125_000.0, 1.0, 1.0, 0.0),
            Err(ExcelError::Num)
        );
        assert_eq!(
            cumprinc(0.09 / 12.0, 0.0, 125_000.0, 1.0, 1.0, 0.0),
            Err(ExcelError::Num)
        );
        assert_eq!(
            cumprinc(0.09 / 12.0, 360.0, 0.0, 1.0, 1.0, 0.0),
            Err(ExcelError::Num)
        );
        assert_eq!(
            cumprinc(0.09 / 12.0, 360.0, -125_000.0, 1.0, 1.0, 0.0),
            Err(ExcelError::Num)
        );
        assert_eq!(
            cumprinc(0.09 / 12.0, 360.0, 125_000.0, 0.0, 1.0, 0.0),
            Err(ExcelError::Num)
        );
        assert_eq!(
            cumprinc(0.09 / 12.0, 360.0, 125_000.0, 24.0, 13.0, 0.0),
            Err(ExcelError::Num)
        );
        assert_eq!(
            cumprinc(0.09 / 12.0, 360.0, 125_000.0, 1.0, 361.0, 0.0),
            Err(ExcelError::Num)
        );
        assert_eq!(
            cumprinc(0.09 / 12.0, 360.0, 125_000.0, 1.0, 1.0, 2.0),
            Err(ExcelError::Num)
        );
    }

    #[test]
    fn cumprinc_hot_path_many_calls() {
        let start = std::time::Instant::now();
        let mut acc = 0.0f64;
        let rate = 0.05 / 12.0;
        for i in 0..80_000u32 {
            acc += cumprinc(rate, 360.0, 200_000.0 + f64::from(i), 1.0, 360.0, 0.0).unwrap();
        }
        let elapsed = start.elapsed();
        assert!(acc.is_finite());
        assert!(
            elapsed.as_millis() < 400,
            "80k CUMPRINC(1..360) calls took {elapsed:?} (expected a cheap closed form)"
        );
    }
}
