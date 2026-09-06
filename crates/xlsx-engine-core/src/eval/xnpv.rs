//! Excel `XNPV(rate, values, dates)` kernel.
//!
//! Desktop Excel / Microsoft XNPV help (no golden-reading):
//! - `Σ P_i / (1+rate)^((d_i − d_1) / 365)` on a 365-day year (not 365.25,
//!   not actual/actual). Day counts are **serial differences**, so the 1900
//!   leap-year bug (serial 60) is included when the span crosses it.
//! - Dates are truncated toward zero. The first date is the discount origin;
//!   every other date must be on or after it (a preceding date is `#NUM!`).
//!   Later dates may be unsorted.
//! - `values` and `dates` must contain the same number of entries (`#NUM!`
//!   otherwise). An empty series has no origin date (`#NUM!`).
//! - Invalid dates (negative / past 9999-12-31) are `#VALUE!`.
//! - Range / array blanks are **zeros**, not skips (unlike `NPV` / `IRR`):
//!   a blank date becomes serial 0 and typically `#NUM!` (precedes). Text and
//!   logicals in a range or array are nonnumeric (`#VALUE!`).
//! - Scalar (non-reference) arguments coerce like `SUM`: `TRUE`→1, `"100"`→100.
//! - Mixed signs are **not** required (Microsoft’s wording is copied from
//!   `XIRR`). `rate = -1` with a later (days > 0) flow is `#DIV/0!`.
//!
//! Production path uses `exp(ln1p(rate) * days / 365)` so each term is one
//! `exp`, not `powf`. [`xnpv_naive`] keeps the `powf` form for the bench.

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use crate::dates::{truncate_date_serial, EXCEL_MAX_SERIAL_1900};
use xlsx_types::{DateSystem, EvalError, ExcelError, ExcelValue};

const DAYS_PER_YEAR: f64 = 365.0;

/// Production `XNPV` kernel (`exp` / hoisted `ln1p` discount).
pub fn xnpv(rate: f64, values: &[f64], dates: &[i32]) -> Result<f64, ExcelError> {
    let d0 = origin(rate, values, dates)?;
    let one_plus = 1.0 + rate;
    if one_plus > 0.0 {
        // One `ln1p` for the whole series; each term is a single `exp`.
        let k = -rate.ln_1p() / DAYS_PER_YEAR;
        if !k.is_finite() {
            return Err(ExcelError::Num);
        }
        let mut sum = 0.0;
        let mut i = 0;
        while i < values.len() {
            if !values[i].is_finite() {
                return Err(ExcelError::Num);
            }
            let days = dates[i] - d0;
            if days < 0 {
                return Err(ExcelError::Num);
            }
            let mut flow = values[i];
            i += 1;
            while i < values.len() && dates[i] - d0 == days {
                if !values[i].is_finite() {
                    return Err(ExcelError::Num);
                }
                flow += values[i];
                i += 1;
            }
            if days == 0 {
                sum += flow;
            } else {
                let log_term = k * (days as f64);
                if !log_term.is_finite() || log_term.abs() > 700.0 {
                    return Err(ExcelError::Num);
                }
                sum += flow * log_term.exp();
            }
            if !sum.is_finite() {
                return Err(ExcelError::Num);
            }
        }
        return Ok(sum);
    }
    accumulate(rate, values, dates, d0, discount_powf)
}

/// Per-term `powf` baseline. Same Excel rules as [`xnpv`].
pub fn xnpv_naive(rate: f64, values: &[f64], dates: &[i32]) -> Result<f64, ExcelError> {
    let d0 = origin(rate, values, dates)?;
    accumulate(rate, values, dates, d0, discount_powf)
}

fn origin(rate: f64, values: &[f64], dates: &[i32]) -> Result<i32, ExcelError> {
    if values.len() != dates.len() || values.is_empty() {
        return Err(ExcelError::Num);
    }
    if !rate.is_finite() {
        return Err(ExcelError::Num);
    }
    Ok(dates[0])
}

fn accumulate(
    rate: f64,
    values: &[f64],
    dates: &[i32],
    d0: i32,
    discount: fn(f64, f64, i32) -> Result<f64, ExcelError>,
) -> Result<f64, ExcelError> {
    let mut sum = 0.0;
    for (i, &v) in values.iter().enumerate() {
        if !v.is_finite() {
            return Err(ExcelError::Num);
        }
        let days = dates[i] - d0;
        if days < 0 {
            return Err(ExcelError::Num);
        }
        if days == 0 {
            sum += v;
        } else {
            sum += v * discount(rate, 1.0 + rate, days)?;
        }
        if !sum.is_finite() {
            return Err(ExcelError::Num);
        }
    }
    Ok(sum)
}

/// `1 / (1+rate)^(days/365)` via `powf`.
fn discount_powf(_rate: f64, one_plus: f64, days: i32) -> Result<f64, ExcelError> {
    if one_plus == 0.0 {
        return Err(ExcelError::Div0);
    }
    let expn = days as f64 / DAYS_PER_YEAR;
    if one_plus < 0.0 && expn.fract() != 0.0 {
        return Err(ExcelError::Num);
    }
    let den = one_plus.powf(expn);
    if den == 0.0 {
        return Err(ExcelError::Div0);
    }
    if !den.is_finite() {
        return Err(ExcelError::Num);
    }
    finite(1.0 / den)
}

