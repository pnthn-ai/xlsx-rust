//! Excel `FIXED(number, [decimals], [no_commas])` — ROUND then en-US text.
//!
//! Desktop Excel / Microsoft FIXED help (no golden-reading):
//! - Rounds `number` with `ROUND` (half away from zero; 15-digit leftover
//!   snap) and returns the result as **text**, not a number.
//! - Microsoft examples: `FIXED(1234.567, 1)` → `"1,234.6"`;
//!   `FIXED(1234.567, -1)` → `"1,230"`;
//!   `FIXED(-1234.567, -1, TRUE)` → `"-1230"`;
//!   `FIXED(44.332)` → `"44.33"`.
//! - Omitted `decimals` defaults to **2**. A blank cell is 0 (arithmetic
//!   coerce), which is not the same as an omitted slot (`FIXED(n,)` → 2).
//! - Negative `decimals` rounds left of the decimal (tens, hundreds, …)
//!   and the text has no decimal point.
//! - Microsoft: decimals can be as large as 127. After truncate-toward-zero,
//!   `decimals >= 128` is `#VALUE!`. Extra fractional digits past the
//!   rounded value are zeros (`FIXED(1, 6)` → `"1.000000"`).
//! - Omitted / FALSE `no_commas` inserts en-US thousands commas. TRUE
//!   (or nonzero) suppresses them. Text `no_commas` is `#VALUE!` (`IF`
//!   logical coerce — `"TRUE"` is not a boolean here).
//! - `number` uses arithmetic coerce: empty → `0`, `TRUE`/`FALSE` → 1/0,
//!   numeric text (`"1234.5"`, `"  7  "`, `"1E3"`) → parsed. Other text
//!   (`"x"`, `""`, `"$5"`, `"1,000"`) → `#VALUE!` (that is not `VALUE`).
//! - Errors propagate left-to-right. Wrong arity (`FIXED()` / 4+ args)
//!   is `#VALUE!`. Scalar context: a range implicit-intersects the host;
//!   an array literal takes the top-left (no `FIXED` spill).
//! - The minus sign is taken from the **rounded** value
//!   (`FIXED(-0.001, 2)` → `"0.00"`). The result is text: `TYPE` is 2,
//!   `ISTEXT` is TRUE, `VALUE(FIXED(…))` parses the en-US string back.
//!
//! Production writes digits into a stack buffer (common `|n| < 1e15`
//! path) after [`excel_round`](xlsx_types::excel_round). The naive path
//! always runs [`excel_round_naive`](xlsx_types::excel_round_naive)
//! (`log10` / `powi`) and builds the string with `format!` + a second
//! comma-insert allocation so benches can print before/after.
//! This kernel does **not** read fixture goldens. `DOLLAR` / `TEXT` /
//! `ROUND` stay on their own kernels.

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{excel_round, excel_round_naive, EvalError, ExcelError, ExcelValue};

/// Microsoft's documented maximum `decimals` (truncate toward zero first).
pub const MAX_DECIMALS: i32 = 127;

/// Production `FIXED` on an already-coerced number / decimals / flag.
pub fn fixed(n: f64, decimals: i32, no_commas: bool) -> Result<String, ExcelError> {
    emit(n, decimals, no_commas, false)
}

/// Allocating first-draft kernel used for the hill-climb bench.
///
/// Same Excel result as [`fixed`] on the documented cases. Kept so
/// `cargo bench -p xlsx-engine-core --bench fixed` can print before/after.
pub fn fixed_naive(n: f64, decimals: i32, no_commas: bool) -> Result<String, ExcelError> {
    emit(n, decimals, no_commas, true)
}

/// Shared after arguments are evaluated. `None` = omitted (not a blank).
pub fn fixed_apply(
    number: &ExcelValue,
    decimals: Option<&ExcelValue>,
    no_commas: Option<&ExcelValue>,
) -> ExcelValue {
    let n = match number_arg(number) {
        Ok(n) => n,
        Err(e) => return ExcelValue::Error(e),
    };
    let d = match decimals_arg(decimals) {
        Ok(d) => d,
        Err(e) => return ExcelValue::Error(e),
    };
    let nc = match no_commas_arg(no_commas) {
        Ok(b) => b,
        Err(e) => return ExcelValue::Error(e),
    };
    match fixed(n, d, nc) {
        Ok(s) => ExcelValue::Text(s),
        Err(e) => ExcelValue::Error(e),
    }
}

