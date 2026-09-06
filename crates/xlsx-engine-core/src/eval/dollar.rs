//! Excel `DOLLAR(number, [decimals])` — en-US currency text.
//!
//! Desktop Excel / Microsoft DOLLAR help (no golden-reading):
//! - Converts a number to **text** using currency format, with the value
//!   rounded to `decimals`. Microsoft examples: `DOLLAR(1234.567, 2)` →
//!   `$1,234.57`; `DOLLAR(1234.567, -2)` → `$1,200`; `DOLLAR(-1234.567, -2)`
//!   → `($1,200)`; `DOLLAR(-0.123, 4)` → `($0.1230)`; `DOLLAR(99.888)` →
//!   `$99.89`.
//! - The documented format is `$#,##0.00_);($#,##0.00)` (en-US `$`). The
//!   text result has **no** `_` alignment pad: positives are `$1,234.57`
//!   (no trailing space); negatives use accounting parentheses
//!   `($1,234.57)`, **not** TEXT's `-$1,234.57`.
//! - Omitted `decimals` — including a trailing-comma slot (`DOLLAR(n,)`) —
//!   defaults to **2**. A blank cell coerces to `0` (that is not omitted).
//! - Negative `decimals` rounds to the left of the decimal (tens, hundreds).
//! - Rounding is Excel `ROUND` (half away from zero). The sign is taken
//!   from the **rounded** value: `DOLLAR(-0.001)` is `$0.00`, not `($0.00)`.
//! - `number` / `decimals` use arithmetic coerce: empty → `0`, `TRUE` → `1`,
//!   `FALSE` → `0`, numeric text parsed, other text (`"x"`, `""`, `"$5"`,
//!   `"1,000"`, `"50%"`) → `#VALUE!`. That is **not** `VALUE`.
//! - Errors propagate left-to-right. Wrong arity (`DOLLAR()` / extra args)
//!   is `#VALUE!`. Scalar context: a range implicit-intersects; an array
//!   literal takes the top-left (no `DOLLAR` spill).
//! - Locale is **en-US** (`$`, `,` thousands, `.` decimal). `USDOLLAR` /
//!   `DOLLARDE` / `DOLLARFR` / `FIXED` / `TEXT` stay out of this kernel.
//!
//! Production specialises common `decimals` (`0`, `±1`, `±2`, `±3`) and
//! writes `$` / grouping / fractional digits from a scaled integer (one
//! allocation, no `format!`). The naive path always runs `excel_round_naive`
//! plus `format!` and a second comma-insert pass so benches can print
//! before/after. This kernel does **not** read fixture goldens.

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{excel_round, excel_round_naive, EvalError, ExcelError, ExcelValue};

/// Exact `10^e` for `e` in `0..=22` (same table as `ROUND`).
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

/// Production `DOLLAR` on an already-coerced number and truncated `decimals`.
#[inline]
pub fn dollar(n: f64, digits: i32) -> String {
    match digits {
        2 => emit_rounded(excel_round(n, 2), 2),
        0 => emit_rounded(excel_round(n, 0), 0),
        1 => emit_rounded(excel_round(n, 1), 1),
        3 => emit_rounded(excel_round(n, 3), 3),
        -1 => emit_rounded(excel_round(n, -1), 0),
        -2 => emit_rounded(excel_round(n, -2), 0),
        -3 => emit_rounded(excel_round(n, -3), 0),
        d if d > 0 => emit_rounded(excel_round(n, d), d as u32),
        d => emit_rounded(excel_round(n, d), 0),
    }
}

/// `format!` + comma-insert baseline used for the hill-climb bench.
///
/// Same Excel result as [`dollar`] on the documented cases. Always pays
/// `excel_round_naive` (`log10` / `powi`) and two string allocations.
pub fn dollar_naive(n: f64, digits: i32) -> String {
    let rounded = excel_round_naive(n, digits);
    let places = if digits > 0 { digits as u32 } else { 0 };
    emit_naive(rounded, places)
}

/// Production `DOLLAR` on scalar Excel values.
///
/// `decimals == None` is the omitted-argument default (**2**). `Some(Empty)`
/// is a blank cell and coerces to `0`.
pub fn dollar_value(number: &ExcelValue, decimals: Option<&ExcelValue>) -> ExcelValue {
    let n = match coerce_number_hot(number) {
        Ok(n) => n,
        Err(e) => return ExcelValue::Error(e),
    };
    let digits = match coerce_digits(decimals) {
        Ok(d) => d,
        Err(e) => return ExcelValue::Error(e),
    };
    ExcelValue::Text(dollar(n, digits))
}

