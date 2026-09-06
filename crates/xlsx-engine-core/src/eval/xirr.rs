//! Excel `XIRR(values, dates, [guess])` Newton / bisection kernel.
//!
//! Desktop Excel / Microsoft XIRR help (no golden-reading):
//! - Finds `r` such that `XNPV(r, values, dates) = 0`:
//!   `Σ P_i / (1+r)^((d_i − d_1) / 365)` on a **365-day year** (not 365.25,
//!   not actual/actual). Day counts are **serial differences**, so the 1900
//!   leap-year bug (serial 60) is included when the span crosses it.
//! - Dates are truncated toward zero. The first date is the discount origin;
//!   every other date must be on or after it (a preceding date is `#NUM!`).
//!   Later dates may be unsorted.
//! - `values` and `dates` must contain the same number of entries (`#NUM!`
//!   otherwise). An empty series has no origin date (`#NUM!`).
//! - Invalid dates (negative / past 9999-12-31) are `#VALUE!`.
//! - At least one inflow and one outflow are required (`#NUM!` otherwise).
//!   Mixed signs are **required** (unlike `XNPV`).
//! - Range / array blanks are **zeros**, not skips (unlike `NPV` / `IRR`):
//!   a blank date becomes serial 0 and typically `#NUM!` (precedes). Text and
//!   logicals in a range or array are nonnumeric (`#VALUE!`).
//! - Scalar (non-reference) arguments coerce like `SUM`: `TRUE`→1, `"100"`→100.
//! - Default `guess` is `0.1`. Guess `-1` (or any `r <= -1`) is `#NUM!`.
//! - Excel iterates until the rate is accurate within **0.000001 percent**
//!   (`1e-8` as a decimal rate), for up to **100** tries. This kernel uses
//!   Newton-Raphson from `guess`, then a sign-bracket bisection if Newton
//!   steps off `r > -1` or fails to settle.
//!
//! Production evaluation hoists `ln1p(rate)` and uses one `exp` per packed
//! date. [`xirr_naive`] keeps per-term `powf` so the bench can report a
//! before/after. Same-day flows are collapsed once up front.

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use crate::dates::{truncate_date_serial, EXCEL_MAX_SERIAL_1900};
use xlsx_types::{DateSystem, EvalError, ExcelError, ExcelValue};

/// Excel iteration cap (`#NUM!` if the rate has not settled).
pub const MAX_ITERS: u32 = 100;

/// Absolute rate tolerance: 0.000001 percent = `1e-8`.
pub const RATE_TOL: f64 = 1e-8;

const DAYS_PER_YEAR: f64 = 365.0;
const DERIV_MIN: f64 = 1e-14;
const NEAR_ZERO_RATE: f64 = 1e-14;

/// Production `XIRR` kernel (`exp` / hoisted `ln1p` Newton + bisection).
///
/// `None` means the worksheet function must return `#NUM!`.
pub fn xirr(values: &[f64], dates: &[i32], guess: f64) -> Option<f64> {
    xirr_loop(values, dates, guess, xnpv_deriv_exp)
}

/// Per-term `powf` baseline. Same Excel rules as [`xirr`].
pub fn xirr_naive(values: &[f64], dates: &[i32], guess: f64) -> Option<f64> {
    xirr_loop(values, dates, guess, xnpv_deriv_pow)
}

fn xirr_loop(
    values: &[f64],
    dates: &[i32],
    guess: f64,
    npv_deriv: fn(&[(i32, f64)], f64) -> Option<(f64, f64)>,
) -> Option<f64> {
    if !guess.is_finite() || !rate_ok(guess) {
        return None;
    }
    let packed = pack(values, dates)?;
    if packed.is_empty() {
        // Inflows and outflows cancelled on every date: XNPV ≡ 0.
        return Some(0.0);
    }
    if packed.iter().all(|(d, _)| *d == 0) {
        // Constant nonzero NPV (all remaining flow is on the origin date).
        return None;
    }
    newton(&packed, guess, npv_deriv).or_else(|| bisect(&packed, guess, npv_deriv))
}

