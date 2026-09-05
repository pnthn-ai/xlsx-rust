//! Time-value-of-money helpers used by worksheet financial functions.
//!
//! Pure math: no workbook, no fixture goldens. `PMT` and `CUMIPMT` share
//! [`pow_term`]; `PV` / `FV` / `NPER` / `IPMT` are expected to reuse it later.

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

/// Excel / OpenFormula `CUMIPMT(rate, nper, pv, start_period, end_period, type)`.
///
/// Cumulative interest paid on a loan between `start_period` and
/// `end_period` (inclusive). OpenFormula 6.12.12 is the sum of
/// `IPMT(rate, i, nper, pv, 0, type)` for each integer `i` in that span.
/// `fv` is always 0. This workstream does **not** export worksheet `IPMT`
/// or `FV`; remaining-balance `FV` is a private helper.
///
/// Microsoft / OpenFormula domain (integers via toward-zero truncate):
///
/// - `rate ≤ 0`, `nper ≤ 0`, or `pv ≤ 0` → `#NUM!`
/// - `start < 1`, `end < 1`, `start > end`, or `end > nper` → `#NUM!`
/// - `type` not `0` or `1` → `#NUM!`
///
/// The hot path is a closed form of that IPMT sum (two [`pow_term`]s), not
/// a period loop. [`cumipmt_naive`] is the loop + `powf` baseline.
#[inline]
pub fn cumipmt(
    rate: f64,
    nper: f64,
    pv: f64,
    start: f64,
    end: f64,
    typ: f64,
) -> Result<f64, ExcelError> {
    cumipmt_kernel(rate, nper, pv, start, end, typ, false)
}

/// Same OpenFormula 6.12.12 identity as [`cumipmt`], but sums one IPMT per
/// period and uses `powf` for each remaining-balance `FV`.
///
/// Production [`cumipmt`] is O(1) via [`pow_term`]. Useful as a before/after
/// bench baseline (full-loan spans make the loop the expensive part).
#[inline]
pub fn cumipmt_naive(
    rate: f64,
    nper: f64,
    pv: f64,
    start: f64,
    end: f64,
    typ: f64,
) -> Result<f64, ExcelError> {
    cumipmt_kernel(rate, nper, pv, start, end, typ, true)
}

#[inline]
fn cumipmt_kernel(
    rate: f64,
    nper: f64,
    pv: f64,
    start: f64,
    end: f64,
    typ: f64,
    naive: bool,
) -> Result<f64, ExcelError> {
    if !rate.is_finite()
        || !nper.is_finite()
        || !pv.is_finite()
        || !start.is_finite()
        || !end.is_finite()
        || !typ.is_finite()
    {
        return Err(ExcelError::Num);
    }

    let nper = nper.trunc();
    let start = start.trunc();
    let end = end.trunc();
    let typ = typ.trunc();

    if rate <= 0.0 || nper <= 0.0 || pv <= 0.0 {
        return Err(ExcelError::Num);
    }
    if start < 1.0 || end < 1.0 || start > end || end > nper {
        return Err(ExcelError::Num);
    }
    if typ != 0.0 && typ != 1.0 {
        return Err(ExcelError::Num);
    }

    if naive {
        cumipmt_loop(rate, nper, pv, start, end, typ)
    } else {
        cumipmt_closed(rate, nper, pv, start, end, typ)
    }
}

/// Closed form of Σ IPMT for `fv = 0` after the CUMIPMT domain checks
/// (`rate > 0`, integer `start..=end`, `type ∈ {0,1}`).
///
/// ```text
/// P = PMT(rate, nper, pv, 0, type)
/// type = 0:
///   S  = (1+r)^{start−1} · ((1+r)^n − 1) / r
///   Σ  = P·n − S·(P + pv·r)
/// type = 1 (period 1 is 0; sum from max(start,2)):
///   S1 = (1+r)^{s−2} · ((1+r)^n − 1) / r
///   Σ  = P·n − S1·(pv·r + P·(1+r))
/// ```
#[inline]
fn cumipmt_closed(
    rate: f64,
    nper: f64,
    pv: f64,
    start: f64,
    end: f64,
    typ: f64,
) -> Result<f64, ExcelError> {
    let one_plus = 1.0 + rate;
    if typ != 0.0 {
        let s = if start < 2.0 { 2.0 } else { start };
        if s > end {
            return Ok(0.0);
        }
        let pmt_val = pmt(rate, nper, pv, 0.0, typ)?;
        let n_terms = end - s + 1.0;
        let (term_s, _) = pow_term(one_plus, rate, s - 2.0)?;
        let (_, term_n_m1) = pow_term(one_plus, rate, n_terms)?;
        if !term_s.is_finite() || !term_n_m1.is_finite() {
            return Err(ExcelError::Num);
        }
        let s1 = term_s * term_n_m1 / rate;
        // P·(n − S1·(1+r)) − S1·pv·r avoids (P + pv·r) rounding.
        finite(pmt_val * (n_terms - s1 * one_plus) - s1 * pv * rate)
    } else {
        // Period 1 interest is exactly −pv·rate (no PMT cancellation).
        if start == 1.0 && end == 1.0 {
            return finite(-pv * rate);
        }
        let pmt_val = pmt(rate, nper, pv, 0.0, typ)?;
        let n_terms = end - start + 1.0;
        let (term_s, _) = pow_term(one_plus, rate, start - 1.0)?;
        let (_, term_n_m1) = pow_term(one_plus, rate, n_terms)?;
        if !term_s.is_finite() || !term_n_m1.is_finite() {
            return Err(ExcelError::Num);
        }
        let s_geom = term_s * term_n_m1 / rate;
        // P·(n − S) − S·pv·r keeps period-1 / S≈1 cases aligned with −pv·r.
        finite(pmt_val * (n_terms - s_geom) - s_geom * pv * rate)
    }
}

