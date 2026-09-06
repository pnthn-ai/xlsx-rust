//! Excel `TRUNC(number, [num_digits])` — toward zero.
//!
//! Shared kernel so `calc-core` and `seed-compliant` stay aligned. Does not
//! read fixture goldens — callers pass a coerced `f64` and truncated digits.
//!
//! Desktop Excel / Microsoft TRUNC help:
//! - Always truncates **toward zero** (same direction as `ROUNDDOWN`; not
//!   `INT` / `ROUND`). `TRUNC(-8.9)` is `-8`; `INT(-8.9)` is `-9`.
//!   `TRUNC(-0.5)` is `0`; `ROUND(-0.5, 0)` is `-1` (half away from zero).
//! - `num_digits` omitted (one-arg form, or a trailing-comma / blank slot)
//!   is **0** — drop the fractional part. Microsoft's examples are the
//!   one-arg form (`TRUNC(8.9)`, `TRUNC(PI())`).
//! - `num_digits > 0`: decimal places to the right. `< 0`: tens, hundreds, …
//!   to the left. Non-integers truncate toward zero (`2.9` → 2, `-1.9` → −1).
//! - Signed input: absolute value, toward-zero, reapply sign.
//! - Arithmetic coerce on both arguments (empty → 0, `TRUE` → 1, numeric
//!   text). Non-numeric text is `#VALUE!`. Errors evaluate left-to-right.
//! - Arity: 1 or 2 arguments. `TRUNC()` / three-or-more is `#VALUE!`.
//!
//! Production specialises the common `num_digits` (`0`, `±1`…`±4`) and
//! otherwise uses a table of exact `10^e` (e ≤ 22). Negative `num_digits`
//! divide by the integer `10^|d|` (never multiply by inexact `0.1`). A
//! 15-significant-digit snap keeps `TRUNC(1.15, 2)` at `1.15`. The naive
//! path issues two `powi` calls and a full `excel_round_15` (`log10` /
//! `powi`) snap so benches can print a before/after.

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{excel_round_15, EvalError, ExcelError, ExcelValue};

/// Exact `10^e` for `e` in `0..=22` (all representable as f64 integers).
const POW10: [f64; 23] = [
    1.0,
    10.0,
    100.0,
    1_000.0,
    10_000.0,
    100_000.0,
    1_000_000.0,
    10_000_000.0,
    100_000_000.0,
    1_000_000_000.0,
    10_000_000_000.0,
    100_000_000_000.0,
    1_000_000_000_000.0,
    10_000_000_000_000.0,
    100_000_000_000_000.0,
    1_000_000_000_000_000.0,
    10_000_000_000_000_000.0,
    100_000_000_000_000_000.0,
    1_000_000_000_000_000_000.0,
    10_000_000_000_000_000_000.0,
    100_000_000_000_000_000_000.0,
    1_000_000_000_000_000_000_000.0,
    10_000_000_000_000_000_000_000.0,
];

/// Production `TRUNC` kernel.
#[inline]
pub fn trunc(n: f64, digits: i32) -> f64 {
    match digits {
        0 => toward_zero_int(n),
        1 => scale_trunc(n, 10.0, true),
        2 => scale_trunc(n, 100.0, true),
        3 => scale_trunc(n, 1_000.0, true),
        4 => scale_trunc(n, 10_000.0, true),
        -1 => scale_trunc(n, 10.0, false),
        -2 => scale_trunc(n, 100.0, false),
        -3 => scale_trunc(n, 1_000.0, false),
        -4 => scale_trunc(n, 10_000.0, false),
        d => excel_trunc(n, d),
    }
}

/// First-draft kernel: two `powi` calls and a full `excel_round_15`
/// (`log10` / `powi`) snap before `trunc`. Same results on clean inputs;
/// benches print before/after against the cheap relative snap.
#[inline]
pub fn trunc_naive(n: f64, digits: i32) -> f64 {
    if !n.is_finite() {
        return n;
    }
    if n == 0.0 {
        return 0.0;
    }
    let e = digits.unsigned_abs() as i32;
    let p = 10f64.powi(e);
    let unscale = 10f64.powi(e);
    let sign = if n < 0.0 { -1.0 } else { 1.0 };
    let mag = n.abs();
    let scaled = if digits >= 0 { mag * p } else { mag / p };
    let rounded = excel_round_15(scaled).trunc();
    if digits >= 0 {
        sign * rounded / unscale
    } else {
        sign * rounded * unscale
    }
}