fn pack(values: &[f64], dates: &[i32]) -> Option<Vec<(i32, f64)>> {
    if values.len() != dates.len() || values.is_empty() {
        return None;
    }
    let d0 = dates[0];
    let mut pos = false;
    let mut neg = false;
    let mut items = Vec::with_capacity(values.len());
    for i in 0..values.len() {
        let v = values[i];
        if !v.is_finite() {
            return None;
        }
        let days = dates[i].checked_sub(d0)?;
        if days < 0 {
            return None;
        }
        if v > 0.0 {
            pos = true;
        } else if v < 0.0 {
            neg = true;
        }
        items.push((days, v));
    }
    if !pos || !neg {
        return None;
    }
    items.sort_unstable_by_key(|(d, _)| *d);
    let mut packed: Vec<(i32, f64)> = Vec::with_capacity(items.len());
    for (d, v) in items {
        if let Some(last) = packed.last_mut() {
            if last.0 == d {
                last.1 += v;
                continue;
            }
        }
        packed.push((d, v));
    }
    packed.retain(|(_, v)| *v != 0.0);
    Some(packed)
}

fn newton(
    flows: &[(i32, f64)],
    guess: f64,
    npv_deriv: fn(&[(i32, f64)], f64) -> Option<(f64, f64)>,
) -> Option<f64> {
    let mut r = guess;
    for _ in 0..MAX_ITERS {
        let (npv, deriv) = npv_deriv(flows, r)?;
        if !npv.is_finite() {
            return None;
        }
        if npv.abs() == 0.0 {
            return Some(clean_rate(r));
        }
        if !deriv.is_finite() || deriv.abs() < DERIV_MIN {
            return None;
        }
        let r1 = r - npv / deriv;
        if !r1.is_finite() || !rate_ok(r1) {
            return None;
        }
        // Excel documents 1e-8; polish to ~1e-12 so 15-digit goldens line up.
        if (r1 - r).abs() <= 1e-12 {
            return Some(clean_rate(r1));
        }
        r = r1;
    }
    None
}

fn bisect(
    flows: &[(i32, f64)],
    guess: f64,
    npv_deriv: fn(&[(i32, f64)], f64) -> Option<(f64, f64)>,
) -> Option<f64> {
    let mut probes = vec![
        -0.999999,
        -0.99,
        -0.5,
        -0.1,
        0.0,
        0.1,
        0.25,
        0.5,
        1.0,
        2.0,
        10.0,
        100.0,
        1_000.0,
        1_000_000.0,
        1e12,
    ];
    if rate_ok(guess) {
        probes.push(guess);
    }
    probes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    probes.dedup_by(|a, b| (*a - *b).abs() < 1e-15);

    let mut prev: Option<(f64, f64)> = None;
    let mut bracket: Option<(f64, f64, f64, f64)> = None;
    for p in probes {
        if !rate_ok(p) {
            continue;
        }
        let Some((f, _)) = npv_deriv(flows, p) else {
            continue;
        };
        if !f.is_finite() {
            continue;
        }
        if f.abs() == 0.0 {
            return newton(flows, p, npv_deriv).or(Some(clean_rate(p)));
        }
        if let Some((pr, pf)) = prev {
            if pf * f <= 0.0 {
                bracket = Some((pr, pf, p, f));
                break;
            }
        }
        prev = Some((p, f));
    }
    let (mut lo, mut flo, mut hi, mut fhi) = bracket?;
    if lo > hi {
        std::mem::swap(&mut lo, &mut hi);
        std::mem::swap(&mut flo, &mut fhi);
    }
    for _ in 0..MAX_ITERS {
        let mid = 0.5 * (lo + hi);
        if !mid.is_finite() || !rate_ok(mid) {
            return None;
        }
        if (hi - lo).abs() <= RATE_TOL {
            return newton(flows, mid, npv_deriv).or(Some(clean_rate(mid)));
        }
        let (fm, _) = npv_deriv(flows, mid)?;
        if !fm.is_finite() {
            return None;
        }
        if fm.abs() == 0.0 {
            return newton(flows, mid, npv_deriv).or(Some(clean_rate(mid)));
        }
        if flo * fm <= 0.0 {
            hi = mid;
            fhi = fm;
        } else {
            lo = mid;
            flo = fm;
        }
        let _ = fhi;
    }
    newton(flows, 0.5 * (lo + hi), npv_deriv).or_else(|| Some(clean_rate(0.5 * (lo + hi))))
}

#[inline]
fn rate_ok(r: f64) -> bool {
    r.is_finite() && r > -1.0
}

#[inline]
fn clean_rate(r: f64) -> f64 {
    if r.abs() < NEAR_ZERO_RATE {
        0.0
    } else {
        r
    }
}

