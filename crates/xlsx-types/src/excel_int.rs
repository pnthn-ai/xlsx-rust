//! Excel `INT(number)` — floor toward −∞.
//!
//! Shared kernel so `calc-core` and `seed-compliant` stay aligned. Does not
//! read fixture goldens — callers pass a coerced `f64`.
//!
//! Desktop Excel (Microsoft):
//! - `INT` rounds toward −∞ (`INT(-8.9)` is `-9`). That is not `TRUNC`
//!   (toward zero: `TRUNC(-8.9)` is `-8`).
//! - `INT(n)` matches classic `FLOOR(n, 1)` for finite `n`.
//! - Excel's 15-significant-digit model treats leftovers such as ten
//!   `+ 0.1` addends (`0.999…9`) as `1`, so `INT` of that sum is `1`
//!   (IEEE `floor` is `0`). A tiny negative leftover (`0.3-0.1-0.2`)
//!   snaps to `0` rather than flooring to `-1`.
//!
//! Production specialises already-integers (identity, including `|n| > 2^53`
//! where every `f64` is integral) and uses a cheap near-integer test instead
//! of `excel_round_15` (`log10` / `powi`) on every call. The naive path is
//! IEEE `floor` so benches can print before/after.

use crate::value::excel_round_15;

/// Largest magnitude still exactly representable as an integer in `f64`.
const SAFE_INT: f64 = (1i64 << 53) as f64;

/// Production Excel `INT` kernel.
#[inline]
pub fn excel_int(n: f64) -> f64 {
    if n == 0.0 {
        return 0.0;
    }
    if !n.is_finite() {
        return n;
    }
    // Already an integer (safe `i64` range, or `|n| > 2^53` where every
    // representable `f64` is integral): skip `floor` and the leftover probe.
    if n == n.trunc() {
        return n;
    }
    // 15-digit leftover (`1/3+1/3+1/3` ≈ 0.999…9) is already the next integer.
    let r = n.round();
    if (n - r).abs() <= 5e-15 * r.abs().max(1.0) {
        return r;
    }
    n.floor()
}

/// IEEE `floor` baseline used by the hill-climb bench (no integer path, no
/// 15-digit leftover snap). Same sign / zero rules; leftovers may drift.
#[inline]
pub fn excel_int_naive(n: f64) -> f64 {
    if n == 0.0 {
        return 0.0;
    }
    n.floor()
}

/// Apply [`excel_int`] to every `n[i]`. Hot path for column-shaped work.
pub fn excel_int_slice(n: &[f64], out: &mut [f64]) {
    let len = n.len().min(out.len());
    for i in 0..len {
        out[i] = excel_int(n[i]);
    }
}

/// IEEE slice baseline matching [`excel_int_naive`].
pub fn excel_int_slice_naive(n: &[f64], out: &mut [f64]) {
    let len = n.len().min(out.len());
    for i in 0..len {
        out[i] = excel_int_naive(n[i]);
    }
}

/// `excel_round_15` then `floor` — correct 15-digit model, expensive. Used as
/// a cross-check that the cheap leftover probe matches the documented snap.
pub fn excel_int_round15(n: f64) -> f64 {
    if n == 0.0 {
        return 0.0;
    }
    if !n.is_finite() {
        return n;
    }
    excel_round_15(n).floor()
}

