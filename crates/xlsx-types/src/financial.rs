//! Time-value-of-money helpers used by worksheet financial functions.
//!
//! Pure math: no workbook, no fixture goldens. `PMT` is the closed-form
//! starter; [`rate`] inverts the same identity by Excel's Newton loop.
//! `PV` / `FV` / `NPER` are expected to reuse [`pow_term`] later.

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

/// Excel iteration cap for [`rate`] (`#NUM!` if the guess has not settled).
pub const RATE_MAX_ITERS: u32 = 20;

/// Absolute rate tolerance: successive results within `0.0000001` (Excel RATE).
pub const RATE_TOL: f64 = 1e-7;

const DERIV_MIN: f64 = 1e-14;
const ZERO_RATE: f64 = 1e-14;

/// Excel / OpenFormula `RATE(nper, pmt, pv, [fv], [type], [guess])`.
///
/// OpenFormula 6.12.42 — solve the same TVM identity as [`pmt`] for `rate`:
///
/// ```text
/// r = 0  →  pv + pmt·nper + fv = 0
/// else   →  pv·(1+r)^n + pmt·(1 + r·type)·((1+r)^n − 1)/r + fv = 0
/// ```
///
/// Desktop Excel (Microsoft RATE help):
/// - Starts at `guess` (default `0.1`) and iterates until successive rates
///   agree within **0.0000001**.
/// - After **20** tries without a result → `#NUM!`.
/// - A Newton (or secant) step to `r <= -1` is a failed iteration: `(1+r)^n`
///   is undefined on that side, and Excel does not return those roots.
/// - `guess <= -1` cannot start the loop → `#NUM!`.
///
/// `type` is the OpenFormula PayType multiplier (same real as [`pmt`], not
/// a boolean). Closed forms (`nper = 1`, `pmt = 0`, exact 0% residual) skip
/// the loop. Production evaluation uses [`pow_term`]; [`rate_naive`] is the
/// `powf` baseline so the bench can report a before/after.
pub fn rate(
    nper: f64,
    pmt: f64,
    pv: f64,
    fv: f64,
    typ: f64,
    guess: f64,
) -> Result<f64, ExcelError> {
    rate_inner(nper, pmt, pv, fv, typ, guess, false)
}

/// Baseline `RATE`: same Excel decision rules, but Newton uses `powf`
/// so `(1+r)^n − 1` cancels on tiny rates. Used only for the hill-climb bench.
pub fn rate_naive(
    nper: f64,
    pmt: f64,
    pv: f64,
    fv: f64,
    typ: f64,
    guess: f64,
) -> Result<f64, ExcelError> {
    rate_inner(nper, pmt, pv, fv, typ, guess, true)
}

fn rate_inner(
    nper: f64,
    pmt: f64,
    pv: f64,
    fv: f64,
    typ: f64,
    guess: f64,
    naive: bool,
) -> Result<f64, ExcelError> {
    if !nper.is_finite()
        || !pmt.is_finite()
        || !pv.is_finite()
        || !fv.is_finite()
        || !typ.is_finite()
        || !guess.is_finite()
    {
        return Err(ExcelError::Num);
    }
    if guess <= -1.0 {
        return Err(ExcelError::Num);
    }

    // Exact 0% root of the TVM identity (no iteration).
    let zero_residual = pv + pmt * nper + fv;
    if zero_residual == 0.0 {
        return Ok(0.0);
    }

    if nper == 0.0 {
        return Err(ExcelError::Num);
    }

    if nper == 1.0 {
        return rate_one_period(pmt, pv, fv, typ);
    }

    if pmt == 0.0 {
        return rate_no_pmt(nper, pv, fv);
    }

    rate_newton(nper, pmt, pv, fv, typ, guess, naive)
}

/// `nper = 1` → `r = −(pv + pmt + fv) / (pv + pmt·type)`.
fn rate_one_period(pmt: f64, pv: f64, fv: f64, typ: f64) -> Result<f64, ExcelError> {
    let den = pv + pmt * typ;
    if den == 0.0 {
        return Err(ExcelError::Num);
    }
    let r = -(pv + pmt + fv) / den;
    if !r.is_finite() || r <= -1.0 {
        return Err(ExcelError::Num);
    }
    Ok(if r.abs() < ZERO_RATE { 0.0 } else { r })
}