/// Horner-style `exp` / `ln1p` evaluation of XNPV and XNPV'.
fn xnpv_deriv_exp(flows: &[(i32, f64)], rate: f64) -> Option<(f64, f64)> {
    if !rate_ok(rate) {
        return None;
    }
    let one = 1.0 + rate;
    let ln1p = rate.ln_1p();
    if !ln1p.is_finite() {
        return None;
    }
    let inv_year = 1.0 / DAYS_PER_YEAR;
    let k = -ln1p * inv_year;
    if !k.is_finite() {
        return None;
    }
    let mut npv = 0.0;
    let mut weighted = 0.0;
    for &(days, v) in flows {
        let discount = if days == 0 {
            1.0
        } else {
            let log_term = k * f64::from(days);
            if !log_term.is_finite() || log_term.abs() > 700.0 {
                return None;
            }
            log_term.exp()
        };
        if !discount.is_finite() {
            return None;
        }
        npv += v * discount;
        weighted += v * f64::from(days) * discount;
        if !npv.is_finite() || !weighted.is_finite() {
            return None;
        }
    }
    let deriv = -weighted * inv_year / one;
    if !deriv.is_finite() {
        return None;
    }
    Some((npv, deriv))
}

/// Per-term `powf` XNPV / XNPV' (bench baseline).
fn xnpv_deriv_pow(flows: &[(i32, f64)], rate: f64) -> Option<(f64, f64)> {
    if !rate_ok(rate) {
        return None;
    }
    let one = 1.0 + rate;
    let mut npv = 0.0;
    let mut deriv = 0.0;
    for &(days, v) in flows {
        if days == 0 {
            npv += v;
            continue;
        }
        let t = f64::from(days) / DAYS_PER_YEAR;
        let pow_t = one.powf(t);
        if !pow_t.is_finite() || pow_t == 0.0 {
            return None;
        }
        npv += v / pow_t;
        deriv += v * (-t) / (pow_t * one);
        if !npv.is_finite() || !deriv.is_finite() {
            return None;
        }
    }
    Some((npv, deriv))
}

/// Flatten a `values` or `dates` argument into a packed series.
///
/// Range / array: keep numbers and blanks-as-0; reject text / logicals.
/// Scalar: `SUM`-style coerce.
pub fn collect_series(v: &ExcelValue, from_range: bool) -> Result<Vec<f64>, ExcelError> {
    let mut out = Vec::new();
    collect_series_into(v, from_range, &mut out)?;
    Ok(out)
}

fn collect_series_into(
    v: &ExcelValue,
    from_range: bool,
    out: &mut Vec<f64>,
) -> Result<(), ExcelError> {
    match (v, from_range) {
        (ExcelValue::Array(rows), _) => {
            for row in rows {
                for c in row {
                    collect_series_into(c, true, out)?;
                }
            }
            Ok(())
        }
        (ExcelValue::Error(e), _) => Err(*e),
        (ExcelValue::Number(n), _) => {
            if !n.is_finite() {
                return Err(ExcelError::Num);
            }
            out.push(*n);
            Ok(())
        }
        (ExcelValue::Empty, true) => {
            out.push(0.0);
            Ok(())
        }
        (ExcelValue::Bool(_) | ExcelValue::Text(_), true) => Err(ExcelError::Value),
        (other, false) => match coerce::to_number(other) {
            Ok(n) if n.is_finite() => {
                out.push(n);
                Ok(())
            }
            Ok(_) => Err(ExcelError::Num),
            Err(e) => Err(e),
        },
    }
}

/// Truncate and accept a date serial. Invalid (negative / past 9999-12-31)
/// is `#VALUE!` per Microsoft XIRR, not the `#NUM!` used by `DATE`.
pub fn date_serial_trunc(n: f64, system: DateSystem) -> Result<i32, ExcelError> {
    match truncate_date_serial(n, system) {
        Ok(s) => Ok(s),
        Err(ExcelError::Num) => Err(ExcelError::Value),
        Err(e) => Err(e),
    }
}

