//! Excel `FLOOR.MATH` / `CEILING.MATH`, plus re-exports of classic `FLOOR` /
//! `CEILING` from their dedicated kernels.
//!
//! Shared kernel so `calc-core` and `seed-compliant` stay aligned. Does not
//! read fixture goldens — callers pass coerced `f64`s.

use crate::error::ExcelError;
use crate::value::excel_round_15;

/// Largest magnitude still exactly representable as an integer in `f64`.
const SAFE_INT: f64 = (1i64 << 53) as f64;

/// Classic Excel `FLOOR` — dedicated kernel in [`crate::excel_floor`].
pub use crate::excel_floor::{
    excel_floor, excel_floor_naive, excel_floor_slice, excel_floor_slice_naive,
};

/// Classic Excel `CEILING` — dedicated kernel in [`crate::excel_ceiling`].
pub use crate::excel_ceiling::{
    excel_ceiling, excel_ceiling_naive, excel_ceiling_slice, excel_ceiling_slice_naive,
};

/// `FLOOR.MATH(number, significance, mode)`.
///
/// Significance sign is ignored. Omitted significance is `1`, omitted mode is
/// `0`. Significance `0` returns `0`. Mode `0` rounds toward −∞; nonzero mode
/// rounds toward zero for negatives.
pub fn excel_floor_math(n: f64, s: f64, mode: f64) -> Result<f64, ExcelError> {
    math_round(n, s, mode, MathKind::Floor)
}

/// `CEILING.MATH(number, significance, mode)`.
///
/// Significance sign is ignored. Significance `0` returns `0`. Mode `0` rounds
/// toward +∞; nonzero mode rounds away from zero for negatives.
pub fn excel_ceiling_math(n: f64, s: f64, mode: f64) -> Result<f64, ExcelError> {
    math_round(n, s, mode, MathKind::Ceiling)
}

#[derive(Clone, Copy)]
enum Dir {
    Floor,
    Ceiling,
}

#[derive(Clone, Copy)]
enum MathKind {
    Floor,
    Ceiling,
}

fn try_int_path(n: f64, s: f64, dir: Dir) -> Option<f64> {
    // Probe significance first — decimal `0.1` / `0.01` exit before touching `n`.
    if !is_safe_int(s) || !is_safe_int(n) {
        return None;
    }
    let ni = n as i64;
    let si = s as i64;
    let q = match dir {
        Dir::Floor => i64_div_floor(ni, si)?,
        Dir::Ceiling => i64_div_ceil(ni, si)?,
    };
    Some(q.checked_mul(si)? as f64)
}

fn is_safe_int(x: f64) -> bool {
    x.is_finite() && x == x.trunc() && x.abs() <= SAFE_INT
}

/// Rust `/` truncates toward zero; Excel `FLOOR` uses toward −∞.
fn i64_div_floor(n: i64, s: i64) -> Option<i64> {
    if s == 0 {
        return None;
    }
    let q = n / s;
    let r = n % s;
    if r != 0 && (n < 0) != (s < 0) {
        q.checked_sub(1)
    } else {
        Some(q)
    }
}

/// Rust `/` truncates toward zero; Excel `CEILING` uses toward +∞ of `n/s`.
fn i64_div_ceil(n: i64, s: i64) -> Option<i64> {
    if s == 0 {
        return None;
    }
    let q = n / s;
    let r = n % s;
    if r != 0 && (n < 0) == (s < 0) {
        q.checked_add(1)
    } else {
        Some(q)
    }
}

fn round_multiple(n: f64, s: f64, dir: Dir) -> f64 {
    let q = n / s;
    // Cheap 15-digit "already a multiple" test — avoids `excel_num_eq`'s
    // two `log10` snaps on the hot decimal path (`CEILING(1.2, 0.1)`).
    if nearly_int(q) {
        return snap(s * q.round());
    }
    match dir {
        Dir::Floor => s * q.floor(),
        Dir::Ceiling => s * q.ceil(),
    }
}

fn nearly_int(q: f64) -> bool {
    let r = q.round();
    (q - r).abs() <= 5e-15 * r.abs().max(1.0)
}

fn snap(x: f64) -> f64 {
    excel_round_15(x)
}