/// Value-level baseline: full [`coerce::to_number`] / [`coerce::to_logical`]
/// plus [`fixed_naive`].
pub fn fixed_apply_naive(
    number: &ExcelValue,
    decimals: Option<&ExcelValue>,
    no_commas: Option<&ExcelValue>,
) -> ExcelValue {
    let n = match coerce::to_number(number) {
        Ok(n) => n,
        Err(e) => return ExcelValue::Error(e),
    };
    let d = match decimals {
        None => 2,
        Some(v) => match coerce::to_number(v) {
            Ok(x) => match trunc_decimals(x) {
                Ok(d) => d,
                Err(e) => return ExcelValue::Error(e),
            },
            Err(e) => return ExcelValue::Error(e),
        },
    };
    let nc = match no_commas {
        None => false,
        Some(v) => match coerce::to_logical(v) {
            Ok(b) => b,
            Err(e) => return ExcelValue::Error(e),
        },
    };
    match fixed_naive(n, d, nc) {
        Ok(s) => ExcelValue::Text(s),
        Err(e) => ExcelValue::Error(e),
    }
}

/// Packed walk (constant decimals / flag). Used by the kernel bench.
pub fn fixed_slice(src: &[f64], decimals: i32, no_commas: bool, dst: &mut [String]) {
    let n = src.len().min(dst.len());
    for i in 0..n {
        dst[i] = fixed(src[i], decimals, no_commas).unwrap_or_else(|e| e.excel_text().to_string());
    }
}

/// Naive packed walk matching [`fixed_naive`].
pub fn fixed_slice_naive(src: &[f64], decimals: i32, no_commas: bool, dst: &mut [String]) {
    let n = src.len().min(dst.len());
    for i in 0..n {
        dst[i] =
            fixed_naive(src[i], decimals, no_commas).unwrap_or_else(|e| e.excel_text().to_string());
    }
}

/// `FIXED(number, [decimals], [no_commas])` — 1..=3 args; omitted decimals → 2.
pub(crate) fn fn_fixed(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.is_empty() || args.len() > 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    // Errors left-to-right: number, then optional decimals, then no_commas.
    let number = ev.eval_scalar(&args[0], ctx)?;
    if let ExcelValue::Error(e) = number {
        return Ok(ExcelValue::Error(e));
    }
    let decimals = if args.len() >= 2 && !args[1].is_omitted() {
        let v = ev.eval_scalar(&args[1], ctx)?;
        if let ExcelValue::Error(e) = v {
            return Ok(ExcelValue::Error(e));
        }
        Some(v)
    } else {
        None
    };
    let no_commas = if args.len() >= 3 && !args[2].is_omitted() {
        let v = ev.eval_scalar(&args[2], ctx)?;
        if let ExcelValue::Error(e) = v {
            return Ok(ExcelValue::Error(e));
        }
        Some(v)
    } else {
        None
    };
    Ok(fixed_apply(&number, decimals.as_ref(), no_commas.as_ref()))
}

