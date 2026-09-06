//! Time-value-of-money helpers used by worksheet financial functions.
//!
//! Pure math: no workbook, no fixture goldens. Shared [`pow_term`] backs the
//! closed-form TVM family (`PMT` / `FV` / `PV` / `NPER` / `RATE` / `IPMT` /
//! `PPMT` / `CUMPRINC` / `CUMIPMT`) plus `EFFECT` / `NOMINAL` / `PDURATION` /
//! `RRI`.

use crate::error::ExcelError;

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

#[inline]
fn finite(n: f64) -> Result<f64, ExcelError> {
    if n.is_finite() {
        Ok(n)
    } else {
        Err(ExcelError::Num)
    }
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
            let balance = fv_at(rate, per - 1.0, pmt_val, pv, typ, false)?;
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

/// Excel / OpenFormula `EFFECT(nominal_rate, npery)`.
///
/// OpenFormula 6.12.19 (Excel `TRUNC` on `npery` before the power):
///
/// ```text
/// EFFECT = (1 + nominal / npery)^npery − 1
/// ```
///
/// Domain (support.microsoft.com EFFECT):
/// - non-finite inputs → `#NUM!`
/// - `nominal_rate ≤ 0` or truncated `npery < 1` → `#NUM!`
/// - overflow / non-finite result → `#NUM!` (same as `POWER`)
///
/// Production path:
/// - truncated `npery == 1` is the identity `nominal`
/// - `npery == 2` is `nominal + (nominal/2)^2`
/// - otherwise [`pow_term`]: integer `powi` when `|r/n| ≥ 1e-5`, else
///   `expm1(n · ln1p(r/n))` so `(1+ε)^n − 1` does not cancel
#[inline]
pub fn effect(nominal: f64, npery: f64) -> Result<f64, ExcelError> {
    let n = trunc_npery(nominal, npery)?;
    if n == 1.0 {
        return finite(nominal);
    }
    if n == 2.0 {
        let half = nominal * 0.5;
        return finite(nominal + half * half);
    }
    let period = nominal / n;
    if !period.is_finite() {
        return Err(ExcelError::Num);
    }
    let (_, term_m1) = pow_term(1.0 + period, period, n)?;
    finite(term_m1)
}

/// Textbook `(1 + nominal/npery).powf(npery) - 1` baseline (same domain as
/// [`effect`]). Used as the microbench naive path.
#[inline]
pub fn effect_naive(nominal: f64, npery: f64) -> Result<f64, ExcelError> {
    let n = trunc_npery(nominal, npery)?;
    let one_plus = 1.0 + nominal / n;
    if !one_plus.is_finite() {
        return Err(ExcelError::Num);
    }
    finite(one_plus.powf(n) - 1.0)
}

#[inline]
fn trunc_npery(nominal: f64, npery: f64) -> Result<f64, ExcelError> {
    if !nominal.is_finite() || !npery.is_finite() {
        return Err(ExcelError::Num);
    }
    if nominal <= 0.0 {
        return Err(ExcelError::Num);
    }
    let n = npery.trunc();
    if n < 1.0 {
        return Err(ExcelError::Num);
    }
    Ok(n)
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

/// Excel / OpenFormula `RRI(nper, pv, fv)`.
///
/// OpenFormula 6.12.45 / Microsoft RRI:
///
/// ```text
/// (fv / pv) ^ (1 / nper) − 1
/// ```
///
/// Production path uses `expm1(ln1p((fv−pv)/pv) / nper)` so a tiny growth
/// (`fv ≈ pv`) does not cancel. Integer `nper == 1` is the simple return
/// `(fv − pv) / pv`.
///
/// Domain errors (`#NUM!`):
/// - non-finite arguments
/// - `nper ≤ 0` (Microsoft / LibreOffice: periods must be positive)
/// - `pv = 0` (division by present value)
/// - `pv` and `fv` have opposite signs and `nper ≠ 1` (LibreOffice Excel
///   compat; `nper = 1` is just `fv/pv − 1`)
/// - overflow of the power (`#NUM!`, same as `POWER`)
#[inline]
pub fn rri(nper: f64, pv: f64, fv: f64) -> Result<f64, ExcelError> {
    rri_checked(nper, pv, fv, rri_expm1)
}

/// Quadratic-ish baseline: `(fv/pv).powf(1/nper) − 1`.
///
/// Same Excel domain as [`rri`]; slower / less accurate when `fv ≈ pv`.
#[inline]
pub fn rri_naive(nper: f64, pv: f64, fv: f64) -> Result<f64, ExcelError> {
    rri_checked(nper, pv, fv, rri_powf)
}

#[inline]
fn rri_checked(
    nper: f64,
    pv: f64,
    fv: f64,
    eval: fn(f64, f64, f64) -> Result<f64, ExcelError>,
) -> Result<f64, ExcelError> {
    if !nper.is_finite() || !pv.is_finite() || !fv.is_finite() {
        return Err(ExcelError::Num);
    }
    if nper <= 0.0 || pv == 0.0 {
        return Err(ExcelError::Num);
    }
    let opposite = (pv > 0.0 && fv < 0.0) || (pv < 0.0 && fv > 0.0);
    if opposite && nper != 1.0 {
        return Err(ExcelError::Num);
    }
    eval(nper, pv, fv)
}

#[inline]
fn rri_expm1(nper: f64, pv: f64, fv: f64) -> Result<f64, ExcelError> {
    if nper == 1.0 {
        return finite((fv - pv) / pv);
    }
    if fv == pv {
        return Ok(0.0);
    }
    if fv == 0.0 {
        return Ok(-1.0);
    }
    // (1 + rel)^(1/nper) − 1 with rel = fv/pv − 1.
    let rel = (fv - pv) / pv;
    let log_term = rel.ln_1p() / nper;
    if !log_term.is_finite() || log_term.abs() > 700.0 {
        return Err(ExcelError::Num);
    }
    finite(log_term.exp_m1())
}

#[inline]
fn rri_powf(nper: f64, pv: f64, fv: f64) -> Result<f64, ExcelError> {
    if nper == 1.0 {
        return finite((fv - pv) / pv);
    }
    let ratio = fv / pv;
    if ratio < 0.0 {
        return Err(ExcelError::Num);
    }
    finite(ratio.powf(1.0 / nper) - 1.0)
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
            (tiny - limit).abs() < 1e-5,
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
            // Closed-form invert is algebraically exact; f64 leaves ~1e-12
            // residual at nper=360, which is outside Excel's 15-digit
            // crossover. Relative slack is the honest check.
            let slop = 1e-9 * periods.abs().max(1.0);
            assert!(
                (back - periods).abs() <= slop,
                "invert {rate},{periods},{pv},{fv},{typ}: got {back}"
            );
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
        // Payment smaller than interest: log argument is negative.
        assert_eq!(nper(0.1, -50.0, 10_000.0, 0.0, 0.0), Err(ExcelError::Num));
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
            let slop = 1e-9 * a.abs().max(b.abs()).max(1.0);
            assert!(
                (a - b).abs() <= slop,
                "naive vs kernel: {a} vs {b} (rate={rate})"
            );
        }
    }

    #[test]
    fn nper_hot_path_many_calls() {
        let start = std::time::Instant::now();
        let mut acc = 0.0f64;
        let rate = 0.05 / 12.0;
        for i in 0..80_000u32 {
            acc += nper(rate, -1_200.0, 100_000.0 + f64::from(i), 0.0, 0.0).unwrap();
        }
        let elapsed = start.elapsed();
        assert!(acc.is_finite());
        assert!(
            elapsed.as_millis() < 400,
            "80k NPER calls took {elapsed:?} (expected a cheap closed form)"
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

    fn close_rel(actual: f64, expected: f64) {
        let scale = actual.abs().max(expected.abs()).max(1.0);
        assert!(
            (actual - expected).abs() / scale < 1e-12,
            "financial rel mismatch: got {actual} expected {expected}"
        );
    }

    #[test]
    fn effect_microsoft_quarterly() {
        // support.microsoft.com EFFECT(0.0525, 4) = 0.0535426673707582
        // IEEE powi of the OpenFormula identity is ~1 ULP from that print.
        close_rel(effect(0.0525, 4.0).unwrap(), 0.0535426673707582);
        close_rel(effect_naive(0.0525, 4.0).unwrap(), 0.0535426673707582);
    }

    #[test]
    fn effect_npery_one_is_identity() {
        assert_eq!(effect(0.1, 1.0).unwrap(), 0.1);
        assert_eq!(effect(2.5, 1.0).unwrap(), 2.5);
    }

    #[test]
    fn effect_npery_two_closed_form() {
        close(effect(0.1, 2.0).unwrap(), 0.1025);
        close(effect(0.1, 2.0).unwrap(), effect_naive(0.1, 2.0).unwrap());
    }

    #[test]
    fn effect_truncates_npery_toward_zero() {
        close(effect(0.1, 12.9).unwrap(), effect(0.1, 12.0).unwrap());
        assert_eq!(effect(0.1, 1.9).unwrap(), 0.1);
    }

    #[test]
    fn effect_domain_errors() {
        assert_eq!(effect(0.0, 12.0), Err(ExcelError::Num));
        assert_eq!(effect(-0.05, 12.0), Err(ExcelError::Num));
        assert_eq!(effect(0.05, 0.0), Err(ExcelError::Num));
        assert_eq!(effect(0.05, 0.9), Err(ExcelError::Num));
        assert_eq!(effect(0.05, -1.0), Err(ExcelError::Num));
        assert_eq!(effect(f64::INFINITY, 12.0), Err(ExcelError::Num));
        assert_eq!(effect(0.05, f64::NAN), Err(ExcelError::Num));
        assert_eq!(effect(1e200, 2.0), Err(ExcelError::Num));
    }

    #[test]
    fn effect_common_frequencies_match_naive() {
        for &(r, n) in &[
            (0.05, 4.0),
            (0.08, 12.0),
            (0.12, 12.0),
            (0.05, 52.0),
            (0.06, 365.0),
            (0.01, 12.0),
            (2.0, 12.0),
        ] {
            close_rel(effect(r, n).unwrap(), effect_naive(r, n).unwrap());
        }
    }

    #[test]
    fn effect_tiny_rate_does_not_cancel() {
        let tiny = effect(1e-16, 12.0).unwrap();
        assert!(
            tiny > 0.0 && tiny < 1e-15,
            "tiny-rate EFFECT should stay near the nominal, got {tiny}"
        );
        // powf(1+ε, n) − 1 cancels to 0 in IEEE; naive is the contrast.
        assert_eq!(effect_naive(1e-16, 12.0).unwrap(), 0.0);
    }

    #[test]
    fn effect_large_npery_approaches_continuous() {
        let discrete = effect(0.05, 1_000_000.0).unwrap();
        let continuous = 0.05f64.exp_m1();
        assert!(
            (discrete - continuous).abs() < 1e-8,
            "EFFECT(0.05, 1e6)={discrete} should approach expm1(0.05)={continuous}"
        );
    }

    #[test]
    fn effect_hot_path_many_calls() {
        let start = std::time::Instant::now();
        let mut acc = 0.0f64;
        for i in 0..80_000u32 {
            let r = 0.01 + f64::from(i) * 1e-8;
            acc += effect(r, 12.0).unwrap();
        }
        let elapsed = start.elapsed();
        assert!(acc.is_finite());
        assert!(
            elapsed.as_millis() < 400,
            "80k EFFECT calls took {elapsed:?} (expected a cheap closed form)"
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

    fn both_rri(nper: f64, pv: f64, fv: f64) -> Result<f64, ExcelError> {
        let fast = rri(nper, pv, fv);
        let slow = rri_naive(nper, pv, fv);
        match (fast, slow) {
            (Ok(a), Ok(b)) => {
                let scale = a.abs().max(b.abs()).max(1.0);
                assert!(
                    (a - b).abs() / scale < 1e-12,
                    "naive/fast mismatch: {a} vs {b} for RRI({nper},{pv},{fv})"
                );
                Ok(a)
            }
            (Err(a), Err(b)) => {
                assert_eq!(a, b, "naive/fast error mismatch for RRI({nper},{pv},{fv})");
                Err(a)
            }
            other => panic!("naive/fast Result mismatch: {other:?}"),
        }
    }

    #[test]
    fn rri_microsoft_example() {
        // support.microsoft.com RRI: published 0.0009933 (7 digits).
        let n = both_rri(96.0, 10_000.0, 11_000.0).unwrap();
        assert!((n - 0.0009933).abs() < 5e-8, "got {n}");
        close(n, (0.1f64.ln_1p() / 96.0).exp_m1());
    }

    #[test]
    fn rri_simple_return_and_zeros() {
        close(both_rri(1.0, 100.0, 110.0).unwrap(), 0.1);
        close(both_rri(1.0, -100.0, 110.0).unwrap(), -2.1);
        assert_eq!(both_rri(10.0, 100.0, 100.0).unwrap(), 0.0);
        assert_eq!(both_rri(10.0, 100.0, 0.0).unwrap(), -1.0);
        close(both_rri(10.0, 1000.0, 2000.0).unwrap(), 0.07177346253629316);
    }

    #[test]
    fn rri_same_sign_negative() {
        close(
            both_rri(96.0, -10_000.0, -11_000.0).unwrap(),
            both_rri(96.0, 10_000.0, 11_000.0).unwrap(),
        );
    }

    #[test]
    fn rri_domain_errors() {
        assert_eq!(both_rri(0.0, 100.0, 110.0), Err(ExcelError::Num));
        assert_eq!(both_rri(-1.0, 100.0, 110.0), Err(ExcelError::Num));
        assert_eq!(both_rri(10.0, 0.0, 110.0), Err(ExcelError::Num));
        assert_eq!(both_rri(2.0, 100.0, -110.0), Err(ExcelError::Num));
        assert_eq!(both_rri(0.5, 100.0, -100.0), Err(ExcelError::Num));
        assert_eq!(both_rri(f64::NAN, 100.0, 110.0), Err(ExcelError::Num));
        assert_eq!(both_rri(1e-10, 1.0, 2.0), Err(ExcelError::Num));
    }

    #[test]
    fn rri_tiny_growth_stays_accurate() {
        // powf(1+ε, 1/n) − 1 cancels; ln1p/expm1 keeps the leading digits.
        let n = rri(360.0, 100_000.0, 100_001.0).unwrap();
        let identity = (1e-5f64.ln_1p() / 360.0).exp_m1();
        assert!(
            (n - identity).abs() <= 1e-18,
            "tiny-growth RRI {n} should match {identity}"
        );
    }

    #[test]
    fn rri_hot_path_many_calls() {
        let start = std::time::Instant::now();
        let mut acc = 0.0f64;
        for i in 0..80_000u32 {
            let fv = 11_000.0 + f64::from(i);
            acc += rri(96.0, 10_000.0, fv).unwrap();
        }
        let elapsed = start.elapsed();
        assert!(acc.is_finite());
        assert!(
            elapsed.as_millis() < 400,
            "80k RRI calls took {elapsed:?} (expected a cheap closed form)"
        );
    }
}
