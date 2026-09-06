//! Excel `MIRR(values, finance_rate, reinvest_rate)` kernel.
//!
//! Desktop Excel (Microsoft's MIRR help):
//! - `values` is one array / reference of periodic cash flows. Order is the
//!   period order. A stored `0` occupies a period; blanks, text, and logicals
//!   in a range or array are skipped (same compaction as [`super::irr`]).
//! - At least one inflow (`> 0`) and one outflow (`< 0`) are required;
//!   otherwise the worksheet function returns `#DIV/0!`.
//! - `n` is the number of kept cash flows. The closed form is
//!
//! ```text
//! ((−NPV(rrate, values⁺) · (1+rrate)^n)
//!   / (NPV(frate, values⁻) · (1+frate))) ^ (1/(n−1)) − 1
//! ```
//!
//! where `values⁺` / `values⁻` keep the original period slots (opposite-sign
//! flows become `0`) and `NPV` is Excel's period-1 discount ([`super::npv`]).
//! That identity is the same as compounding inflows to the last period at
//! `reinvest_rate` and discounting outflows to the first period at
//! `finance_rate`.
//!
//! Production evaluation streams two running discount factors (no `pow` per
//! period, no sign-masked `Vec`s). [`mirr_naive`] builds the masked series
//! and calls [`super::npv::npv_naive`] so the bench can print a before/after.

use super::npv::npv_naive;
use xlsx_types::ExcelError;

#[cfg(test)]
use super::npv::npv;

/// Production `MIRR` kernel (streaming NPV of each sign).
pub fn mirr(values: &[f64], finance_rate: f64, reinvest_rate: f64) -> Result<f64, ExcelError> {
    if !finance_rate.is_finite() || !reinvest_rate.is_finite() {
        return Err(ExcelError::Num);
    }
    let n = values.len();
    if n < 2 {
        return Err(ExcelError::Div0);
    }

    let one_f = 1.0 + finance_rate;
    let one_r = 1.0 + reinvest_rate;
    let mut fac_f = 1.0;
    let mut fac_r = 1.0;
    let mut npv_neg = 0.0;
    let mut npv_pos = 0.0;
    let mut has_pos = false;
    let mut has_neg = false;

    for &v in values {
        if !v.is_finite() {
            return Err(ExcelError::Num);
        }
        fac_f *= one_f;
        fac_r *= one_r;
        if v > 0.0 {
            has_pos = true;
            if reinvest_rate == -1.0 {
                return Err(ExcelError::Div0);
            }
            npv_pos += v / fac_r;
        } else if v < 0.0 {
            has_neg = true;
            if finance_rate == -1.0 {
                return Err(ExcelError::Div0);
            }
            npv_neg += v / fac_f;
        }
        if !npv_pos.is_finite() || !npv_neg.is_finite() {
            return Err(ExcelError::Num);
        }
    }

    if !has_pos || !has_neg {
        return Err(ExcelError::Div0);
    }
    mirr_from_npvs(n, npv_pos, npv_neg, finance_rate, reinvest_rate)
}

/// Per-term `pow` baseline: mask signs, then [`npv_naive`].
pub fn mirr_naive(
    values: &[f64],
    finance_rate: f64,
    reinvest_rate: f64,
) -> Result<f64, ExcelError> {
    if !finance_rate.is_finite() || !reinvest_rate.is_finite() {
        return Err(ExcelError::Num);
    }
    let n = values.len();
    if n < 2 {
        return Err(ExcelError::Div0);
    }
    if values.iter().any(|v| !v.is_finite()) {
        return Err(ExcelError::Num);
    }

    let mut has_pos = false;
    let mut has_neg = false;
    let mut pos = Vec::with_capacity(n);
    let mut neg = Vec::with_capacity(n);
    for &v in values {
        if v > 0.0 {
            has_pos = true;
            pos.push(v);
            neg.push(0.0);
        } else if v < 0.0 {
            has_neg = true;
            pos.push(0.0);
            neg.push(v);
        } else {
            pos.push(0.0);
            neg.push(0.0);
        }
    }
    if !has_pos || !has_neg {
        return Err(ExcelError::Div0);
    }

    let npv_pos = npv_naive(reinvest_rate, &pos)?;
    let npv_neg = npv_naive(finance_rate, &neg)?;
    mirr_from_npvs(n, npv_pos, npv_neg, finance_rate, reinvest_rate)
}

