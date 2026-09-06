//! Excel `ABS(number)` — absolute value of a real number.
//!
//! Desktop Excel / Microsoft ABS help (no golden-reading):
//! - `ABS(number)` returns `number` without its sign. Microsoft examples:
//!   `ABS(2)` → `2`, `ABS(-2)` → `2`.
//! - `number` uses arithmetic coerce: empty → `0`, `TRUE` → `1`, `FALSE` →
//!   `0`, numeric text (`"-7"`, `"  3.5  "`, `"1E3"`) → parsed, other text
//!   (`"x"`, `""`, `"$5"`, `"1,000"`, `"50%"`) → `#VALUE!`. That is **not**
//!   `VALUE` (currency / thousands / `%` text stay `#VALUE!`).
//! - Errors propagate. Wrong arity (`ABS()` / extra args) is `#VALUE!`.
//! - Scalar context: a range implicit-intersects the host; an array
//!   literal takes the top-left (no `ABS` spill). `SIGN` / `INT` / `SQRT`
//!   stay on `fn_unary_num`.
//!
//! Production path clears the IEEE sign bit (branchless; `-0.0` → `+0.0`).
//! The comparison-branch baseline lives beside it so benches can print
//! before/after. This kernel does **not** read fixture goldens.

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

/// Sign-bit mask: keep the exponent + mantissa, drop the sign.
const SIGN_CLEAR: u64 = 0x7fff_ffff_ffff_ffff;

/// Production `ABS` on an already-coerced number.
#[inline]
pub fn abs(n: f64) -> f64 {
    f64::from_bits(n.to_bits() & SIGN_CLEAR)
}

/// Comparison-branch baseline used for the hill-climb bench.
///
/// Same Excel result as [`abs`] for every finite nonzero value. `-0.0`
/// stays `-0.0` here (`n < 0.0` is false) — the production path
/// canonicalizes it to `+0.0`. Kept so
/// `cargo bench -p xlsx-engine-core --bench abs` can print before/after.
#[inline]
pub fn abs_naive(n: f64) -> f64 {
    if n < 0.0 {
        -n
    } else {
        n
    }
}

/// Production `ABS` on a scalar Excel value (Number hot path, no `Result`).
pub fn abs_value(v: &ExcelValue) -> ExcelValue {
    match v {
        ExcelValue::Number(n) => ExcelValue::Number(abs(*n)),
        ExcelValue::Empty => ExcelValue::Number(0.0),
        ExcelValue::Bool(true) => ExcelValue::Number(1.0),
        ExcelValue::Bool(false) => ExcelValue::Number(0.0),
        ExcelValue::Text(s) => match coerce::parse_numeric_text(s) {
            Ok(n) => ExcelValue::Number(abs(n)),
            Err(e) => ExcelValue::Error(e),
        },
        ExcelValue::Error(e) => ExcelValue::Error(*e),
        ExcelValue::Array(_) => ExcelValue::Error(ExcelError::Value),
    }
}

/// Value-level baseline: full [`coerce::to_number`] + [`abs_naive`].
pub fn abs_value_naive(v: &ExcelValue) -> ExcelValue {
    match coerce::to_number(v) {
        Ok(n) => ExcelValue::Number(abs_naive(n)),
        Err(e) => ExcelValue::Error(e),
    }
}

/// Branchless packed walk. Used by the kernel bench (and MAP-like callers).
pub fn abs_slice(src: &[f64], dst: &mut [f64]) {
    let n = src.len().min(dst.len());
    for i in 0..n {
        dst[i] = abs(src[i]);
    }
}

/// Branchy packed walk (bench baseline).
pub fn abs_slice_naive(src: &[f64], dst: &mut [f64]) {
    let n = src.len().min(dst.len());
    for i in 0..n {
        dst[i] = abs_naive(src[i]);
    }
}