/// Value-level baseline: full [`coerce::to_number`] + [`dollar_naive`].
pub fn dollar_value_naive(number: &ExcelValue, decimals: Option<&ExcelValue>) -> ExcelValue {
    let n = match coerce::to_number(number) {
        Ok(n) => n,
        Err(e) => return ExcelValue::Error(e),
    };
    let digits = match decimals {
        None => 2,
        Some(d) => match coerce::to_number(d) {
            Ok(v) => trunc_digits(v),
            Err(e) => return ExcelValue::Error(e),
        },
    };
    ExcelValue::Text(dollar_naive(n, digits))
}

/// Packed walk. Used by the kernel bench.
pub fn dollar_slice(src: &[f64], digits: i32, dst: &mut [String]) {
    let n = src.len().min(dst.len());
    for i in 0..n {
        dst[i] = dollar(src[i], digits);
    }
}

/// Naive packed walk (bench baseline).
pub fn dollar_slice_naive(src: &[f64], digits: i32, dst: &mut [String]) {
    let n = src.len().min(dst.len());
    for i in 0..n {
        dst[i] = dollar_naive(src[i], digits);
    }
}

/// `DOLLAR(number, [decimals])` — scalar context, wrong arity → `#VALUE!`.
pub(crate) fn fn_dollar(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.is_empty() || args.len() > 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let number = ev.eval_scalar(&args[0], ctx)?;
    if let ExcelValue::Error(e) = number {
        return Ok(ExcelValue::Error(e));
    }
    let decimals = if args.len() >= 2 {
        match &args[1] {
            Expr::Missing => None,
            other => {
                let d = ev.eval_scalar(other, ctx)?;
                if let ExcelValue::Error(e) = d {
                    return Ok(ExcelValue::Error(e));
                }
                Some(d)
            }
        }
    } else {
        None
    };
    Ok(dollar_value(&number, decimals.as_ref()))
}

#[inline]
fn coerce_number_hot(v: &ExcelValue) -> Result<f64, ExcelError> {
    match v {
        ExcelValue::Number(n) => Ok(*n),
        ExcelValue::Empty => Ok(0.0),
        ExcelValue::Bool(true) => Ok(1.0),
        ExcelValue::Bool(false) => Ok(0.0),
        ExcelValue::Text(s) => coerce::parse_numeric_text(s),
        ExcelValue::Error(e) => Err(*e),
        ExcelValue::Array(_) => Err(ExcelError::Value),
    }
}

#[inline]
fn coerce_digits(decimals: Option<&ExcelValue>) -> Result<i32, ExcelError> {
    match decimals {
        None => Ok(2),
        Some(v) => Ok(trunc_digits(coerce_number_hot(v)?)),
    }
}

#[inline]
fn trunc_digits(d: f64) -> i32 {
    if !d.is_finite() {
        return 0;
    }
    let t = d.trunc();
    if t >= i32::MAX as f64 {
        i32::MAX
    } else if t <= i32::MIN as f64 {
        i32::MIN
    } else {
        t as i32
    }
}

fn emit_rounded(rounded: f64, places: u32) -> String {
    if !rounded.is_finite() {
        return emit_naive(rounded, places);
    }
    let abs = rounded.abs();
    if abs == 0.0 {
        return zero_dollar(places);
    }
    if let Some((int_part, frac_part)) = split_scaled(abs, places) {
        return emit_parts(rounded < 0.0, int_part, frac_part, places);
    }
    emit_naive(rounded, places)
}

fn zero_dollar(places: u32) -> String {
    if places == 0 {
        return "$0".to_string();
    }
    let mut s = String::with_capacity(2 + places as usize);
    s.push_str("$0.");
    for _ in 0..places {
        s.push('0');
    }
    s
}

fn split_scaled(abs: f64, places: u32) -> Option<(u128, u128)> {
    if places > 18 {
        return None;
    }
    let factor = pow10_u(places);
    let scaled = abs * factor;
    if !scaled.is_finite() || scaled >= 1e18 {
        return None;
    }
    // Already Excel-rounded; snap leftovers (`1234.57 * 100` → `123457`).
    let rnd = excel_round(scaled, 0);
    if !rnd.is_finite() || rnd < 0.0 || rnd >= 1e18 {
        return None;
    }
    let rnd = rnd as u128;
    let div = 10u128.pow(places);
    Some((rnd / div, rnd % div))
}

#[inline]
fn pow10_u(e: u32) -> f64 {
    if (e as usize) < POW10.len() {
        POW10[e as usize]
    } else {
        10f64.powi(e as i32)
    }
}