/// Microsoft closed form from the two Excel `NPV` results.
fn mirr_from_npvs(
    n: usize,
    npv_pos: f64,
    npv_neg: f64,
    finance_rate: f64,
    reinvest_rate: f64,
) -> Result<f64, ExcelError> {
    let n_f = n as f64;
    let grow = if n <= i32::MAX as usize {
        (1.0 + reinvest_rate).powi(n as i32)
    } else {
        (1.0 + reinvest_rate).powf(n_f)
    };
    if !grow.is_finite() {
        return Err(ExcelError::Num);
    }
    let denom = npv_neg * (1.0 + finance_rate);
    if denom == 0.0 {
        return Err(ExcelError::Div0);
    }
    let ratio = (-npv_pos * grow) / denom;
    if !ratio.is_finite() {
        return Err(ExcelError::Num);
    }
    if ratio < 0.0 {
        // Excel POWER: negative base + non-integer exponent is `#NUM!`.
        // `1/(n-1)` is an integer only for `n == 2` (exponent 1).
        if n != 2 {
            return Err(ExcelError::Num);
        }
        return finite(ratio - 1.0);
    }
    if ratio == 0.0 {
        return Ok(-1.0);
    }
    finite(ratio.powf(1.0 / (n_f - 1.0)) - 1.0)
}

#[inline]
fn finite(n: f64) -> Result<f64, ExcelError> {
    if n.is_finite() {
        Ok(n)
    } else {
        Err(ExcelError::Num)
    }
}