fn math_round(n: f64, s: f64, mode: f64, kind: MathKind) -> Result<f64, ExcelError> {
    if !n.is_finite() || !s.is_finite() || !mode.is_finite() {
        return Err(ExcelError::Num);
    }
    if s == 0.0 || n == 0.0 {
        return Ok(0.0);
    }
    let sig = s.abs();
    let toward_zero = mode != 0.0;
    let signed = match (kind, toward_zero, n < 0.0) {
        // FLOOR.MATH mode 0: toward −∞. Nonzero mode + negative: toward 0.
        (MathKind::Floor, false, _) => Dir::Floor,
        (MathKind::Floor, true, true) => Dir::Ceiling,
        (MathKind::Floor, true, false) => Dir::Floor,
        // CEILING.MATH mode 0: toward +∞. Nonzero mode + negative: away from 0.
        (MathKind::Ceiling, false, _) => Dir::Ceiling,
        (MathKind::Ceiling, true, true) => Dir::Floor,
        (MathKind::Ceiling, true, false) => Dir::Ceiling,
    };
    if let Some(v) = try_int_path(n, sig, signed) {
        return Ok(v);
    }
    Ok(round_multiple(n, sig, signed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::excel_num_eq;

    fn n(v: Result<f64, ExcelError>) -> f64 {
        v.expect("number")
    }

    #[test]
    fn microsoft_floor_examples() {
        assert_eq!(n(excel_floor(3.7, 2.0)), 2.0);
        assert_eq!(n(excel_floor(-2.5, -2.0)), -2.0);
        assert_eq!(excel_floor(2.5, -2.0), Err(ExcelError::Num));
        assert!(excel_num_eq(n(excel_floor(1.58, 0.1)), 1.5));
        assert!(excel_num_eq(n(excel_floor(0.234, 0.01)), 0.23));
    }

    #[test]
    fn microsoft_ceiling_examples() {
        assert_eq!(n(excel_ceiling(2.5, 1.0)), 3.0);
        assert_eq!(n(excel_ceiling(-2.5, -2.0)), -4.0);
        assert_eq!(n(excel_ceiling(-2.5, 2.0)), -2.0);
        assert!(excel_num_eq(n(excel_ceiling(1.5, 0.1)), 1.5));
        assert!(excel_num_eq(n(excel_ceiling(0.234, 0.01)), 0.24));
    }

    #[test]
    fn zero_significance() {
        assert_eq!(excel_floor(15.0, 0.0), Err(ExcelError::Div0));
        assert_eq!(excel_ceiling(15.0, 0.0), Err(ExcelError::Div0));
        assert_eq!(n(excel_floor(0.0, 0.0)), 0.0);
        assert_eq!(n(excel_ceiling(0.0, 0.0)), 0.0);
        assert_eq!(n(excel_floor_math(15.0, 0.0, 0.0)), 0.0);
        assert_eq!(n(excel_ceiling_math(15.0, 0.0, 0.0)), 0.0);
    }

    #[test]
    fn mixed_sign_floor_toward_neg_inf() {
        assert_eq!(n(excel_floor(-1.5, 1.0)), -2.0);
        assert_eq!(n(excel_floor(-2.5, 2.0)), -4.0);
        assert_eq!(n(excel_floor(0.0, -1.0)), 0.0);
    }

    #[test]
    fn already_multiple_decimal() {
        assert!(excel_num_eq(n(excel_ceiling(1.2, 0.1)), 1.2));
        assert!(excel_num_eq(n(excel_floor(1.2, 0.1)), 1.2));
        assert_eq!(n(excel_ceiling(6.0, 3.0)), 6.0);
        assert_eq!(n(excel_floor(-6.0, 3.0)), -6.0);
    }

    #[test]
    fn floor_math_mode() {
        assert_eq!(n(excel_floor_math(2.5, 1.0, 0.0)), 2.0);
        assert_eq!(n(excel_floor_math(-2.5, 1.0, 0.0)), -3.0);
        assert_eq!(n(excel_floor_math(-2.5, 2.0, 0.0)), -4.0);
        assert_eq!(n(excel_floor_math(-2.5, 2.0, 1.0)), -2.0);
        assert_eq!(n(excel_floor_math(-2.5, -2.0, 0.0)), -4.0);
    }

    #[test]
    fn ceiling_math_mode() {
        assert_eq!(n(excel_ceiling_math(2.5, 1.0, 0.0)), 3.0);
        assert_eq!(n(excel_ceiling_math(-2.5, 1.0, 0.0)), -2.0);
        assert_eq!(n(excel_ceiling_math(-2.5, 2.0, 0.0)), -2.0);
        assert_eq!(n(excel_ceiling_math(-2.5, 2.0, -1.0)), -4.0);
        assert_eq!(n(excel_ceiling_math(-2.5, -2.0, 0.0)), -2.0);
    }

    #[test]
    fn integer_path_matches_ieee_on_ints() {
        for n in [-20i64, -7, -5, -4, -1, 0, 1, 4, 5, 7, 20] {
            for s in [-7i64, -3, -2, -1, 1, 2, 3, 7] {
                let nf = n as f64;
                let sf = s as f64;
                let a = excel_floor(nf, sf);
                let b = excel_floor_naive(nf, sf);
                assert_eq!(a, b, "FLOOR({n},{s})");
                let a = excel_ceiling(nf, sf);
                let b = excel_ceiling_naive(nf, sf);
                assert_eq!(a, b, "CEILING({n},{s})");
            }
        }
    }

    #[test]
    fn slice_matches_scalar() {
        let ns: Vec<f64> = (0..64).map(|i| i as f64 * 1.7 - 20.0).collect();
        let mut out = vec![0.0; ns.len()];
        let errs = excel_floor_slice(&ns, 3.0, &mut out);
        assert_eq!(errs, 0);
        for (n, got) in ns.iter().zip(out.iter()) {
            assert_eq!(*got, excel_floor(*n, 3.0).unwrap());
        }
    }
}