/// Per-period IPMT sum (OpenFormula 6.12.28, `fv = 0`) using `powf` FV.
#[inline]
fn cumipmt_loop(
    rate: f64,
    nper: f64,
    pv: f64,
    start: f64,
    end: f64,
    typ: f64,
) -> Result<f64, ExcelError> {
    let pmt_val = pmt(rate, nper, pv, 0.0, typ)?;
    let mut sum = 0.0;
    let mut per = start;
    while per <= end {
        let interest = if typ != 0.0 && per == 1.0 {
            0.0
        } else {
            let balance = fv_at(rate, per - 1.0, pmt_val, pv, typ)?;
            let mut interest = balance * rate;
            if typ != 0.0 {
                interest /= 1.0 + rate;
            }
            interest
        };
        if !interest.is_finite() {
            return Err(ExcelError::Num);
        }
        sum += interest;
        per += 1.0;
    }
    finite(sum)
}

/// OpenFormula 6.12.20 `FV` used only as the per-period remaining-balance
/// step inside [`cumipmt_naive`]. Always `powf` (the naive baseline).
#[inline]
fn fv_at(rate: f64, nper: f64, pmt: f64, pv: f64, typ: f64) -> Result<f64, ExcelError> {
    if nper == 0.0 {
        return finite(-pv);
    }
    let one_plus = 1.0 + rate;
    let type_scale = 1.0 + rate * typ;
    let term = one_plus.powf(nper);
    if !term.is_finite() {
        return Err(ExcelError::Num);
    }
    finite(-pv * term - pmt * type_scale * (term - 1.0) / rate)
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

    fn cum_close(actual: f64, expected: f64) {
        assert!(
            excel_num_eq(actual, expected),
            "cumipmt mismatch: got {actual} expected {expected}"
        );
    }

    #[test]
    fn microsoft_cumipmt_examples() {
        // support.microsoft.com CUMIPMT: second-year interest ($11,135.23),
        // first-month interest ($937.50) on 9% / 30y / $125,000.
        assert_eq!(
            cents(cumipmt(0.09 / 12.0, 30.0 * 12.0, 125_000.0, 13.0, 24.0, 0.0).unwrap()),
            -1_113_523
        );
        assert_eq!(
            cumipmt(0.09 / 12.0, 30.0 * 12.0, 125_000.0, 1.0, 1.0, 0.0).unwrap(),
            -937.5
        );
    }

    #[test]
    fn cumipmt_first_period_is_pv_times_rate() {
        cum_close(
            cumipmt(0.09 / 12.0, 360.0, 125_000.0, 1.0, 1.0, 0.0).unwrap(),
            -125_000.0 * 0.09 / 12.0,
        );
    }

    #[test]
    fn cumipmt_type_one_first_period_is_zero() {
        assert_eq!(
            cumipmt(0.09 / 12.0, 360.0, 125_000.0, 1.0, 1.0, 1.0).unwrap(),
            0.0
        );
    }

    #[test]
    fn cumipmt_single_period_matches_ipmt_identity() {
        let rate = 0.08 / 12.0;
        let nper = 10.0;
        let pv = 10_000.0;
        for per in 1..=10 {
            let per = f64::from(per);
            let closed = cumipmt(rate, nper, pv, per, per, 0.0).unwrap();
            let looped = cumipmt_naive(rate, nper, pv, per, per, 0.0).unwrap();
            let scale = closed.abs().max(looped.abs()).max(1.0);
            assert!(
                (closed - looped).abs() / scale < 1e-9,
                "period {per}: closed {closed} vs IPMT-sum {looped}"
            );
        }
    }

    #[test]
    fn cumipmt_truncates_periods_and_type() {
        let a = cumipmt(0.1, 10.9, 1_000.0, 1.9, 3.2, 0.9).unwrap();
        let b = cumipmt(0.1, 10.0, 1_000.0, 1.0, 3.0, 0.0).unwrap();
        cum_close(a, b);
        let due = cumipmt(0.1, 10.0, 1_000.0, 1.0, 3.0, 1.9).unwrap();
        cum_close(due, cumipmt(0.1, 10.0, 1_000.0, 1.0, 3.0, 1.0).unwrap());
    }

    #[test]
    fn cumipmt_domain_errors() {
        assert_eq!(
            cumipmt(0.0, 10.0, 1_000.0, 1.0, 1.0, 0.0),
            Err(ExcelError::Num)
        );
        assert_eq!(
            cumipmt(-0.05, 10.0, 1_000.0, 1.0, 1.0, 0.0),
            Err(ExcelError::Num)
        );
        assert_eq!(
            cumipmt(0.1, 0.0, 1_000.0, 1.0, 1.0, 0.0),
            Err(ExcelError::Num)
        );
        assert_eq!(cumipmt(0.1, 10.0, 0.0, 1.0, 1.0, 0.0), Err(ExcelError::Num));
        assert_eq!(
            cumipmt(0.1, 10.0, -1_000.0, 1.0, 1.0, 0.0),
            Err(ExcelError::Num)
        );
        assert_eq!(
            cumipmt(0.1, 10.0, 1_000.0, 0.0, 1.0, 0.0),
            Err(ExcelError::Num)
        );
        assert_eq!(
            cumipmt(0.1, 10.0, 1_000.0, 2.0, 1.0, 0.0),
            Err(ExcelError::Num)
        );
        assert_eq!(
            cumipmt(0.1, 10.0, 1_000.0, 1.0, 11.0, 0.0),
            Err(ExcelError::Num)
        );
        assert_eq!(
            cumipmt(0.1, 10.0, 1_000.0, 1.0, 1.0, 2.0),
            Err(ExcelError::Num)
        );
        assert_eq!(
            cumipmt(0.1, 10.0, 1_000.0, 1.0, 1.0, -1.0),
            Err(ExcelError::Num)
        );
    }

    #[test]
    fn cumipmt_overflow_is_num() {
        // Period 1 is −pv·rate (no pow). Overflow is the PMT / (1+r)^n step
        // needed once the span leaves that identity.
        assert_eq!(
            cumipmt(0.5, 2000.0, 1_000.0, 1.0, 2.0, 0.0),
            Err(ExcelError::Num)
        );
    }

    #[test]
    fn cumipmt_naive_matches_hot_path() {
        let cases = [
            (0.09 / 12.0, 360.0, 125_000.0, 13.0, 24.0, 0.0),
            (0.09 / 12.0, 360.0, 125_000.0, 1.0, 1.0, 0.0),
            (0.09 / 12.0, 360.0, 125_000.0, 1.0, 1.0, 1.0),
            (0.05 / 12.0, 360.0, 200_000.0, 1.0, 360.0, 0.0),
            (0.05 / 12.0, 360.0, 200_000.0, 2.0, 12.0, 1.0),
            (0.08 / 12.0, 10.0, 10_000.0, 1.0, 10.0, 0.0),
            (0.1, 10.0, 1_000.0, 1.0, 3.0, 1.0),
        ];
        for (rate, nper, pv, start, end, typ) in cases {
            let a = cumipmt(rate, nper, pv, start, end, typ).unwrap();
            let b = cumipmt_naive(rate, nper, pv, start, end, typ).unwrap();
            let scale = a.abs().max(b.abs()).max(1.0);
            assert!(
                (a - b).abs() / scale < 1e-9,
                "cumipmt vs naive: {a} vs {b} (start={start} end={end} type={typ})"
            );
        }
    }

    #[test]
    fn cumipmt_hot_path_many_calls() {
        let start = std::time::Instant::now();
        let mut acc = 0.0f64;
        let rate = 0.05 / 12.0;
        for i in 0..80_000u32 {
            acc += cumipmt(rate, 360.0, 200_000.0 + f64::from(i), 13.0, 24.0, 0.0).unwrap();
        }
        let elapsed = start.elapsed();
        assert!(acc.is_finite());
        assert!(
            elapsed.as_millis() < 400,
            "80k CUMIPMT calls took {elapsed:?} (expected a cheap closed form)"
        );
    }
}