/// Horner [`npv`] of the sign-masked series — same identity as [`mirr`].
#[cfg(test)]
fn mirr_via_horner_npv(
    values: &[f64],
    finance_rate: f64,
    reinvest_rate: f64,
) -> Result<f64, ExcelError> {
    let n = values.len();
    let mut pos = Vec::with_capacity(n);
    let mut neg = Vec::with_capacity(n);
    let mut has_pos = false;
    let mut has_neg = false;
    for &v in values {
        if v > 0.0 {
            has_pos = true;
            pos.push(v);
            neg.push(0.0);
        } else if v < 0.0 {
            has_neg = true;
            pos.push(0.0);
            neg.push(v);
        } else {
            pos.push(0.0);
            neg.push(0.0);
        }
    }
    if !has_pos || !has_neg {
        return Err(ExcelError::Div0);
    }
    let npv_pos = npv(reinvest_rate, &pos)?;
    let npv_neg = npv(finance_rate, &neg)?;
    mirr_from_npvs(n, npv_pos, npv_neg, finance_rate, reinvest_rate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use xlsx_types::{Cell, ExcelValue, Sheet, Workbook};

    fn close(a: f64, b: f64) {
        let scale = a.abs().max(b.abs()).max(1.0);
        assert!((a - b).abs() / scale < 1e-12, "mirr mismatch: {a} vs {b}");
    }

    fn both(values: &[f64], fr: f64, rr: f64) -> Result<f64, ExcelError> {
        let fast = mirr(values, fr, rr);
        let slow = mirr_naive(values, fr, rr);
        let horner = if values.iter().all(|v| v.is_finite()) && values.len() >= 2 {
            mirr_via_horner_npv(values, fr, rr)
        } else {
            fast.clone()
        };
        match (fast, slow, horner) {
            (Ok(a), Ok(b), Ok(c)) => {
                close(a, b);
                close(a, c);
                Ok(a)
            }
            (Err(a), Err(b), Err(c)) if a == b && a == c => Err(a),
            other => panic!("mirr/naive/horner mismatch: {other:?}"),
        }
    }

    #[test]
    fn microsoft_five_year() {
        let v = [-120000.0, 39000.0, 30000.0, 21000.0, 37000.0, 46000.0];
        let r = both(&v, 0.1, 0.12).unwrap();
        close(r, 0.1260941303659051);
    }

    #[test]
    fn microsoft_three_year() {
        let v = [-120000.0, 39000.0, 30000.0, 21000.0];
        let r = both(&v, 0.1, 0.12).unwrap();
        close(r, -0.0480446552499808);
    }

    #[test]
    fn microsoft_five_year_reinvest_14() {
        let v = [-120000.0, 39000.0, 30000.0, 21000.0, 37000.0, 46000.0];
        let r = both(&v, 0.1, 0.14).unwrap();
        close(r, 0.1347591108283148);
    }

    #[test]
    fn simple_ten_percent() {
        close(both(&[-100.0, 110.0], 0.1, 0.1).unwrap(), 0.1);
        close(both(&[-100.0, 110.0], 0.0, 0.0).unwrap(), 0.1);
    }

    #[test]
    fn zero_cashflow_occupies_a_period() {
        close(both(&[-100.0, 0.0, 121.0], 0.0, 0.0).unwrap(), 0.1);
    }

    #[test]
    fn missing_sign_is_div0() {
        assert_eq!(both(&[10.0, 20.0], 0.1, 0.12), Err(ExcelError::Div0));
        assert_eq!(both(&[-10.0, -20.0], 0.1, 0.12), Err(ExcelError::Div0));
        assert_eq!(both(&[0.0, 0.0], 0.1, 0.12), Err(ExcelError::Div0));
        assert_eq!(both(&[-100.0], 0.1, 0.12), Err(ExcelError::Div0));
        assert_eq!(both(&[], 0.1, 0.12), Err(ExcelError::Div0));
    }

    #[test]
    fn rate_minus_one_is_div0() {
        assert_eq!(both(&[-100.0, 110.0], -1.0, 0.12), Err(ExcelError::Div0));
        assert_eq!(both(&[-100.0, 110.0], 0.1, -1.0), Err(ExcelError::Div0));
    }

    #[test]
    fn losing_project_can_be_negative() {
        let r = both(&[-100.0, 10.0, 10.0, 10.0], 0.1, 0.12).unwrap();
        close(r, -0.3038029359706866);
    }

    #[test]
    fn non_finite_rate_is_num() {
        assert_eq!(mirr(&[-100.0, 110.0], f64::NAN, 0.1), Err(ExcelError::Num));
        assert_eq!(
            mirr(&[-100.0, 110.0], 0.1, f64::INFINITY),
            Err(ExcelError::Num)
        );
    }

    #[test]
    fn grow_overflow_is_num() {
        let mut v = vec![1.0; 2000];
        v[0] = -1.0;
        assert_eq!(both(&v, 0.5, 0.5), Err(ExcelError::Num));
    }

    #[test]
    fn workbook_range_skips_blank_and_text() {
        let mut sheet = Sheet::new("Sheet1");
        sheet
            .cells
            .insert("A1".into(), Cell::value(ExcelValue::Number(-100.0)));
        sheet
            .cells
            .insert("A2".into(), Cell::value(ExcelValue::Text("x".into())));
        sheet
            .cells
            .insert("A3".into(), Cell::value(ExcelValue::Number(110.0)));
        let wb = Workbook {
            sheets: vec![sheet],
            names: vec![],
        };
        let skipped = crate::eval::eval_formula_in(&wb, "=MIRR(A1:A3,0,0)").unwrap();
        let compact = crate::eval::eval_formula_in(&wb, "=MIRR({-100,110},0,0)").unwrap();
        match (skipped, compact) {
            (ExcelValue::Number(a), ExcelValue::Number(b)) => {
                close(a, b);
                close(a, 0.1);
            }
            other => panic!("expected numbers, got {other:?}"),
        }
    }
}
