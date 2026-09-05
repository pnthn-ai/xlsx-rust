//! Excel `IRR(values, [guess])` Newton / secant kernel.
//!
//! Desktop Excel (Microsoft's IRR help):
//! - Starts at `guess` (default `0.1`) and iterates until the rate is
//!   accurate within **0.00001 percent** (`1e-7` as a decimal rate).
//! - After **20** tries without a result, the worksheet function returns
//!   `#NUM!`.
//! - A series with no sign change (no `> 0` and no `< 0`) is also `#NUM!`.
//! - Rates at or below `-100%` (`r <= -1`) make `(1+r)^k` undefined for
//!   the NPV polynomial; Excel does not return them. A Newton step that
//!   lands there is a failed iteration (the two-year Microsoft example
//!   needs an explicit guess for this reason).
//!
//! Production evaluation uses Horner's method for NPV and NPV' in one
//! pass. [`irr_naive`] is the same iteration with per-term `pow` so the
//! bench can report a before/after.

/// Excel iteration cap (`#NUM!` if the rate has not settled).
pub const MAX_ITERS: u32 = 20;

/// Absolute rate tolerance: 0.00001 percent = `1e-7`.
pub const RATE_TOL: f64 = 1e-7;

const DERIV_MIN: f64 = 1e-14;
const MIN_ONE_PLUS: f64 = 1e-12;

/// Production `IRR` kernel (Horner NPV / NPV').
///
/// `None` means the worksheet function must return `#NUM!`.
pub fn irr(values: &[f64], guess: f64) -> Option<f64> {
    irr_loop(values, guess, npv_deriv_horner)
}

/// Quadratic-cost baseline: `pow(1+r, k)` per cash-flow per iteration.
///
/// Same Excel decision rules as [`irr`]; slower on long series.
pub fn irr_naive(values: &[f64], guess: f64) -> Option<f64> {
    irr_loop(values, guess, npv_deriv_pow)
}

fn irr_loop(
    values: &[f64],
    guess: f64,
    npv_deriv: fn(&[f64], f64) -> Option<(f64, f64)>,
) -> Option<f64> {
    if !guess.is_finite() {
        return None;
    }
    if !has_sign_change(values) {
        return None;
    }
    if !rate_ok(guess) {
        return None;
    }

    let mut r0 = guess;
    let mut prev: Option<(f64, f64)> = None;

    for _ in 0..MAX_ITERS {
        let (npv, deriv) = npv_deriv(values, r0)?;
        if !npv.is_finite() {
            return None;
        }
        if npv.abs() == 0.0 {
            return Some(r0);
        }

        let newton = if deriv.is_finite() && deriv.abs() > DERIV_MIN {
            Some(r0 - npv / deriv)
        } else {
            None
        };
        let r1 = match newton.filter(|r| rate_ok(*r) && r.is_finite()) {
            Some(r) => r,
            None => {
                let (pr, pn) = prev?;
                let den = npv - pn;
                if !den.is_finite() || den.abs() < 1e-18 {
                    return None;
                }
                let secant = r0 - npv * (r0 - pr) / den;
                if !secant.is_finite() || !rate_ok(secant) {
                    return None;
                }
                secant
            }
        };

        if (r1 - r0).abs() <= RATE_TOL {
            // Tiny residuals around a true 0% root compare as 0 under Excel 15-digit.
            return Some(if r1.abs() < 1e-14 { 0.0 } else { r1 });
        }
        prev = Some((r0, npv));
        r0 = r1;
    }
    None
}

fn has_sign_change(values: &[f64]) -> bool {
    let mut pos = false;
    let mut neg = false;
    for &v in values {
        if !v.is_finite() {
            return false;
        }
        if v > 0.0 {
            pos = true;
        } else if v < 0.0 {
            neg = true;
        }
        if pos && neg {
            return true;
        }
    }
    false
}

#[inline]
fn rate_ok(r: f64) -> bool {
    r.is_finite() && r > -1.0
}

