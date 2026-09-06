//! Time-value-of-money helpers used by worksheet financial functions.
//!
//! Pure math: no workbook, no fixture goldens. `PMT` and `NOMINAL` live here;
//! `PV` / `FV` / `NPER` / `EFFECT` are expected to reuse [`pow_term`] later.

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

/// Excel / OpenFormula `NOMINAL(effect_rate, npery)`.
///
/// OpenFormula 6.12.32 (Excel `TRUNC` on `npery` before the root):
///
/// ```text
/// NOMINAL = npery * ((1 + effect_rate)^(1/npery) − 1)
/// ```
///
/// Inverse of `EFFECT`: `NOMINAL(EFFECT(r, n), n) = r`. This kernel does not
/// call `EFFECT`; it evaluates the closed form directly.
///
/// Domain (support.microsoft.com NOMINAL):
/// - non-finite inputs → `#NUM!`
/// - `effect_rate ≤ 0` or truncated `npery < 1` → `#NUM!`
/// - overflow / non-finite result → `#NUM!` (same as `POWER`)
///
/// Production path:
/// - truncated `npery == 1` is the identity `effect_rate`
/// - `npery == 2` is the rationalized `2e / (√(1+e) + 1)`
/// - otherwise [`pow_term`]`(1+e, e, 1/n)` → `n · ((1+e)^{1/n} − 1)`,
///   using `expm1(ln1p(e) / n)` so the root-minus-one does not cancel
#[inline]
pub fn nominal(effect_rate: f64, npery: f64) -> Result<f64, ExcelError> {
    let n = trunc_npery(effect_rate, npery)?;
    if n == 1.0 {
        return finite(effect_rate);
    }
    if n == 2.0 {
        let s = (1.0 + effect_rate).sqrt();
        if !s.is_finite() {
            return Err(ExcelError::Num);
        }
        return finite(2.0 * effect_rate / (s + 1.0));
    }
    let inv = 1.0 / n;
    if !inv.is_finite() {
        return Err(ExcelError::Num);
    }
    let (_, term_m1) = pow_term(1.0 + effect_rate, effect_rate, inv)?;
    finite(n * term_m1)
}

/// Textbook `npery * ((1 + effect).powf(1/npery) - 1)` baseline (same domain
/// as [`nominal`]). Used as the microbench naive path.
#[inline]
pub fn nominal_naive(effect_rate: f64, npery: f64) -> Result<f64, ExcelError> {
    let n = trunc_npery(effect_rate, npery)?;
    let one_plus = 1.0 + effect_rate;
    if !one_plus.is_finite() {
        return Err(ExcelError::Num);
    }
    finite(n * (one_plus.powf(1.0 / n) - 1.0))
}

#[inline]
fn trunc_npery(rate: f64, npery: f64) -> Result<f64, ExcelError> {
    if !rate.is_finite() || !npery.is_finite() {
        return Err(ExcelError::Num);
    }
    if rate <= 0.0 {
        return Err(ExcelError::Num);
    }
    let n = npery.trunc();
    if n < 1.0 {
        return Err(ExcelError::Num);
    }
    Ok(n)
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
    fn nominal_microsoft_quarterly() {
        // support.microsoft.com NOMINAL(0.053543, 4) prints 0.05250032.
        // IEEE powf of the OpenFormula 6.12.32 identity is the golden.
        close_rel(nominal(0.053543, 4.0).unwrap(), 0.05250031986835602);
        close_rel(nominal_naive(0.053543, 4.0).unwrap(), 0.05250031986835602);
    }

    #[test]
    fn nominal_npery_one_is_identity() {
        assert_eq!(nominal(0.1, 1.0).unwrap(), 0.1);
        assert_eq!(nominal(2.5, 1.0).unwrap(), 2.5);
    }

    #[test]
    fn nominal_npery_two_closed_form() {
        // 2e / (√(1+e)+1) at e=0.1025 is exactly 0.1
        close(nominal(0.1025, 2.0).unwrap(), 0.1);
        close(
            nominal(0.1025, 2.0).unwrap(),
            nominal_naive(0.1025, 2.0).unwrap(),
        );
    }

    #[test]
    fn nominal_truncates_npery_toward_zero() {
        close(nominal(0.1, 12.9).unwrap(), nominal(0.1, 12.0).unwrap());
        assert_eq!(nominal(0.1, 1.9).unwrap(), 0.1);
    }

    #[test]
    fn nominal_domain_errors() {
        assert_eq!(nominal(0.0, 12.0), Err(ExcelError::Num));
        assert_eq!(nominal(-0.05, 12.0), Err(ExcelError::Num));
        assert_eq!(nominal(0.05, 0.0), Err(ExcelError::Num));
        assert_eq!(nominal(0.05, 0.9), Err(ExcelError::Num));
        assert_eq!(nominal(0.05, -1.0), Err(ExcelError::Num));
        assert_eq!(nominal(f64::INFINITY, 12.0), Err(ExcelError::Num));
        assert_eq!(nominal(0.05, f64::NAN), Err(ExcelError::Num));
        assert_eq!(nominal(f64::MAX, 2.0), Err(ExcelError::Num));
    }

    #[test]
    fn nominal_common_frequencies_match_naive() {
        for &(e, n) in &[
            (0.053543, 4.0),
            (0.08, 12.0),
            (0.12, 12.0),
            (0.05, 52.0),
            (0.06, 365.0),
            (0.01, 12.0),
            (2.0, 12.0),
        ] {
            close_rel(nominal(e, n).unwrap(), nominal_naive(e, n).unwrap());
        }
    }

    #[test]
    fn nominal_tiny_rate_does_not_cancel() {
        let tiny = nominal(1e-16, 12.0).unwrap();
        assert!(
            tiny > 0.0 && tiny < 1e-15,
            "tiny-rate NOMINAL should stay near the effective, got {tiny}"
        );
        // powf(1+ε, 1/n) − 1 cancels to 0 in IEEE; naive is the contrast.
        assert_eq!(nominal_naive(1e-16, 12.0).unwrap(), 0.0);
    }

    #[test]
    fn nominal_inverts_openformula_effect() {
        // EFFECT(r, n) = (1 + r/n)^n − 1; NOMINAL of that is r.
        // Evaluated here so this workstream does not call EFFECT.
        let r = 0.0525_f64;
        let n = 4.0_f64;
        let effect = (1.0 + r / n).powf(n) - 1.0;
        close_rel(nominal(effect, n).unwrap(), r);
    }

    #[test]
    fn nominal_large_npery_approaches_log() {
        let discrete = nominal(0.05, 1_000_000.0).unwrap();
        let continuous = 0.05f64.ln_1p();
        assert!(
            (discrete - continuous).abs() < 1e-8,
            "NOMINAL(0.05, 1e6)={discrete} should approach ln1p(0.05)={continuous}"
        );
    }

    #[test]
    fn nominal_hot_path_many_calls() {
        let start = std::time::Instant::now();
        let mut acc = 0.0f64;
        for i in 0..80_000u32 {
            let e = 0.01 + f64::from(i) * 1e-8;
            acc += nominal(e, 12.0).unwrap();
        }
        let elapsed = start.elapsed();
        assert!(acc.is_finite());
        assert!(
            elapsed.as_millis() < 400,
            "80k NOMINAL calls took {elapsed:?} (expected a cheap closed form)"
        );
    }
}