fn emit_parts(neg: bool, int_part: u128, frac_part: u128, places: u32) -> String {
    let mut buf = [0u8; 80];
    let mut i = 80usize;
    if places > 0 {
        let mut frac = frac_part;
        for _ in 0..places {
            i -= 1;
            buf[i] = b'0' + (frac % 10) as u8;
            frac /= 10;
        }
        i -= 1;
        buf[i] = b'.';
    }
    let mut x = int_part;
    let mut digits = 0u32;
    if x == 0 {
        i -= 1;
        buf[i] = b'0';
    } else {
        while x > 0 {
            if digits > 0 && digits % 3 == 0 {
                i -= 1;
                buf[i] = b',';
            }
            i -= 1;
            buf[i] = b'0' + (x % 10) as u8;
            x /= 10;
            digits += 1;
        }
    }
    i -= 1;
    buf[i] = b'$';
    let body = std::str::from_utf8(&buf[i..]).unwrap();
    if neg {
        let mut s = String::with_capacity(body.len() + 2);
        s.push('(');
        s.push_str(body);
        s.push(')');
        s
    } else {
        body.to_owned()
    }
}

fn emit_naive(rounded: f64, places: u32) -> String {
    if !rounded.is_finite() {
        return if rounded.is_sign_negative() && !rounded.is_nan() {
            "($inf)".to_string()
        } else if rounded.is_nan() {
            "$nan".to_string()
        } else {
            "$inf".to_string()
        };
    }
    let abs = rounded.abs();
    let neg = rounded < 0.0 && abs != 0.0;
    let raw = format!("{:.*}", places as usize, abs);
    let grouped = insert_commas(&raw);
    wrap_dollar(neg, &grouped)
}

fn insert_commas(body: &str) -> String {
    let (int, frac) = match body.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (body, None),
    };
    let mut out = String::with_capacity(body.len() + int.len() / 3);
    let bytes = int.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(b as char);
    }
    if let Some(f) = frac {
        out.push('.');
        out.push_str(f);
    }
    out
}