/// `ABS(number)` — scalar context, wrong arity → `#VALUE!`.
pub(crate) fn fn_abs(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() != 1 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    Ok(abs_value(&ev.eval_scalar(&args[0], ctx)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn both_n(n: f64) -> (f64, f64) {
        (abs(n), abs_naive(n))
    }

    #[test]
    fn microsoft_examples() {
        assert_eq!(abs(2.0), 2.0);
        assert_eq!(abs(-2.0), 2.0);
        assert_eq!(abs(-4.0), 4.0);
        assert_eq!(abs(0.0), 0.0);
    }

    #[test]
    fn finite_matches_naive() {
        for n in [
            0.0,
            1.0,
            -1.0,
            1.5,
            -1.5,
            1e-20,
            -1e-20,
            1e20,
            -1e20,
            f64::MIN,
            f64::MAX,
            f64::MIN_POSITIVE,
            -f64::MIN_POSITIVE,
        ] {
            let (fast, slow) = both_n(n);
            assert_eq!(fast, slow.abs(), "abs({n})");
            if n != 0.0 {
                assert_eq!(fast, slow, "finite nonzero mismatch for {n}");
            }
        }
    }

    #[test]
    fn signed_zero_canonicalizes() {
        let n = -0.0_f64;
        assert!(n.to_bits() & (1 << 63) != 0, "precondition: input is -0");
        let fast = abs(n);
        assert_eq!(fast, 0.0);
        assert_eq!(fast.to_bits(), 0.0_f64.to_bits());
        // Naive comparison branch keeps the sign of zero.
        assert_eq!(abs_naive(n).to_bits(), n.to_bits());
    }

    #[test]
    fn infinities() {
        assert_eq!(abs(f64::INFINITY), f64::INFINITY);
        assert_eq!(abs(f64::NEG_INFINITY), f64::INFINITY);
        assert_eq!(abs_naive(f64::NEG_INFINITY), f64::INFINITY);
    }

    #[test]
    fn value_hot_path_and_coerce() {
        assert_eq!(
            abs_value(&ExcelValue::Number(-7.0)),
            ExcelValue::Number(7.0)
        );
        assert_eq!(abs_value(&ExcelValue::Empty), ExcelValue::Number(0.0));
        assert_eq!(abs_value(&ExcelValue::Bool(true)), ExcelValue::Number(1.0));
        assert_eq!(abs_value(&ExcelValue::Bool(false)), ExcelValue::Number(0.0));
        assert_eq!(
            abs_value(&ExcelValue::Text("-7".into())),
            ExcelValue::Number(7.0)
        );
        assert_eq!(
            abs_value(&ExcelValue::Text("  -3.5  ".into())),
            ExcelValue::Number(3.5)
        );
        assert_eq!(
            abs_value(&ExcelValue::Text("x".into())),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            abs_value(&ExcelValue::Text("".into())),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            abs_value(&ExcelValue::Text("$5".into())),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            abs_value(&ExcelValue::Text("1,000".into())),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            abs_value(&ExcelValue::Error(ExcelError::Div0)),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            abs_value(&ExcelValue::Array(vec![vec![ExcelValue::Number(-1.0)]])),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn value_naive_agrees_on_ordinary_inputs() {
        let cases = [
            ExcelValue::Number(-7.0),
            ExcelValue::Number(0.0),
            ExcelValue::Empty,
            ExcelValue::Bool(true),
            ExcelValue::Bool(false),
            ExcelValue::Text("-1E3".into()),
            ExcelValue::Text("x".into()),
            ExcelValue::Error(ExcelError::Na),
        ];
        for v in &cases {
            assert_eq!(abs_value(v), abs_value_naive(v), "{v:?}");
        }
    }

    #[test]
    fn packed_slice_matches() {
        let src: Vec<f64> = (0..64)
            .map(|i| if i % 2 == 0 { i as f64 } else { -(i as f64) })
            .collect();
        let mut fast = vec![0.0; src.len()];
        let mut slow = vec![0.0; src.len()];
        abs_slice(&src, &mut fast);
        abs_slice_naive(&src, &mut slow);
        assert_eq!(fast, slow);
        assert_eq!(fast[1], 1.0);
        assert_eq!(fast[2], 2.0);
    }
}