fn number_arg(v: &ExcelValue) -> Result<f64, ExcelError> {
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

fn decimals_arg(v: Option<&ExcelValue>) -> Result<i32, ExcelError> {
    match v {
        None => Ok(2),
        Some(v) => trunc_decimals(number_arg(v)?),
    }
}

fn no_commas_arg(v: Option<&ExcelValue>) -> Result<bool, ExcelError> {
    match v {
        None => Ok(false),
        Some(v) => coerce::to_logical(v),
    }
}

fn trunc_decimals(x: f64) -> Result<i32, ExcelError> {
    if !x.is_finite() {
        return Err(ExcelError::Num);
    }
    if x >= (MAX_DECIMALS as f64) + 1.0 {
        return Err(ExcelError::Value);
    }
    // Clamp huge negatives so `powi` / `as i32` cannot wrap. Any finite
    // n rounded 400+ places left of the decimal is 0.
    if x <= -400.0 {
        return Ok(-400);
    }
    Ok(x.trunc() as i32)
}

fn emit(n: f64, decimals: i32, no_commas: bool, naive: bool) -> Result<String, ExcelError> {
    if decimals > MAX_DECIMALS {
        return Err(ExcelError::Value);
    }
    if !n.is_finite() {
        return Err(ExcelError::Num);
    }
    let rounded = if naive {
        excel_round_naive(n, decimals)
    } else {
        excel_round(n, decimals)
    };
    if naive {
        emit_naive(rounded, decimals, no_commas)
    } else {
        emit_fast(rounded, decimals, no_commas)
    }
}

fn emit_fast(rounded: f64, decimals: i32, no_commas: bool) -> Result<String, ExcelError> {
    let neg = rounded < 0.0 && rounded != 0.0;
    let mag = if rounded == 0.0 { 0.0 } else { rounded.abs() };
    if decimals <= 0 {
        return Ok(emit_int_fast(neg, mag, no_commas));
    }
    let places = decimals as u32;
    if mag < 1e15 && places <= 15 {
        if let Some(s) = emit_scaled_fast(neg, mag, places, no_commas) {
            return Ok(s);
        }
    }
    if mag < 1e15 && places > 15 {
        if let Some(mut s) = emit_scaled_fast(neg, mag, 15, no_commas) {
            s.extend(std::iter::repeat('0').take((places - 15) as usize));
            return Ok(s);
        }
    }
    emit_naive(rounded, decimals, no_commas)
}

fn emit_int_fast(neg: bool, mag: f64, no_commas: bool) -> String {
    if mag < 1e18 {
        let n = (mag + 0.5).floor() as u128;
        return emit_parts_fast(neg, n, 0, 0, no_commas);
    }
    let body = format!("{mag:.0}");
    decorate(neg, &body, None, no_commas)
}

fn emit_scaled_fast(neg: bool, mag: f64, places: u32, no_commas: bool) -> Option<String> {
    let p = pow10_u(places);
    let scaled = mag * p;
    if !scaled.is_finite() || scaled < 0.0 || scaled >= 1e18 {
        return None;
    }
    let q = (scaled + 0.5).floor() as u128;
    let div = 10u128.pow(places);
    Some(emit_parts_fast(neg, q / div, q % div, places, no_commas))
}

fn emit_parts_fast(
    neg: bool,
    int_part: u128,
    frac_part: u128,
    places: u32,
    no_commas: bool,
) -> String {
    // sign + 40 int digits + 13 commas + '.' + 127 frac
    let mut buf = [0u8; 192];
    let mut i = buf.len();
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
    if no_commas {
        i = write_u128(&mut buf, i, int_part);
    } else {
        i = write_grouped_u128(&mut buf, i, int_part);
    }
    if neg {
        i -= 1;
        buf[i] = b'-';
    }
    // SAFETY: only ASCII digits / comma / dot / minus.
    unsafe { std::str::from_utf8_unchecked(&buf[i..]).to_owned() }
}

fn write_u128(buf: &mut [u8], mut i: usize, mut n: u128) -> usize {
    if n == 0 {
        i -= 1;
        buf[i] = b'0';
        return i;
    }
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    i
}

fn write_grouped_u128(buf: &mut [u8], mut i: usize, mut n: u128) -> usize {
    if n == 0 {
        i -= 1;
        buf[i] = b'0';
        return i;
    }
    let mut digits = 0u32;
    while n > 0 {
        if digits > 0 && digits % 3 == 0 {
            i -= 1;
            buf[i] = b',';
        }
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        digits += 1;
    }
    i
}

fn emit_naive(rounded: f64, decimals: i32, no_commas: bool) -> Result<String, ExcelError> {
    let neg = rounded < 0.0 && rounded != 0.0;
    let mag = if rounded == 0.0 { 0.0 } else { rounded.abs() };
    let body = if decimals <= 0 {
        format!("{mag:.0}")
    } else {
        format!("{mag:.prec$}", prec = decimals as usize)
    };
    Ok(decorate(neg, &body, None, no_commas))
}

fn decorate(neg: bool, body: &str, frac_override: Option<&str>, no_commas: bool) -> String {
    let (int_part, frac) = match body.split_once('.') {
        Some((a, b)) => (a, Some(frac_override.unwrap_or(b))),
        None => (body, frac_override),
    };
    let grouped = if no_commas {
        int_part.to_owned()
    } else {
        insert_commas(int_part)
    };
    let mut s = String::with_capacity(grouped.len() + frac.map(|f| f.len() + 1).unwrap_or(0) + 1);
    if neg {
        s.push('-');
    }
    s.push_str(&grouped);
    if let Some(f) = frac {
        s.push('.');
        s.push_str(f);
    }
    s
}

fn insert_commas(int_part: &str) -> String {
    let b = int_part.as_bytes();
    if b.len() <= 3 {
        return int_part.to_owned();
    }
    let extra = (b.len() - 1) / 3;
    let mut out = String::with_capacity(b.len() + extra);
    for (i, &c) in b.iter().enumerate() {
        if i > 0 && (b.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c as char);
    }
    out
}

fn pow10_u(e: u32) -> f64 {
    const POW10: [f64; 16] = [
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
    ];
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
    use xlsx_types::{Cell, Sheet, Workbook};

    fn both(n: f64, d: i32, nc: bool) -> String {
        let fast = fixed(n, d, nc).expect("fast");
        let slow = fixed_naive(n, d, nc).expect("naive");
        assert_eq!(
            fast, slow,
            "FIXED({n}, {d}, {nc}): fast={fast} naive={slow}"
        );
        fast
    }

    #[test]
    fn microsoft_examples() {
        assert_eq!(both(1234.567, 1, false), "1,234.6");
        assert_eq!(both(1234.567, -1, false), "1,230");
        assert_eq!(both(-1234.567, -1, true), "-1230");
        assert_eq!(both(44.332, 2, false), "44.33");
    }

    #[test]
    fn omitted_decimals_is_two() {
        assert_eq!(both(44.332, 2, false), "44.33");
        assert_eq!(both(1000.0, 2, false), "1,000.00");
        assert_eq!(both(1234.567, 2, false), "1,234.57");
    }

    #[test]
    fn no_commas_flag() {
        assert_eq!(both(1234.567, 1, true), "1234.6");
        assert_eq!(both(1234.567, 2, true), "1234.57");
        assert_eq!(both(1_000_000.0, 0, true), "1000000");
        assert_eq!(both(1_000_000.0, 0, false), "1,000,000");
        assert_eq!(both(-1234.567, 1, false), "-1,234.6");
    }

    #[test]
    fn negative_decimals() {
        assert_eq!(both(1234.567, -2, false), "1,200");
        assert_eq!(both(99.9, -1, false), "100");
        assert_eq!(both(999.9, -1, false), "1,000");
        assert_eq!(both(21.5, -1, false), "20");
        assert_eq!(both(1.98, -1, false), "0");
    }

    #[test]
    fn half_away_and_pad() {
        assert_eq!(both(2.5, 0, false), "3");
        assert_eq!(both(-1.5, 0, false), "-2");
        assert_eq!(both(2.15, 1, false), "2.2");
        assert_eq!(both(1.1, 2, false), "1.10");
        assert_eq!(both(1.0, 4, true), "1.0000");
        assert_eq!(both(1.0, 6, false), "1.000000");
    }

    #[test]
    fn rounded_zero_drops_sign() {
        assert_eq!(both(-0.001, 2, false), "0.00");
        assert_eq!(both(-0.4, 0, false), "0");
        assert_eq!(both(-0.0, 2, false), "0.00");
        assert_eq!(both(0.0, 2, false), "0.00");
    }

    #[test]
    fn decimals_cap() {
        assert_eq!(fixed(1.0, 128, false), Err(ExcelError::Value));
        assert_eq!(fixed_naive(1.0, 128, true), Err(ExcelError::Value));
        let s = both(1.0, 20, true);
        assert_eq!(s, "1.00000000000000000000");
        assert_eq!(s.len(), 22);
    }

    #[test]
    fn grouping_and_millions() {
        assert_eq!(both(1_234_567.89, 2, false), "1,234,567.89");
        assert_eq!(both(12.0, 0, false), "12");
        assert_eq!(both(123.0, 0, false), "123");
        assert_eq!(both(1234.0, 0, false), "1,234");
    }

    #[test]
    fn apply_hot_path_and_coerce() {
        assert_eq!(
            fixed_apply(&ExcelValue::Number(1234.567), None, None),
            ExcelValue::Text("1,234.57".into())
        );
        assert_eq!(
            fixed_apply(&ExcelValue::Empty, None, None),
            ExcelValue::Text("0.00".into())
        );
        assert_eq!(
            fixed_apply(&ExcelValue::Bool(true), None, None),
            ExcelValue::Text("1.00".into())
        );
        assert_eq!(
            fixed_apply(
                &ExcelValue::Text("1234.5".into()),
                Some(&ExcelValue::Number(1.0)),
                None
            ),
            ExcelValue::Text("1,234.5".into())
        );
        assert_eq!(
            fixed_apply(&ExcelValue::Text("x".into()), None, None),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            fixed_apply(&ExcelValue::Text("".into()), None, None),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            fixed_apply(&ExcelValue::Text("$5".into()), None, None),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            fixed_apply(&ExcelValue::Text("1,000".into()), None, None),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            fixed_apply(
                &ExcelValue::Number(1.0),
                Some(&ExcelValue::Number(128.0)),
                None
            ),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            fixed_apply(
                &ExcelValue::Number(10.0),
                None,
                Some(&ExcelValue::Text("x".into()))
            ),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            fixed_apply(
                &ExcelValue::Number(1000.0),
                Some(&ExcelValue::Number(0.0)),
                Some(&ExcelValue::Bool(true))
            ),
            ExcelValue::Text("1000".into())
        );
        assert_eq!(
            fixed_apply(
                &ExcelValue::Error(ExcelError::Div0),
                Some(&ExcelValue::Error(ExcelError::Na)),
                None
            ),
            ExcelValue::Error(ExcelError::Div0)
        );
    }

    #[test]
    fn apply_naive_agrees() {
        let cases: [(&ExcelValue, Option<&ExcelValue>, Option<&ExcelValue>); 8] = [
            (&ExcelValue::Number(1234.567), None, None),
            (
                &ExcelValue::Number(-1234.567),
                Some(&ExcelValue::Number(-1.0)),
                Some(&ExcelValue::Bool(true)),
            ),
            (&ExcelValue::Empty, None, None),
            (
                &ExcelValue::Bool(false),
                Some(&ExcelValue::Number(0.0)),
                None,
            ),
            (
                &ExcelValue::Text("7".into()),
                None,
                Some(&ExcelValue::Number(1.0)),
            ),
            (&ExcelValue::Text("x".into()), None, None),
            (&ExcelValue::Error(ExcelError::Na), None, None),
            (
                &ExcelValue::Number(1.0),
                Some(&ExcelValue::Number(128.0)),
                None,
            ),
        ];
        for (n, d, nc) in cases {
            assert_eq!(fixed_apply(n, d, nc), fixed_apply_naive(n, d, nc), "{n:?}");
        }
    }

    #[test]
    fn naive_matches_fast_over_grid() {
        let digits = [-4, -3, -2, -1, 0, 1, 2, 3, 4, 6];
        for i in -80i32..=80 {
            let n = i as f64 * 12.37 + 0.15;
            for &d in &digits {
                both(n, d, false);
                both(n, d, true);
                both(-n, d, false);
            }
        }
    }

    #[test]
    fn packed_slice_matches() {
        let src: Vec<f64> = (0..64).map(|i| i as f64 * 111.1 - 200.0).collect();
        let mut fast = vec![String::new(); src.len()];
        let mut slow = vec![String::new(); src.len()];
        fixed_slice(&src, 2, false, &mut fast);
        fixed_slice_naive(&src, 2, false, &mut slow);
        assert_eq!(fast, slow);
        assert_eq!(fast[0], fixed(src[0], 2, false).unwrap());
    }

    #[test]
    fn formula_microsoft_and_omitted() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=FIXED(1234.567, 1)").unwrap(),
            ExcelValue::Text("1,234.6".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIXED(1234.567, -1)").unwrap(),
            ExcelValue::Text("1,230".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIXED(-1234.567, -1, TRUE)").unwrap(),
            ExcelValue::Text("-1230".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIXED(44.332)").unwrap(),
            ExcelValue::Text("44.33".into())
        );
        // Trailing-comma omitted decimals is 2, not 0.
        assert_eq!(
            eval_formula_in(&wb, "=FIXED(1234.567,)").unwrap(),
            ExcelValue::Text("1,234.57".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIXED(1234.567,,TRUE)").unwrap(),
            ExcelValue::Text("1234.57".into())
        );
    }

    #[test]
    fn formula_arity_errors_ltr_and_type() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=FIXED()").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIXED(1, 2, TRUE, 4)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIXED(#DIV/0!, #N/A)").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIXED(#N/A, 1/0)").unwrap(),
            ExcelValue::Error(ExcelError::Na)
        );
        assert_eq!(
            eval_formula_in(&wb, "=FIXED(1, 128)").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=TYPE(FIXED(1))").unwrap(),
            ExcelValue::Number(2.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=ISTEXT(FIXED(1))").unwrap(),
            ExcelValue::Bool(true)
        );
        assert_eq!(
            eval_formula_in(&wb, "=VALUE(FIXED(1234.5, 1))").unwrap(),
            ExcelValue::Number(1234.5)
        );
        assert_eq!(
            eval_formula_in(
                &wb,
                "=FIXED(1234.567, 1)=TEXT(ROUND(1234.567, 1), \"#,##0.0\")"
            )
            .unwrap(),
            ExcelValue::Bool(true)
        );
    }

    #[test]
    fn formula_blank_decimals_is_zero() {
        let mut sheet = Sheet::new("Sheet1");
        sheet
            .cells
            .insert("A1".into(), Cell::value(ExcelValue::Number(1234.567)));
        let wb = Workbook {
            sheets: vec![sheet],
            names: vec![],
        };
        // Blank B1 coerces to 0 decimals → "1,235"
        assert_eq!(
            eval_formula_in(&wb, "=FIXED(A1, B1)").unwrap(),
            ExcelValue::Text("1,235".into())
        );
    }
}
