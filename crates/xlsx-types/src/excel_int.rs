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
//! Production is one `floor` plus a cheap "within 15 digits of the next
//! integer" bump. The naive path always runs `excel_round_15` (`log10` /
//! `powi`) then `floor`, so benches can print before/after.

use crate::value::excel_round_15;

/// Production Excel `INT` kernel.
#[inline]
pub fn excel_int(n: f64) -> f64 {
    if !n.is_finite() {
        return n;
    }
    // Already an integer (including `|n| ≥ 2^52`, where every `f64` is
    // integral). Skip leftover math so a large exact integer cannot bump.
    if n == n.trunc() {
        return if n == 0.0 { 0.0 } else { n };
    }
    let f = n.floor();
    // Leftover just below the next integer (ten `+ 0.1`, `0.3-0.1-0.2`, …).
    // `gap < 0.5` keeps `2^51 + 0.5` from snapping; the relative bound is
    // the 15-digit leftover (same scale as classic `FLOOR(n, 1)`).
    let next = f + 1.0;
    let gap = next - n;
    if gap < 0.5 && gap <= 5e-15 * next.abs().max(1.0) {
        return if next == 0.0 { 0.0 } else { next };
    }
    if f == 0.0 {
        0.0
    } else {
        f
    }
}

/// First-draft kernel: always snap to Excel's 15-digit model then `floor`.
/// Same results on clean inputs; `log10` / `powi` on every call.
#[inline]
pub fn excel_int_naive(n: f64) -> f64 {
    if n == 0.0 {
        return 0.0;
    }
    if !n.is_finite() {
        return n;
    }
    excel_round_15(n).floor()
}

/// Apply [`excel_int`] to every `n[i]`. Hot path for column-shaped work.
pub fn excel_int_slice(n: &[f64], out: &mut [f64]) {
    let len = n.len().min(out.len());
    for i in 0..len {
        out[i] = excel_int(n[i]);
    }
}

/// Naive slice baseline matching [`excel_int_naive`].
pub fn excel_int_slice_naive(n: &[f64], out: &mut [f64]) {
    let len = n.len().min(out.len());
    for i in 0..len {
        out[i] = excel_int_naive(n[i]);
    }
}

/// IEEE `floor` (no leftover snap). Kept for leftover contrast tests.
#[inline]
pub fn excel_int_ieee(n: f64) -> f64 {
    if n == 0.0 {
        return 0.0;
    }
    n.floor()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::floor_ceiling::excel_floor;
    use crate::value::excel_num_eq;

    const SAFE_INT: f64 = (1i64 << 53) as f64;

    fn both(n: f64) -> f64 {
        let fast = excel_int(n);
        let slow = excel_int_naive(n);
        assert_eq!(
            fast, slow,
            "int mismatch vs 15-digit floor n={n}: fast={fast} naive={slow}"
        );
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
        for n in [-20.0, -1.0, 0.0, 1.0, 8.0, 40909.0] {
            assert_eq!(both(n), n);
        }
        // 2^53 is exact in f64; `excel_round_15` drifts, production must not.
        assert_eq!(excel_int(SAFE_INT), SAFE_INT);
        assert_eq!(excel_int(-0.0), 0.0);
        assert_eq!(excel_int_naive(-0.0), 0.0);
        assert_eq!(excel_int_ieee(-0.0), 0.0);
    }

    #[test]
    fn fifteen_digit_leftover_snaps() {
        // Repeated IEEE `+ 0.1` sits just below 1; `0.1 * 10` is exact.
        let tenths = (0..10).fold(0.0, |a, _| a + 0.1);
        assert!(
            tenths < 1.0,
            "IEEE 0.1×10 leftover should sit below 1: {tenths}"
        );
        assert_eq!(excel_int_ieee(tenths), 0.0);
        assert_eq!(excel_int(tenths), 1.0);
        assert_eq!(excel_int_naive(tenths), 1.0);

        // `0.3-0.1-0.2` is a tiny negative leftover; IEEE floor is −1.
        // The cheap probe matches FLOOR(n, 1); `excel_round_15` keeps the ulp.
        let sub = 0.3 - 0.1 - 0.2;
        assert!(sub < 0.0, "IEEE leftover should be negative: {sub}");
        assert_eq!(excel_int_ieee(sub), -1.0);
        assert_eq!(excel_int(sub), 0.0);
        assert_eq!(excel_floor(sub, 1.0).unwrap(), 0.0);

        let below_int = f64::from_bits(7.0f64.to_bits() - 1);
        assert!(below_int < 7.0);
        assert_eq!(excel_int_ieee(below_int), 6.0);
        assert_eq!(excel_int(below_int), 7.0);
        assert_eq!(excel_int_naive(below_int), 7.0);
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
        assert_eq!(excel_int(SAFE_INT), SAFE_INT);
        assert_eq!(excel_int(SAFE_INT + 2.0), SAFE_INT + 2.0);
        // Half-unit at 2^51 is a real fraction, not a leftover.
        let half = (1u64 << 51) as f64 + 0.5;
        assert_eq!(excel_int(half), (1u64 << 51) as f64);
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