/// Horner evaluation of `Σ v[k] x^k` and `d(NPV)/dr` with `x = 1/(1+r)`.
fn npv_deriv_horner(values: &[f64], rate: f64) -> Option<(f64, f64)> {
    let one = 1.0 + rate;
    if one.abs() < MIN_ONE_PLUS {
        return None;
    }
    let x = 1.0 / one;
    if !x.is_finite() {
        return None;
    }
    let mut p = 0.0;
    let mut dp = 0.0;
    for &v in values.iter().rev() {
        dp = dp * x + p;
        p = p * x + v;
    }
    let deriv = dp * (-x / one);
    if !p.is_finite() || !deriv.is_finite() {
        return None;
    }
    Some((p, deriv))
}

/// Per-term `pow` NPV / NPV' (bench baseline).
fn npv_deriv_pow(values: &[f64], rate: f64) -> Option<(f64, f64)> {
    let one = 1.0 + rate;
    if one.abs() < MIN_ONE_PLUS {
        return None;
    }
    let mut npv = 0.0;
    let mut deriv = 0.0;
    for (k, &v) in values.iter().enumerate() {
        let pk = one.powi(k as i32);
        let pk1 = pk * one;
        if !pk.is_finite() || !pk1.is_finite() || pk == 0.0 {
            return None;
        }
        npv += v / pk;
        deriv += -(k as f64) * v / pk1;
    }
    if !npv.is_finite() || !deriv.is_finite() {
        return None;
    }
    Some((npv, deriv))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn both(values: &[f64], guess: f64) -> Option<f64> {
        let fast = irr(values, guess);
        let slow = irr_naive(values, guess);
        match (fast, slow) {
            (Some(a), Some(b)) => {
                assert!(
                    (a - b).abs() <= 1e-12,
                    "naive/fast mismatch: {a} vs {b} for {values:?} guess={guess}"
                );
                Some(a)
            }
            (None, None) => None,
            other => panic!("naive/fast Option mismatch: {other:?}"),
        }
    }

    #[test]
    fn simple_ten_percent() {
        let r = both(&[-100.0, 110.0], 0.1).unwrap();
        assert!((r - 0.1).abs() < 1e-12);
    }

    #[test]
    fn microsoft_five_year() {
        let v = [-70000.0, 12000.0, 15000.0, 18000.0, 21000.0, 26000.0];
        let r = both(&v, 0.1).unwrap();
        assert!((r - 0.0866309480365316).abs() < 1e-12);
    }

    #[test]
    fn microsoft_four_year() {
        let v = [-70000.0, 12000.0, 15000.0, 18000.0, 21000.0];
        let r = both(&v, 0.1).unwrap();
        assert!((r - -0.021244848273411).abs() < 1e-12);
    }

    #[test]
    fn microsoft_two_year_needs_guess() {
        let v = [-70000.0, 12000.0, 15000.0];
        assert_eq!(both(&v, 0.1), None, "default guess must #NUM!");
        let r = both(&v, -0.1).unwrap();
        assert!((r - -0.443506941334741).abs() < 1e-12);
    }

    #[test]
    fn two_roots_follow_guess() {
        let v = [-100.0, 230.0, -132.0];
        let lo = both(&v, 0.05).unwrap();
        let hi = both(&v, 0.25).unwrap();
        assert!((lo - 0.1).abs() < 1e-9);
        assert!((hi - 0.2).abs() < 1e-9);
    }

    #[test]
    fn no_sign_change_is_num() {
        assert_eq!(both(&[10.0, 20.0], 0.1), None);
        assert_eq!(both(&[-10.0, -20.0], 0.1), None);
        assert_eq!(both(&[0.0, 0.0], 0.1), None);
        assert_eq!(both(&[-100.0], 0.1), None);
    }

    #[test]
    fn guess_at_minus_one_is_num() {
        assert_eq!(both(&[-100.0, 110.0], -1.0), None);
    }

    #[test]
    fn zero_cashflow_occupies_a_period() {
        let r = both(&[-100.0, 0.0, 121.0], 0.1).unwrap();
        assert!((r - 0.1).abs() < 1e-12);
    }

    #[test]
    fn rate_of_zero() {
        let r = both(&[-100.0, 100.0], 0.1).unwrap();
        assert!(r.abs() < 1e-12);
    }
}