/// Apply [`trunc`] to every `(n[i], digits[i])`. Hot path for columns.
pub fn trunc_slice(n: &[f64], digits: &[i32], out: &mut [f64]) {
    let len = n.len().min(digits.len()).min(out.len());
    for i in 0..len {
        out[i] = trunc(n[i], digits[i]);
    }
}

/// Naive slice baseline matching [`trunc_naive`].
pub fn trunc_slice_naive(n: &[f64], digits: &[i32], out: &mut [f64]) {
    let len = n.len().min(digits.len()).min(out.len());
    for i in 0..len {
        out[i] = trunc_naive(n[i], digits[i]);
    }
}

/// Broadcast one `num_digits` across a packed walk.
pub fn trunc_slice_digits(n: &[f64], digits: i32, out: &mut [f64]) {
    let len = n.len().min(out.len());
    for i in 0..len {
        out[i] = trunc(n[i], digits);
    }
}

/// Naive broadcast matching [`trunc_naive`].
pub fn trunc_slice_digits_naive(n: &[f64], digits: i32, out: &mut [f64]) {
    let len = n.len().min(out.len());
    for i in 0..len {
        out[i] = trunc_naive(n[i], digits);
    }
}

/// `TRUNC(number, [num_digits])` — scalar context, omitted digits = 0.
pub(crate) fn fn_trunc(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.is_empty() || args.len() > 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    // Errors left-to-right: number first, then optional num_digits.
    let n = match coerce::to_number(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let digits = if args.len() == 2 && !args[1].is_omitted() {
        match coerce::to_number(&ev.eval_scalar(&args[1], ctx)?) {
            Ok(d) => d.trunc() as i32,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        0
    };
    Ok(ExcelValue::Number(trunc(n, digits)))
}

/// Integer `num_digits`: snap 15-digit leftovers (`6.999…9` stays 7) then
/// `trunc` toward zero.
#[inline]
fn toward_zero_int(n: f64) -> f64 {
    if !n.is_finite() {
        return n;
    }
    if n == 0.0 {
        return 0.0;
    }
    let sign = if n < 0.0 { -1.0 } else { 1.0 };
    sign * snap_15(n.abs()).trunc()
}

/// Scale by an exact integer power of ten (`p = 10^|digits|`).
/// `mul` is true for positive `num_digits` (multiply then divide).
#[inline]
fn scale_trunc(n: f64, p: f64, mul: bool) -> f64 {
    if n == 0.0 {
        return 0.0;
    }
    let sign = if n < 0.0 { -1.0 } else { 1.0 };
    let mag = n.abs();
    let scaled = if mul { mag * p } else { mag / p };
    let rounded = snap_15(scaled).trunc();
    if mul {
        sign * rounded / p
    } else {
        sign * rounded * p
    }
}

fn excel_trunc(n: f64, digits: i32) -> f64 {
    if !n.is_finite() {
        return n;
    }
    if n == 0.0 {
        return 0.0;
    }
    let e = digits.unsigned_abs();
    let p = pow10_u(e);
    scale_trunc(n, p, digits > 0)
}

/// Snap binary leftovers that agree to Excel's 15 significant digits.
///
/// `1.15 * 100` is `114.999…` in IEEE; without a snap, `TRUNC` would
/// incorrectly yield `1.14`.
#[inline]
fn snap_15(x: f64) -> f64 {
    let r = x.round();
    let tol = x.abs() * 1e-14 + 1e-14;
    if (x - r).abs() <= tol {
        r
    } else {
        x
    }
}

#[inline]
fn pow10_u(e: u32) -> f64 {
    let i = e as usize;
    if i < POW10.len() {
        POW10[i]
    } else {
        10f64.powi(e as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::eval_formula_in;
    use xlsx_types::Workbook;

    fn both(n: f64, d: i32) -> f64 {
        let fast = trunc(n, d);
        let slow = trunc_naive(n, d);
        assert_eq!(
            fast, slow,
            "trunc mismatch n={n} d={d}: fast={fast} naive={slow}"
        );
        fast
    }

    #[test]
    fn microsoft_trunc_examples() {
        assert_eq!(both(8.9, 0), 8.0);
        assert_eq!(both(-8.9, 0), -8.0);
        assert_eq!(both(std::f64::consts::PI, 0), 3.0);
    }

    #[test]
    fn toward_zero_not_int_or_round() {
        assert_eq!(both(-8.9, 0), -8.0);
        assert_eq!(both(-1.5, 0), -1.0);
        assert_eq!(both(-0.5, 0), 0.0);
        assert_eq!(both(-0.99, 0), 0.0);
        assert_eq!(both(0.99, 0), 0.0);
        assert_eq!(both(2.5, 0), 2.0);
        assert_eq!(both(-2.5, 0), -2.0);
    }

    #[test]
    fn optional_and_negative_num_digits() {
        assert_eq!(both(8.9, 0), 8.0);
        assert_eq!(both(3.14159, 2), 3.14);
        assert_eq!(both(-3.14159, 2), -3.14);
        assert_eq!(both(123.456, -1), 120.0);
        assert_eq!(both(-123.456, -1), -120.0);
        assert_eq!(both(31415.92654, -2), 31400.0);
        assert_eq!(both(2_345_678.0, -4), 2_340_000.0);
        assert_eq!(both(2785.2, -3), 2000.0);
    }

    #[test]
    fn already_at_precision_is_identity() {
        assert_eq!(both(8.0, 0), 8.0);
        assert_eq!(both(-8.0, 0), -8.0);
        assert_eq!(both(3.2, 1), 3.2);
        assert_eq!(both(1.1, 2), 1.1);
        assert_eq!(both(123.0, 0), 123.0);
    }

    #[test]
    fn ieee_leftover_does_not_drop() {
        // 1.15 * 100 is 114.999…; snap keeps 1.15.
        assert_eq!(both(1.15, 2), 1.15);
        assert_eq!(both(1.1, 2), 1.1);
        // 15-digit leftover on the integer path must not drop.
        assert_eq!(both(6.999999999999999, 0), 7.0);
    }

    #[test]
    fn zero_and_nonfinite() {
        assert_eq!(both(0.0, 5), 0.0);
        assert_eq!(both(-0.0, 2), 0.0);
        assert!(trunc(f64::NAN, 0).is_nan());
        assert!(trunc(f64::INFINITY, 0).is_infinite());
    }

    #[test]
    fn slice_matches_scalar() {
        let ns: Vec<f64> = (0..64).map(|i| i as f64 * 1.7 - 20.0).collect();
        let ds: Vec<i32> = (0..64).map(|i| (i as i32 % 9) - 4).collect();
        let mut out = vec![0.0; ns.len()];
        trunc_slice(&ns, &ds, &mut out);
        for ((n, d), got) in ns.iter().zip(ds.iter()).zip(out.iter()) {
            assert_eq!(*got, trunc(*n, *d));
        }
        let mut naive = vec![0.0; ns.len()];
        trunc_slice_naive(&ns, &ds, &mut naive);
        for ((n, d), got) in ns.iter().zip(ds.iter()).zip(naive.iter()) {
            assert_eq!(*got, trunc_naive(*n, *d));
        }
        let mut broadcast = vec![0.0; ns.len()];
        trunc_slice_digits(&ns, 0, &mut broadcast);
        for (n, got) in ns.iter().zip(broadcast.iter()) {
            assert_eq!(*got, trunc(*n, 0));
        }
    }

    #[test]
    fn naive_matches_fast_over_grid() {
        let digits = [-5, -4, -3, -2, -1, 0, 1, 2, 3, 4, 5];
        for i in -200i32..=200 {
            let n = i as f64 * 0.137 + 0.15;
            for &d in &digits {
                both(n, d);
                both(-n, d);
            }
        }
    }

    #[test]
    fn formula_microsoft_and_omitted_digits() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=TRUNC(8.9)").unwrap(),
            ExcelValue::Number(8.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=TRUNC(-8.9)").unwrap(),
            ExcelValue::Number(-8.0)
        );
        // Omitted num_digits defaults to 0.
        assert_eq!(
            eval_formula_in(&wb, "=TRUNC(8.9, 0)").unwrap(),
            ExcelValue::Number(8.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=TRUNC(-3.2,)").unwrap(),
            ExcelValue::Number(-3.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=TRUNC()").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=TRUNC(8.9, 0, 1)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn formula_vs_int_round_rounddown() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=TRUNC(-8.9)-INT(-8.9)").unwrap(),
            ExcelValue::Number(1.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=TRUNC(-0.5)=ROUND(-0.5, 0)").unwrap(),
            ExcelValue::Bool(false)
        );
        assert_eq!(
            eval_formula_in(&wb, "=TRUNC(-3.14159, 2)=ROUNDDOWN(-3.14159, 2)").unwrap(),
            ExcelValue::Bool(true)
        );
    }

    #[test]
    fn formula_errors_ltr_and_coerce() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=TRUNC(#DIV/0!, #N/A)").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=TRUNC(\"8.9\")").unwrap(),
            ExcelValue::Number(8.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=TRUNC(TRUE, -1)").unwrap(),
            ExcelValue::Number(0.0)
        );
    }
}