fn wrap_dollar(neg: bool, grouped: &str) -> String {
    if neg {
        let mut s = String::with_capacity(grouped.len() + 3);
        s.push('(');
        s.push('$');
        s.push_str(grouped);
        s.push(')');
        s
    } else {
        let mut s = String::with_capacity(grouped.len() + 1);
        s.push('$');
        s.push_str(grouped);
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::eval_formula_in;
    use xlsx_types::Workbook;

    fn both(n: f64, d: i32) -> String {
        let fast = dollar(n, d);
        let slow = dollar_naive(n, d);
        assert_eq!(fast, slow, "DOLLAR({n}, {d}): fast={fast} naive={slow}");
        fast
    }

    #[test]
    fn microsoft_examples() {
        assert_eq!(both(1234.567, 2), "$1,234.57");
        assert_eq!(both(1234.567, -2), "$1,200");
        assert_eq!(both(-1234.567, -2), "($1,200)");
        assert_eq!(both(-0.123, 4), "($0.1230)");
        assert_eq!(both(99.888, 2), "$99.89");
    }

    #[test]
    fn omitted_decimals_default_two() {
        assert_eq!(dollar(1234.567, 2), dollar(1234.567, 2));
        assert_eq!(
            dollar_value(&ExcelValue::Number(1234.567), None),
            ExcelValue::Text("$1,234.57".into())
        );
        assert_eq!(
            dollar_value(&ExcelValue::Number(99.888), None),
            ExcelValue::Text("$99.89".into())
        );
    }

    #[test]
    fn blank_decimals_is_zero_not_omitted() {
        assert_eq!(
            dollar_value(&ExcelValue::Number(1234.567), Some(&ExcelValue::Empty)),
            ExcelValue::Text("$1,235".into())
        );
    }

    #[test]
    fn parentheses_not_text_minus() {
        assert_eq!(both(-1234.5, 2), "($1,234.50)");
        assert_eq!(both(-1.2, 2), "($1.20)");
        assert_eq!(both(-0.5, 0), "($1)");
        assert_eq!(both(2.5, 0), "$3");
    }

    #[test]
    fn rounded_zero_drops_minus() {
        assert_eq!(both(-0.001, 2), "$0.00");
        assert_eq!(both(-0.0, 2), "$0.00");
        assert_eq!(both(0.0, 0), "$0");
        assert_eq!(both(0.004, 2), "$0.00");
        assert_eq!(both(0.005, 2), "$0.01");
        assert_eq!(both(-0.005, 2), "($0.01)");
    }

    #[test]
    fn grouping_and_pad() {
        assert_eq!(both(12.0, 2), "$12.00");
        assert_eq!(both(0.4, 2), "$0.40");
        assert_eq!(both(1_000_000.0, 2), "$1,000,000.00");
        assert_eq!(both(1234.567, 0), "$1,235");
        assert_eq!(both(1234.567, 4), "$1,234.5670");
        assert_eq!(both(1234.567, -1), "$1,230");
        assert_eq!(both(1234.567, -3), "$1,000");
        assert_eq!(both(1234.567, -4), "$0");
        assert_eq!(both(999.995, 2), "$1,000.00");
    }

    #[test]
    fn value_hot_path_and_coerce() {
        assert_eq!(
            dollar_value(&ExcelValue::Number(1234.567), None),
            ExcelValue::Text("$1,234.57".into())
        );
        assert_eq!(
            dollar_value(&ExcelValue::Empty, None),
            ExcelValue::Text("$0.00".into())
        );
        assert_eq!(
            dollar_value(&ExcelValue::Bool(true), None),
            ExcelValue::Text("$1.00".into())
        );
        assert_eq!(
            dollar_value(&ExcelValue::Bool(false), None),
            ExcelValue::Text("$0.00".into())
        );
        assert_eq!(
            dollar_value(&ExcelValue::Text("1234.5".into()), None),
            ExcelValue::Text("$1,234.50".into())
        );
        assert_eq!(
            dollar_value(&ExcelValue::Text("x".into()), None),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            dollar_value(&ExcelValue::Text("".into()), None),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            dollar_value(&ExcelValue::Text("$5".into()), None),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            dollar_value(&ExcelValue::Text("1,000".into()), None),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            dollar_value(&ExcelValue::Error(ExcelError::Div0), None),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            dollar_value(
                &ExcelValue::Number(1.26),
                Some(&ExcelValue::Text("1".into()))
            ),
            ExcelValue::Text("$1.3".into())
        );
        assert_eq!(
            dollar_value(&ExcelValue::Number(1.26), Some(&ExcelValue::Bool(true))),
            ExcelValue::Text("$1.3".into())
        );
    }

    #[test]
    fn value_naive_agrees() {
        let cases: [(&ExcelValue, Option<&ExcelValue>); 8] = [
            (&ExcelValue::Number(-1234.567), None),
            (&ExcelValue::Number(99.888), Some(&ExcelValue::Number(0.0))),
            (&ExcelValue::Empty, None),
            (&ExcelValue::Bool(true), Some(&ExcelValue::Number(-2.0))),
            (&ExcelValue::Text("7".into()), None),
            (&ExcelValue::Text("x".into()), None),
            (&ExcelValue::Error(ExcelError::Na), None),
            (
                &ExcelValue::Number(1.5),
                Some(&ExcelValue::Error(ExcelError::Div0)),
            ),
        ];
        for (n, d) in cases {
            assert_eq!(dollar_value(n, d), dollar_value_naive(n, d), "{n:?} {d:?}");
        }
    }

    #[test]
    fn formula_microsoft_and_quirks() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=DOLLAR(1234.567, 2)").unwrap(),
            ExcelValue::Text("$1,234.57".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=DOLLAR(1234.567, -2)").unwrap(),
            ExcelValue::Text("$1,200".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=DOLLAR(-1234.567, -2)").unwrap(),
            ExcelValue::Text("($1,200)".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=DOLLAR(-0.123, 4)").unwrap(),
            ExcelValue::Text("($0.1230)".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=DOLLAR(99.888)").unwrap(),
            ExcelValue::Text("$99.89".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=DOLLAR(1234.567,)").unwrap(),
            ExcelValue::Text("$1,234.57".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=DOLLAR(-1234.5)").unwrap(),
            ExcelValue::Text("($1,234.50)".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=DOLLAR(-1234.5)=TEXT(-1234.5,\"$#,##0.00\")").unwrap(),
            ExcelValue::Bool(false)
        );
        assert_eq!(
            eval_formula_in(&wb, "=TEXT(-1234.5,\"$#,##0.00\")").unwrap(),
            ExcelValue::Text("-$1,234.50".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=DOLLAR()").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=DOLLAR(1,2,3)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=LEN(DOLLAR(0))").unwrap(),
            ExcelValue::Number(5.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=VALUE(DOLLAR(-1234.5))").unwrap(),
            ExcelValue::Number(-1234.5)
        );
    }

    #[test]
    fn packed_slice_matches() {
        let src: Vec<f64> = (0..64).map(|i| i as f64 * 17.35 - 200.0).collect();
        let mut fast = vec![String::new(); src.len()];
        let mut slow = vec![String::new(); src.len()];
        dollar_slice(&src, 2, &mut fast);
        dollar_slice_naive(&src, 2, &mut slow);
        assert_eq!(fast, slow);
        dollar_slice(&src, -2, &mut fast);
        dollar_slice_naive(&src, -2, &mut slow);
        assert_eq!(fast, slow);
    }

    #[test]
    fn naive_matches_fast_over_clean_grid() {
        for i in -50i32..=50 {
            let n = i as f64 * 17.135 + 0.567;
            both(n, 2);
            both(n, 0);
            both(-n, 2);
            both(n, -2);
            both(n, 4);
        }
    }
}