pub(crate) fn eval(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 || args.len() > 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let values_v = ev.eval_expr(&args[0], ctx)?;
    let values = match collect_series(&values_v, args[0].is_reference()) {
        Ok(v) => v,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let dates_v = ev.eval_expr(&args[1], ctx)?;
    let dates_raw = match collect_series(&dates_v, args[1].is_reference()) {
        Ok(v) => v,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let system = ctx.spec.options.date_system;
    let mut dates = Vec::with_capacity(dates_raw.len());
    for n in dates_raw {
        match date_serial_trunc(n, system) {
            Ok(s) => dates.push(s),
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    }
    let guess = if args.len() == 3 {
        match coerce::to_number(&ev.eval_scalar(&args[2], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        0.1
    };
    match xirr(&values, &dates, guess) {
        Some(r) => Ok(ExcelValue::Number(r)),
        None => Ok(ExcelValue::Error(ExcelError::Num)),
    }
}

/// 9999-12-31 in the 1900 system; used by unit tests.
#[allow(dead_code)]
const _MAX: i32 = EXCEL_MAX_SERIAL_1900;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dates::date_serial;
    use xlsx_types::{excel_num_eq, Cell, Sheet, Workbook};

    fn close(a: f64, b: f64) {
        let scale = a.abs().max(b.abs()).max(1.0);
        assert!(
            (a - b).abs() / scale <= 1e-9 || excel_num_eq(a, b),
            "xirr mismatch: {a} vs {b}"
        );
    }

    fn both(values: &[f64], dates: &[i32], guess: f64) -> Option<f64> {
        let fast = xirr(values, dates, guess);
        let slow = xirr_naive(values, dates, guess);
        match (fast, slow) {
            (Some(a), Some(b)) => {
                let scale = a.abs().max(b.abs()).max(1.0);
                assert!(
                    (a - b).abs() / scale <= 1e-9,
                    "naive/fast mismatch: {a} vs {b} for {values:?} {dates:?} guess={guess}"
                );
                Some(a)
            }
            (None, None) => None,
            other => panic!("naive/fast Option mismatch: {other:?}"),
        }
    }

    fn d(y: i32, m: i32, day: i32) -> i32 {
        date_serial(y, m, day, DateSystem::Excel1900).unwrap() as i32
    }

    #[test]
    fn microsoft_example() {
        let values = [-10000.0, 2750.0, 4250.0, 3250.0, 2750.0];
        let dates = [
            d(2008, 1, 1),
            d(2008, 3, 1),
            d(2008, 10, 30),
            d(2009, 2, 15),
            d(2009, 4, 1),
        ];
        assert_eq!(dates[0], 39448);
        let r = both(&values, &dates, 0.1).unwrap();
        close(r, 0.3733625335188315);
    }

    #[test]
    fn one_year_ten_percent() {
        let r = both(&[-100.0, 110.0], &[39448, 39448 + 365], 0.1).unwrap();
        close(r, 0.1);
    }

    #[test]
    fn leap_year_is_366_serial_days() {
        // 2008-01-01 → 2009-01-01 is 366 serial days; year basis is still 365.
        let r = both(&[-100.0, 110.0], &[d(2008, 1, 1), d(2009, 1, 1)], 0.1).unwrap();
        close(r, 1.1_f64.powf(365.0 / 366.0) - 1.0);
    }

    #[test]
    fn serial_difference_includes_leap_bug() {
        // Civil 1900-02-28 → 1900-03-01 is 1 day; Excel serials 59 → 61 are 2.
        let excel = both(&[-100.0, 110.0], &[59, 61], 0.1).unwrap();
        let two_day = 1.1_f64.powf(365.0 / 2.0) - 1.0;
        close(excel, two_day);
        let civil_wrong = 1.1_f64.powf(365.0) - 1.0;
        assert!(
            !excel_num_eq(excel, civil_wrong),
            "must not use Gregorian day counts"
        );
    }

    #[test]
    fn serial_60_is_a_valid_origin() {
        let r = both(&[-100.0, 110.0], &[60, 60 + 365], 0.1).unwrap();
        close(r, 0.1);
    }

    #[test]
    fn two_roots_follow_guess() {
        let v = [-100.0, 230.0, -132.0];
        let dates = [0, 365, 730];
        let lo = both(&v, &dates, 0.05).unwrap();
        let hi = both(&v, &dates, 0.25).unwrap();
        close(lo, 0.1);
        close(hi, 0.2);
    }

    #[test]
    fn newton_miss_recovers_via_bisection() {
        // Same series as Excel IRR's two-year example: Newton from 0.1 steps
        // to r <= -1; bisection still finds the XNPV = 0 root.
        let v = [-70000.0, 12000.0, 15000.0];
        let dates = [0, 365, 730];
        let r = both(&v, &dates, 0.1).unwrap();
        close(r, -0.443506941334741);
    }

    #[test]
    fn microsoft_five_year_annual() {
        let v = [-70000.0, 12000.0, 15000.0, 18000.0, 21000.0, 26000.0];
        let dates = [0, 365, 730, 1095, 1460, 1825];
        let r = both(&v, &dates, 0.1).unwrap();
        close(r, 0.0866309480365316);
    }

    #[test]
    fn no_sign_change_is_num() {
        assert_eq!(both(&[10.0, 20.0], &[1, 366], 0.1), None);
        assert_eq!(both(&[-10.0, -20.0], &[1, 366], 0.1), None);
        assert_eq!(both(&[0.0, 0.0], &[1, 366], 0.1), None);
        assert_eq!(both(&[-100.0], &[1], 0.1), None);
    }

    #[test]
    fn precede_is_num() {
        assert_eq!(both(&[-100.0, 110.0], &[39508, 39448], 0.1), None);
    }

    #[test]
    fn length_mismatch_and_empty() {
        assert_eq!(both(&[1.0, 2.0], &[1], 0.1), None);
        assert_eq!(both(&[], &[], 0.1), None);
    }

    #[test]
    fn guess_at_minus_one_is_num() {
        assert_eq!(both(&[-100.0, 110.0], &[39448, 39813], -1.0), None);
    }

    #[test]
    fn same_day_cancel_is_zero() {
        let r = both(&[-100.0, 40.0, 60.0], &[10, 10, 10], 0.1).unwrap();
        assert_eq!(r, 0.0);
    }

    #[test]
    fn same_day_nonzero_is_num() {
        assert_eq!(both(&[-100.0, 40.0], &[10, 10], 0.1), None);
    }

    #[test]
    fn later_dates_may_be_unsorted() {
        let values = [-10000.0, 4250.0, 2750.0, 2750.0, 3250.0];
        let dates = [
            d(2008, 1, 1),
            d(2008, 10, 30),
            d(2008, 3, 1),
            d(2009, 4, 1),
            d(2009, 2, 15),
        ];
        close(both(&values, &dates, 0.1).unwrap(), 0.3733625335188315);
    }

    #[test]
    fn rate_of_zero() {
        let r = both(&[-100.0, 100.0], &[39448, 39813], 0.1).unwrap();
        assert_eq!(r, 0.0);
    }

    #[test]
    fn date_serial_trunc_is_value_on_invalid() {
        assert_eq!(
            date_serial_trunc(-1.0, DateSystem::Excel1900),
            Err(ExcelError::Value)
        );
        assert_eq!(
            date_serial_trunc((EXCEL_MAX_SERIAL_1900 as f64) + 1.0, DateSystem::Excel1900),
            Err(ExcelError::Value)
        );
        assert_eq!(date_serial_trunc(60.9, DateSystem::Excel1900).unwrap(), 60);
        assert_eq!(date_serial_trunc(0.0, DateSystem::Excel1900).unwrap(), 0);
    }

    #[test]
    fn workbook_blank_date_is_num() {
        let mut sheet = Sheet::new("Sheet1");
        sheet
            .cells
            .insert("A1".into(), Cell::value(ExcelValue::Number(-100.0)));
        sheet
            .cells
            .insert("A2".into(), Cell::value(ExcelValue::Number(110.0)));
        sheet
            .cells
            .insert("B1".into(), Cell::value(ExcelValue::Number(39448.0)));
        let wb = Workbook {
            sheets: vec![sheet],
            names: vec![],
        };
        assert_eq!(
            crate::eval::eval_formula_in(&wb, "=XIRR(A1:A2,B1:B2)").unwrap(),
            ExcelValue::Error(ExcelError::Num)
        );
    }

    #[test]
    fn workbook_blank_value_is_zero_flow() {
        let mut sheet = Sheet::new("Sheet1");
        sheet
            .cells
            .insert("A1".into(), Cell::value(ExcelValue::Number(-100.0)));
        sheet
            .cells
            .insert("A3".into(), Cell::value(ExcelValue::Number(110.0)));
        sheet
            .cells
            .insert("B1".into(), Cell::value(ExcelValue::Number(39448.0)));
        sheet
            .cells
            .insert("B2".into(), Cell::value(ExcelValue::Number(39448.0)));
        sheet
            .cells
            .insert("B3".into(), Cell::value(ExcelValue::Number(39813.0)));
        let wb = Workbook {
            sheets: vec![sheet],
            names: vec![],
        };
        let with_blank = crate::eval::eval_formula_in(&wb, "=XIRR(A1:A3,B1:B3)").unwrap();
        let with_zero =
            crate::eval::eval_formula_in(&wb, "=XIRR({-100,0,110},{39448,39448,39813})").unwrap();
        match (&with_blank, &with_zero) {
            (ExcelValue::Number(a), ExcelValue::Number(b)) => close(*a, *b),
            other => panic!("expected numbers, got {other:?}"),
        }
        close(
            match with_blank {
                ExcelValue::Number(n) => n,
                other => panic!("expected number, got {other:?}"),
            },
            0.1,
        );
    }
}