#[inline]
fn finite(n: f64) -> Result<f64, ExcelError> {
    if n.is_finite() {
        Ok(n)
    } else {
        Err(ExcelError::Num)
    }
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
/// is `#VALUE!` per Microsoft XNPV, not the `#NUM!` used by `DATE`.
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
    if args.len() != 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let rate = match coerce::to_number(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    if !rate.is_finite() {
        return Ok(ExcelValue::Error(ExcelError::Num));
    }
    let values_v = ev.eval_expr(&args[1], ctx)?;
    let values = match collect_series(&values_v, args[1].is_reference()) {
        Ok(v) => v,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let dates_v = ev.eval_expr(&args[2], ctx)?;
    let dates_raw = match collect_series(&dates_v, args[2].is_reference()) {
        Ok(v) => v,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    if values.len() != dates_raw.len() {
        return Ok(ExcelValue::Error(ExcelError::Num));
    }
    let system = ctx.spec.options.date_system;
    let mut dates = Vec::with_capacity(dates_raw.len());
    for n in dates_raw {
        match date_serial_trunc(n, system) {
            Ok(s) => dates.push(s),
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    }
    match xnpv(rate, &values, &dates) {
        Ok(n) => Ok(ExcelValue::Number(n)),
        Err(e) => Ok(ExcelValue::Error(e)),
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
        assert!(excel_num_eq(a, b), "xnpv mismatch: {a} vs {b}");
    }

    fn both(rate: f64, values: &[f64], dates: &[i32]) -> f64 {
        let fast = xnpv(rate, values, dates).unwrap();
        let slow = xnpv_naive(rate, values, dates).unwrap();
        close(fast, slow);
        fast
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
        close(both(0.09, &values, &dates), 2086.647602031535);
    }

    #[test]
    fn first_cashflow_undiscounted() {
        assert_eq!(both(0.1, &[100.0], &[39448]), 100.0);
        assert_eq!(both(0.1, &[-100.0, 40.0, 60.0], &[10, 10, 10]), 0.0);
    }

    #[test]
    fn rate_zero_sums() {
        assert_eq!(
            both(0.0, &[-10000.0, 2750.0, 4250.0], &[1, 10, 20]),
            -3000.0
        );
    }

    #[test]
    fn one_year_ten_percent() {
        close(both(0.1, &[-100.0, 110.0], &[39448, 39448 + 365]), 0.0);
    }

    #[test]
    fn serial_difference_includes_leap_bug() {
        // Civil 1900-02-28 → 1900-03-01 is 1 day; Excel serials 59 → 61 are 2.
        let excel = both(1.0, &[100.0, 100.0], &[59, 61]);
        close(excel, 199.62091367899785);
        let civil_wrong = 100.0 + 100.0 / 2.0_f64.powf(1.0 / 365.0);
        assert!(
            !excel_num_eq(excel, civil_wrong),
            "must not use Gregorian day counts"
        );
    }

    #[test]
    fn precede_is_num() {
        assert_eq!(
            xnpv(0.09, &[-100.0, 110.0], &[39508, 39448]),
            Err(ExcelError::Num)
        );
    }

    #[test]
    fn length_mismatch_and_empty() {
        assert_eq!(xnpv(0.1, &[1.0, 2.0], &[1]), Err(ExcelError::Num));
        assert_eq!(xnpv(0.1, &[], &[]), Err(ExcelError::Num));
    }

    #[test]
    fn rate_neg_one() {
        assert_eq!(
            xnpv(-1.0, &[-10000.0, 2750.0], &[39448, 39508]),
            Err(ExcelError::Div0)
        );
        assert_eq!(both(-1.0, &[-100.0, 40.0, 60.0], &[10, 10, 10]), 0.0);
    }

    #[test]
    fn negative_base_integer_year() {
        assert_eq!(both(-2.0, &[-100.0, 110.0], &[39448, 39448 + 365]), -210.0);
        assert_eq!(
            xnpv(-2.0, &[-100.0, 110.0], &[39448, 39449]),
            Err(ExcelError::Num)
        );
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
        close(both(0.09, &values, &dates), 2086.647602031535);
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
        // B2 blank → serial 0, precedes 39448.
        let wb = Workbook {
            sheets: vec![sheet],
            names: vec![],
        };
        assert_eq!(
            crate::eval::eval_formula_in(&wb, "=XNPV(0.1,A1:A2,B1:B2)").unwrap(),
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
            .insert("B1".into(), Cell::value(ExcelValue::Number(1000.0)));
        sheet
            .cells
            .insert("B2".into(), Cell::value(ExcelValue::Number(1000.0)));
        sheet
            .cells
            .insert("B3".into(), Cell::value(ExcelValue::Number(1365.0)));
        let wb = Workbook {
            sheets: vec![sheet],
            names: vec![],
        };
        let with_blank = crate::eval::eval_formula_in(&wb, "=XNPV(0.1,A1:A3,B1:B3)").unwrap();
        let with_zero =
            crate::eval::eval_formula_in(&wb, "=XNPV(0.1,{-100,0,110},{1000,1000,1365})").unwrap();
        match (with_blank, with_zero) {
            (ExcelValue::Number(a), ExcelValue::Number(b)) => close(a, b),
            other => panic!("expected numbers, got {other:?}"),
        }
    }
}