#[inline]
fn is_safe_int(x: f64) -> bool {
    x.is_finite() && x == x.trunc() && x.abs() <= SAFE_INT
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::floor_ceiling::excel_floor;
    use crate::value::excel_num_eq;

    fn both(n: f64) -> f64 {
        let fast = excel_int(n);
        // Naive matches on clean fractions / integers; leftovers may differ.
        if is_safe_int(n) || (n - n.round()).abs() > 1e-12 {
            assert_eq!(
                fast,
                excel_int_naive(n),
                "int mismatch vs IEEE floor n={n}: fast={fast}"
            );
        }
        fast
    }

    #[test]
    fn microsoft_int_examples() {
        assert_eq!(both(8.9), 8.0);
        assert_eq!(both(-8.9), -9.0);
        assert_eq!(both(1.9), 1.0);
        assert_eq!(both(-1.5), -2.0);
    }

    #[test]
    fn toward_neg_inf_not_trunc() {
        assert_eq!(both(-0.01), -1.0);
        assert_eq!(both(-0.0001), -1.0);
        assert_eq!(both(-0.5), -1.0);
        assert_eq!(both(0.5), 0.0);
        assert_eq!(both(0.99), 0.0);
        assert_eq!(both(-0.99), -1.0);
    }

    #[test]
    fn already_integer_is_identity() {
        for n in [-20.0, -1.0, 0.0, 1.0, 8.0, 40909.0, SAFE_INT] {
            assert_eq!(both(n), n);
        }
        assert_eq!(excel_int(-0.0), 0.0);
        assert_eq!(excel_int_naive(-0.0), 0.0);
    }

    #[test]
    fn fifteen_digit_leftover_snaps() {
        // Repeated IEEE `+ 0.1` sits just below 1; `0.1 * 10` is exact.
        let tenths = (0..10).fold(0.0, |a, _| a + 0.1);
        assert!(
            tenths < 1.0,
            "IEEE 0.1×10 leftover should sit below 1: {tenths}"
        );
        assert_eq!(excel_int_naive(tenths), 0.0);
        assert_eq!(excel_int(tenths), 1.0);
        assert_eq!(excel_int_round15(tenths), 1.0);

        // `0.3-0.1-0.2` is a tiny negative leftover; IEEE floor is −1.
        let sub = 0.3 - 0.1 - 0.2;
        assert!(sub < 0.0, "IEEE leftover should be negative: {sub}");
        assert_eq!(excel_int_naive(sub), -1.0);
        assert_eq!(excel_int(sub), 0.0);

        let below_int = f64::from_bits(7.0f64.to_bits() - 1);
        assert!(below_int < 7.0);
        assert_eq!(excel_int_naive(below_int), 6.0);
        assert_eq!(excel_int(below_int), 7.0);
    }

    #[test]
    fn matches_classic_floor_significance_one() {
        let samples = [
            -20.0,
            -8.9,
            -1.5,
            -0.01,
            0.0,
            0.5,
            1.9,
            8.9,
            40909.75,
            1.0 / 3.0,
            (0..10).fold(0.0, |a, _| a + 0.1),
            0.3 - 0.1 - 0.2,
        ];
        for n in samples {
            let a = excel_int(n);
            let b = excel_floor(n, 1.0).expect("FLOOR(n,1)");
            assert!(excel_num_eq(a, b), "INT({n})={a} vs FLOOR({n},1)={b}");
        }
    }

    #[test]
    fn slice_matches_scalar() {
        let ns: Vec<f64> = (0..64).map(|i| i as f64 * 1.7 - 20.0).collect();
        let mut out = vec![0.0; ns.len()];
        excel_int_slice(&ns, &mut out);
        for (n, got) in ns.iter().zip(out.iter()) {
            assert_eq!(*got, excel_int(*n));
        }
        let mut naive = vec![0.0; ns.len()];
        excel_int_slice_naive(&ns, &mut naive);
        for (n, got) in ns.iter().zip(naive.iter()) {
            assert_eq!(*got, excel_int_naive(*n));
        }
    }

    #[test]
    fn large_magnitude_integers() {
        let big = 1e20;
        assert_eq!(excel_int(big), big);
        assert_eq!(excel_int(-big), -big);
        assert_eq!(excel_int(SAFE_INT + 2.0), SAFE_INT + 2.0);
    }

    #[test]
    fn nonfinite_passthrough() {
        assert!(excel_int(f64::INFINITY).is_infinite());
        assert!(excel_int(f64::NEG_INFINITY).is_infinite());
        assert!(excel_int(f64::NAN).is_nan());
    }

    #[test]
    fn naive_matches_fast_over_clean_grid() {
        for i in -200i32..=200 {
            let n = i as f64 * 0.137 + 0.15;
            both(n);
            both(-n);
        }
    }
}