/// `pmt = 0` → `r = (−fv/pv)^(1/nper) − 1`. Needs `−fv/pv > 0` so `r > −1`.
fn rate_no_pmt(nper: f64, pv: f64, fv: f64) -> Result<f64, ExcelError> {
    if pv == 0.0 {
        return Err(ExcelError::Num);
    }
    let ratio = -fv / pv;
    if !(ratio > 0.0) {
        return Err(ExcelError::Num);
    }
    let log_ratio = ratio.ln();
    if !log_ratio.is_finite() {
        return Err(ExcelError::Num);
    }
    let r = (log_ratio / nper).exp_m1();
    if !r.is_finite() || r <= -1.0 {
        return Err(ExcelError::Num);
    }
    Ok(if r.abs() < ZERO_RATE { 0.0 } else { r })
}

fn rate_newton(
    nper: f64,
    pmt: f64,
    pv: f64,
    fv: f64,
    typ: f64,
    guess: f64,
    naive: bool,
) -> Result<f64, ExcelError> {
    let mut r0 = guess;
    let mut prev: Option<(f64, f64)> = None;

    for _ in 0..RATE_MAX_ITERS {
        let (y, dy) = tvm_residual(r0, nper, pmt, pv, fv, typ, naive)?;
        if !y.is_finite() {
            return Err(ExcelError::Num);
        }
        if y.abs() == 0.0 {
            return Ok(polish_rate(r0, nper, pmt, pv, fv, typ, naive));
        }

        let newton = if dy.is_finite() && dy.abs() > DERIV_MIN {
            Some(r0 - y / dy)
        } else {
            None
        };
        let r1 = match newton.filter(|r| r.is_finite() && *r > -1.0) {
            Some(r) => r,
            None => match prev {
                Some((pr, py)) => {
                    let den = y - py;
                    if !den.is_finite() || den.abs() < 1e-18 {
                        return Err(ExcelError::Num);
                    }
                    let secant = r0 - y * (r0 - pr) / den;
                    if !secant.is_finite() || secant <= -1.0 {
                        return Err(ExcelError::Num);
                    }
                    secant
                }
                None => {
                    // Dead derivative on the first try: nudge off the flat
                    // (LibreOffice / Excel Newton quirk) instead of #NUM!.
                    let nudged = r0 + 1.1 * RATE_TOL;
                    if nudged <= -1.0 {
                        return Err(ExcelError::Num);
                    }
                    nudged
                }
            },
        };

        if (r1 - r0).abs() <= RATE_TOL {
            return Ok(polish_rate(r1, nper, pmt, pv, fv, typ, naive));
        }
        prev = Some((r0, y));
        r0 = r1;
    }
    Err(ExcelError::Num)
}

/// Extra Newton steps after Excel's 20-iter settle, so a `PMT` inverse
/// compares equal under 15-digit Excel rounding. Does not change the
/// `#NUM!` decision (that already happened).
fn polish_rate(mut r: f64, nper: f64, pmt: f64, pv: f64, fv: f64, typ: f64, naive: bool) -> f64 {
    if r.abs() < ZERO_RATE {
        return 0.0;
    }
    for _ in 0..4 {
        let Ok((y, dy)) = tvm_residual(r, nper, pmt, pv, fv, typ, naive) else {
            break;
        };
        if y.abs() == 0.0 || !dy.is_finite() || dy.abs() <= DERIV_MIN {
            break;
        }
        let nxt = r - y / dy;
        if !nxt.is_finite() || nxt <= -1.0 {
            break;
        }
        r = nxt;
    }
    if r.abs() < ZERO_RATE {
        0.0
    } else {
        r
    }
}

/// TVM residual `y(r)` and `y'(r)` for Newton.
///
/// ```text
/// y  = pv·g + pmt·(1/r + type)·(g − 1) + fv
/// y' = pv·g' + pmt·(−1/r²)·(g − 1) + pmt·(1/r + type)·g'
/// ```
///
/// `r = 0` is a removable singularity: the annuity factor limit is `nper`
/// and `y' = n·(pv + pmt·((n−1)/2 + type))`.
fn tvm_residual(
    r: f64,
    nper: f64,
    pmt: f64,
    pv: f64,
    fv: f64,
    typ: f64,
    naive: bool,
) -> Result<(f64, f64), ExcelError> {
    if r == 0.0 {
        let y = pv + pmt * nper + fv;
        let dy = nper * (pv + pmt * ((nper - 1.0) * 0.5 + typ));
        if !y.is_finite() || !dy.is_finite() {
            return Err(ExcelError::Num);
        }
        return Ok((y, dy));
    }
    if r <= -1.0 {
        return Err(ExcelError::Num);
    }

    let one_plus = 1.0 + r;
    if one_plus < 0.0 && nper.fract() != 0.0 {
        return Err(ExcelError::Num);
    }

    let (g, g_m1) = if naive {
        let g = one_plus.powf(nper);
        (g, g - 1.0)
    } else {
        pow_term(one_plus, r, nper)?
    };
    if !g.is_finite() || !g_m1.is_finite() {
        return Err(ExcelError::Num);
    }

    let g_prime = nper * g / one_plus;
    let inv_r = 1.0 / r;
    let y = pv * g + pmt * (inv_r + typ) * g_m1 + fv;
    let dy = pv * g_prime + pmt * (-inv_r * inv_r) * g_m1 + pmt * (inv_r + typ) * g_prime;
    if !y.is_finite() || !dy.is_finite() {
        return Err(ExcelError::Num);
    }
    Ok((y, dy))
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

    fn both_rate(
        nper: f64,
        pmt_v: f64,
        pv: f64,
        fv: f64,
        typ: f64,
        guess: f64,
    ) -> Result<f64, ExcelError> {
        let fast = rate(nper, pmt_v, pv, fv, typ, guess);
        let slow = rate_naive(nper, pmt_v, pv, fv, typ, guess);
        match (fast, slow) {
            (Ok(a), Ok(b)) => {
                assert!(
                    excel_num_eq(a, b) || (a - b).abs() <= 1e-12,
                    "naive/fast RATE mismatch: {a} vs {b} nper={nper} pmt={pmt_v} pv={pv} fv={fv} type={typ} guess={guess}"
                );
                Ok(a)
            }
            (Err(a), Err(b)) => {
                assert_eq!(a, b, "naive/fast RATE error mismatch");
                Err(a)
            }
            other => panic!("naive/fast RATE kind mismatch: {other:?}"),
        }
    }

    #[test]
    fn microsoft_rate_loan() {
        // support.microsoft.com RATE: 4 years, −$200/mo, $8,000 loan.
        // Published as 1% (percent, 0 decimals) / 9.24% annualized.
        let r = both_rate(4.0 * 12.0, -200.0, 8_000.0, 0.0, 0.0, 0.1).unwrap();
        assert!((r - 0.007701472).abs() < 1e-9, "monthly RATE got {r}");
        assert_eq!(((r * 12.0) * 10_000.0).round() as i64, 924);
        let pay = pmt(r, 48.0, 8_000.0, 0.0, 0.0).unwrap();
        assert!(
            (pay + 200.0).abs() < 1e-9,
            "RATE→PMT should recover −200, got {pay}"
        );
    }

    #[test]
    fn rate_inverts_pmt() {
        let cases = [
            (0.1, 10.0, 1_000.0, 0.0, 0.0),
            (0.08 / 12.0, 10.0, 10_000.0, 0.0, 0.0),
            (0.08 / 12.0, 10.0, 10_000.0, 0.0, 1.0),
            (0.05 / 12.0, 360.0, 200_000.0, 0.0, 0.0),
            (0.05, 5.0, 10_000.0, 1_000.0, 0.0),
            (-0.05, 10.0, 1_000.0, 0.0, 0.0),
            (0.1, 10.5, 1_000.0, 0.0, 0.0),
            (0.1, -10.0, 1_000.0, 0.0, 0.0),
        ];
        for (r, n, pv, fv, typ) in cases {
            let pay = pmt(r, n, pv, fv, typ).unwrap();
            // Long horizons need a guess near the monthly root; Excel's
            // default 0.1 does not settle in 20 Newton steps (see
            // `rate_long_horizon_needs_guess`).
            let guess = if n.abs() > 60.0 { r } else { 0.1 };
            let got = both_rate(n, pay, pv, fv, typ, guess).unwrap_or_else(|e| {
                panic!("RATE inverse #NUM!/{e:?} for r={r} nper={n} pmt={pay} pv={pv} fv={fv} type={typ}")
            });
            let scale = r.abs().max(1e-6);
            assert!(
                (got - r).abs() / scale <= 1e-8,
                "RATE inverse missed: got {got} expected {r} nper={n} pmt={pay} pv={pv} fv={fv} type={typ}"
            );
        }
    }

    #[test]
    fn rate_zero_is_straight_line() {
        assert_eq!(
            both_rate(10.0, -100.0, 1_000.0, 0.0, 0.0, 0.1).unwrap(),
            0.0
        );
        assert_eq!(
            both_rate(10.0, -150.0, 1_000.0, 500.0, 0.0, 0.1).unwrap(),
            0.0
        );
    }

    #[test]
    fn rate_one_period_closed_form() {
        let r = both_rate(1.0, -110.0, 100.0, 0.0, 0.0, 0.1).unwrap();
        assert!((r - 0.1).abs() < 1e-12, "got {r}");
        // type=1 / nper=1 / fv=0 is rate-independent (PMT = −pv); skip that.
        let pay = pmt(0.1, 1.0, 100.0, 50.0, 1.0).unwrap();
        let got = both_rate(1.0, pay, 100.0, 50.0, 1.0, 0.1).unwrap();
        assert!((got - 0.1).abs() <= 1e-12, "one-period type=1 got {got}");
    }

    #[test]
    fn rate_pmt_zero_is_compound() {
        let r = both_rate(10.0, 0.0, -1_000.0, 2_000.0, 0.0, 0.1).unwrap();
        let expect = (2f64.ln() / 10.0).exp_m1();
        assert!((r - expect).abs() <= 1e-14, "compound RATE {r} vs {expect}");
        assert_eq!(
            both_rate(10.0, 0.0, 1_000.0, 0.0, 0.0, 0.1),
            Err(ExcelError::Num)
        );
    }

    #[test]
    fn rate_guess_minus_one_is_num() {
        assert_eq!(
            both_rate(10.0, -100.0, 1_000.0, 0.0, 0.0, -1.0),
            Err(ExcelError::Num)
        );
        assert_eq!(
            both_rate(10.0, -100.0, 1_000.0, 0.0, 0.0, -1.5),
            Err(ExcelError::Num)
        );
    }

    #[test]
    fn rate_nper_zero_is_num() {
        assert_eq!(
            both_rate(0.0, -100.0, 1_000.0, 0.0, 0.0, 0.1),
            Err(ExcelError::Num)
        );
    }

    #[test]
    fn rate_newton_fails_same_sign_cashflows() {
        // All cash out, no inflow — no root in (−1, ∞) from the default guess.
        assert_eq!(
            both_rate(10.0, 100.0, 1_000.0, 500.0, 0.0, 0.1),
            Err(ExcelError::Num)
        );
    }

    #[test]
    fn rate_type_one_excel_quirk_cases() {
        // HyperFormula / Excel: type=1 with a small fv can #NUM! from guess 0.1.
        assert_eq!(
            both_rate(12.0, -100.0, 400.0, 0.0, 1.0, 0.1),
            Err(ExcelError::Num)
        );
        assert_eq!(
            both_rate(12.0, -100.0, 400.0, -100.0, 1.0, 0.1),
            Err(ExcelError::Num)
        );
        let r = both_rate(12.0, -100.0, 400.0, 100.0, 1.0, 0.1).unwrap();
        assert!((r + 0.4997).abs() < 5e-4, "Excel type=1 root, got {r}");
    }

    #[test]
    fn rate_frac_nper() {
        let r = both_rate(0.9, -100.0, 400.0, 0.0, 0.0, 0.1).unwrap();
        assert!((r + 0.7962).abs() < 5e-4, "frac nper root, got {r}");
    }

    #[test]
    fn rate_long_horizon_needs_guess() {
        // Excel RATE: 20 Newton steps from the default 10% guess. A 30-year
        // monthly mortgage root (~0.42%) is too far; successive rates have
        // not settled → #NUM!. A 1% guess recovers the PMT inverse.
        let monthly = 0.05 / 12.0;
        let pay = pmt(monthly, 360.0, 200_000.0, 0.0, 0.0).unwrap();
        assert_eq!(
            both_rate(360.0, pay, 200_000.0, 0.0, 0.0, 0.1),
            Err(ExcelError::Num)
        );
        let got = both_rate(360.0, pay, 200_000.0, 0.0, 0.0, 0.01).unwrap();
        assert!(
            (got - monthly).abs() <= 1e-12,
            "guess 1% should invert the mortgage, got {got}"
        );
    }

    #[test]
    fn rate_explicit_guess() {
        let pay = pmt(0.25, 8.0, 1_000.0, 0.0, 0.0).unwrap();
        close(both_rate(8.0, pay, 1_000.0, 0.0, 0.0, 0.25).unwrap(), 0.25);
        close(both_rate(8.0, pay, 1_000.0, 0.0, 0.0, 0.1).unwrap(), 0.25);
    }

    #[test]
    fn rate_hot_path_many_calls() {
        let start = std::time::Instant::now();
        let mut acc = 0.0f64;
        let monthly = 0.05 / 12.0;
        for i in 0..20_000u32 {
            let principal = 200_000.0 + f64::from(i);
            let pay = pmt(monthly, 360.0, principal, 0.0, 0.0).unwrap();
            acc += rate(360.0, pay, principal, 0.0, 0.0, monthly).unwrap();
        }
        let elapsed = start.elapsed();
        assert!(acc.is_finite());
        assert!(
            elapsed.as_millis() < 2_000,
            "20k RATE Newton calls took {elapsed:?}"
        );
    }
}
