//! Excel `RANDARRAY([rows], [columns], [min], [max], [integer])`.
//!
//! Dynamic-array result is an [`ExcelValue::Array`] (including 1×1). This
//! engine does **not** write a spill range into the sheet, so occupied
//! neighbors never produce `#SPILL!` — evaluate returns the array that
//! *would* spill.
//!
//! # Excel-compatible surface
//!
//! | Arg | Omitted default | Notes |
//! |---|---|---|
//! | `rows` | 1 | Truncated toward 0; `< 1` (incl. blank cell → 0) is `#CALC!` |
//! | `columns` | 1 | Same as `rows` |
//! | `min` | 0 | Must be `<= max` |
//! | `max` | 1 | |
//! | `integer` | FALSE | TRUE requires integer `min` / `max` |
//!
//! Documented Excel quirks this module implements:
//!
//! - Omitted `rows`/`columns` default to 1 (a single value). A **blank cell**
//!   is 0, not “omitted”, so `RANDARRAY(A1)` of a blank `A1` is `#CALC!`.
//! - Decimal `rows` / `columns` are truncated toward zero (`2.9` → 2).
//! - `rows` or `columns` `< 1` → `#CALC!` (Excel cannot return an empty array).
//! - `min > max` → `#VALUE!`. `min == max` is allowed and is a constant fill.
//! - `integer=TRUE` with a non-integer `min` or `max` → `#VALUE!`.
//! - Integers are inclusive of both ends: `[min, max]`.
//! - Decimals use `min + u * (max - min)` with `u ∈ [0, 1)`, so the
//!   half-open interval `[min, max)` — the same convention as Excel `RAND()`
//!   for the default `[0, 1)` range. When `min == max` the value is exact.
//! - Too many arguments (`> 5`) → `#VALUE!`.
//! - Non-finite dimensions or an overflowing `rows * columns` → `#NUM!`.
//!
//! # Volatility and seeding (honest)
//!
//! `RANDARRAY` is volatile: each workbook recalc draws a new stream. This
//! engine models one [`crate::eval::Evaluator::eval_spec`] call as one
//! recalc. **Desktop Excel does not expose a seed**, and this crate does
//! **not** reproduce Microsoft's RNG (historically Wichmann–Hill, later an
//! undocumented generator). Sequences from this kernel will not match a
//! recorded Excel workbook.
//!
//! [`EvalOptions::rng_seed`](xlsx_types::EvalOptions::rng_seed) is a
//! **test / bench hook only**. It is not a sixth worksheet argument. When
//! `None` (the default), each evaluate call mixes wall-clock time with an
//! atomic counter so consecutive recalcs differ. When `Some(seed)`, the
//! xorshift64* stream is deterministic so unit tests can assert ranges,
//! integer-ness, and reproducibility without inventing Excel goldens.
//!
//! Fixtures that need a stable `expected` use `min = max` (justified: every
//! cell is that number), error cases, or aggregators (`SUM` / `COUNTA` /
//! `INDEX`). Unseeded `RANDARRAY()` stays in `fixtures/ignored`.
//!
//! [`fill`] pre-sizes the output. [`fill_naive`] is the same answers
//! without capacity hints — bench baseline only.

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use xlsx_types::{EvalError, EvalOptions, ExcelError, ExcelValue};

/// SplitMix64 / xorshift64* generator used only inside this crate.
///
/// Not Excel's RNG. See the module docs.
#[derive(Clone, Debug)]
pub struct XorShift64(u64);

impl XorShift64 {
    /// Never-zero xorshift state.
    pub fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    /// Seed from [`EvalOptions::rng_seed`], or fresh entropy when unset.
    pub fn from_eval_options(options: &EvalOptions) -> Self {
        match options.rng_seed {
            Some(seed) => Self::new(seed),
            None => Self::new(entropy_seed()),
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform `u` in `[0, 1)`.
    pub fn next_unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / ((1u64 << 53) as f64)
    }
}

fn entropy_seed() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0xA076_1D64_78BD_642F);
    let c = COUNTER.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
    let mut z = t ^ c.rotate_left(17);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

pub(crate) fn eval(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() > 5 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let mut vals: [Option<ExcelValue>; 5] = [None, None, None, None, None];
    for (i, arg) in args.iter().enumerate() {
        let v = ev.eval_scalar(arg, ctx)?;
        if let ExcelValue::Error(e) = v {
            return Ok(ExcelValue::Error(e));
        }
        vals[i] = Some(v);
    }
    Ok(apply(
        vals[0].as_ref(),
        vals[1].as_ref(),
        vals[2].as_ref(),
        vals[3].as_ref(),
        vals[4].as_ref(),
        &mut ctx.rng,
    ))
}

/// Excel `RANDARRAY` from already-evaluated optional arguments.
pub fn apply(
    rows: Option<&ExcelValue>,
    columns: Option<&ExcelValue>,
    min: Option<&ExcelValue>,
    max: Option<&ExcelValue>,
    integer: Option<&ExcelValue>,
    rng: &mut XorShift64,
) -> ExcelValue {
    apply_strategy(rows, columns, min, max, integer, rng, FillStrategy::Fast)
}

/// Same answers as [`apply`], without `Vec` capacity hints.
pub fn apply_naive(
    rows: Option<&ExcelValue>,
    columns: Option<&ExcelValue>,
    min: Option<&ExcelValue>,
    max: Option<&ExcelValue>,
    integer: Option<&ExcelValue>,
    rng: &mut XorShift64,
) -> ExcelValue {
    apply_strategy(rows, columns, min, max, integer, rng, FillStrategy::Naive)
}

#[derive(Clone, Copy)]
enum FillStrategy {
    Fast,
    Naive,
}

fn apply_strategy(
    rows: Option<&ExcelValue>,
    columns: Option<&ExcelValue>,
    min: Option<&ExcelValue>,
    max: Option<&ExcelValue>,
    integer: Option<&ExcelValue>,
    rng: &mut XorShift64,
    strategy: FillStrategy,
) -> ExcelValue {
    let rows = match dimension(rows, 1.0) {
        Ok(n) => n,
        Err(e) => return ExcelValue::Error(e),
    };
    let columns = match dimension(columns, 1.0) {
        Ok(n) => n,
        Err(e) => return ExcelValue::Error(e),
    };
    let min = match number_or(min, 0.0) {
        Ok(n) => n,
        Err(e) => return ExcelValue::Error(e),
    };
    let max = match number_or(max, 1.0) {
        Ok(n) => n,
        Err(e) => return ExcelValue::Error(e),
    };
    let integer = match logical_or(integer, false) {
        Ok(b) => b,
        Err(e) => return ExcelValue::Error(e),
    };
    match validate(rows, columns, min, max, integer) {
        Ok(()) => fill_strategy(rows, columns, min, max, integer, rng, strategy),
        Err(e) => ExcelValue::Error(e),
    }
}

fn validate(rows: u32, columns: u32, min: f64, max: f64, integer: bool) -> Result<(), ExcelError> {
    if !min.is_finite() || !max.is_finite() {
        return Err(ExcelError::Num);
    }
    if min > max {
        return Err(ExcelError::Value);
    }
    if integer && (!is_whole(min) || !is_whole(max)) {
        return Err(ExcelError::Value);
    }
    let _ = checked_len(rows, columns)?;
    Ok(())
}

fn is_whole(n: f64) -> bool {
    n == n.trunc()
}

fn checked_len(rows: u32, columns: u32) -> Result<usize, ExcelError> {
    let n = (rows as u64)
        .checked_mul(columns as u64)
        .ok_or(ExcelError::Num)?;
    usize::try_from(n).map_err(|_| ExcelError::Num)
}

fn dimension(v: Option<&ExcelValue>, default: f64) -> Result<u32, ExcelError> {
    let n = match v {
        None => default,
        Some(v) => coerce::to_number(v)?,
    };
    if !n.is_finite() {
        return Err(ExcelError::Num);
    }
    let t = n.trunc();
    if t < 1.0 {
        return Err(ExcelError::Calc);
    }
    if t > u32::MAX as f64 {
        return Err(ExcelError::Num);
    }
    Ok(t as u32)
}

fn number_or(v: Option<&ExcelValue>, default: f64) -> Result<f64, ExcelError> {
    match v {
        None => Ok(default),
        Some(v) => coerce::to_number(v),
    }
}

fn logical_or(v: Option<&ExcelValue>, default: bool) -> Result<bool, ExcelError> {
    match v {
        None => Ok(default),
        Some(v) => coerce::to_logical(v),
    }
}

/// Production fill: pre-size rows and columns.
pub fn fill(
    rows: u32,
    columns: u32,
    min: f64,
    max: f64,
    integer: bool,
    rng: &mut XorShift64,
) -> ExcelValue {
    fill_strategy(rows, columns, min, max, integer, rng, FillStrategy::Fast)
}

/// Allocation-churn baseline: `Vec::new()` + `push` per cell.
pub fn fill_naive(
    rows: u32,
    columns: u32,
    min: f64,
    max: f64,
    integer: bool,
    rng: &mut XorShift64,
) -> ExcelValue {
    fill_strategy(rows, columns, min, max, integer, rng, FillStrategy::Naive)
}

fn fill_strategy(
    rows: u32,
    columns: u32,
    min: f64,
    max: f64,
    integer: bool,
    rng: &mut XorShift64,
    strategy: FillStrategy,
) -> ExcelValue {
    let r = rows as usize;
    let c = columns as usize;
    let constant = min == max;
    let mut out = match strategy {
        FillStrategy::Fast => Vec::with_capacity(r),
        FillStrategy::Naive => Vec::new(),
    };
    for _ in 0..r {
        let mut row = match strategy {
            FillStrategy::Fast => Vec::with_capacity(c),
            FillStrategy::Naive => Vec::new(),
        };
        for _ in 0..c {
            let n = if constant {
                min
            } else {
                sample(min, max, integer, rng)
            };
            row.push(ExcelValue::Number(n));
        }
        out.push(row);
    }
    ExcelValue::Array(out)
}

fn sample(min: f64, max: f64, integer: bool, rng: &mut XorShift64) -> f64 {
    let u = rng.next_unit();
    if integer {
        let span = max - min + 1.0;
        let n = min + (u * span).floor();
        if n > max {
            max
        } else {
            n
        }
    } else {
        min + u * (max - min)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xlsx_types::{Candidate, EvalOptions, EvalSpec, Workbook};

    fn n(x: f64) -> ExcelValue {
        ExcelValue::Number(x)
    }

    fn seed(s: u64) -> XorShift64 {
        XorShift64::new(s)
    }

    fn grid(v: &ExcelValue) -> &Vec<Vec<ExcelValue>> {
        match v {
            ExcelValue::Array(rows) => rows,
            other => panic!("expected array, got {other:?}"),
        }
    }

    fn nums(v: &ExcelValue) -> Vec<f64> {
        grid(v)
            .iter()
            .flatten()
            .map(|c| match c {
                ExcelValue::Number(x) => *x,
                other => panic!("expected number, got {other:?}"),
            })
            .collect()
    }

    #[test]
    fn omitted_defaults_are_1x1_unit_interval() {
        let mut rng = seed(1);
        let v = apply(None, None, None, None, None, &mut rng);
        let rows = grid(&v);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].len(), 1);
        let x = nums(&v)[0];
        assert!((0.0..1.0).contains(&x), "{x}");
    }

    #[test]
    fn min_eq_max_is_constant_array() {
        let mut rng = seed(2);
        let v = apply(
            Some(&n(2.0)),
            Some(&n(3.0)),
            Some(&n(7.0)),
            Some(&n(7.0)),
            Some(&ExcelValue::Bool(true)),
            &mut rng,
        );
        assert_eq!(
            v,
            ExcelValue::Array(vec![
                vec![n(7.0), n(7.0), n(7.0)],
                vec![n(7.0), n(7.0), n(7.0)]
            ])
        );
    }

    #[test]
    fn decimal_min_eq_max() {
        let mut rng = seed(3);
        let v = apply(
            Some(&n(1.0)),
            Some(&n(2.0)),
            Some(&n(4.5)),
            Some(&n(4.5)),
            None,
            &mut rng,
        );
        assert_eq!(v, ExcelValue::Array(vec![vec![n(4.5), n(4.5)]]));
    }

    #[test]
    fn zero_or_negative_dim_is_calc() {
        let mut rng = seed(4);
        assert_eq!(
            apply(Some(&n(0.0)), None, None, None, None, &mut rng),
            ExcelValue::Error(ExcelError::Calc)
        );
        assert_eq!(
            apply(Some(&n(-1.0)), None, None, None, None, &mut rng),
            ExcelValue::Error(ExcelError::Calc)
        );
        assert_eq!(
            apply(Some(&n(1.0)), Some(&n(0.9)), None, None, None, &mut rng),
            ExcelValue::Error(ExcelError::Calc)
        );
        assert_eq!(
            apply(Some(&ExcelValue::Empty), None, None, None, None, &mut rng),
            ExcelValue::Error(ExcelError::Calc)
        );
    }

    #[test]
    fn fractional_dims_truncate_toward_zero() {
        let mut rng = seed(5);
        let v = apply(
            Some(&n(2.9)),
            Some(&n(1.1)),
            Some(&n(1.0)),
            Some(&n(1.0)),
            Some(&ExcelValue::Bool(true)),
            &mut rng,
        );
        assert_eq!(grid(&v).len(), 2);
        assert_eq!(grid(&v)[0].len(), 1);
    }

    #[test]
    fn min_gt_max_is_value() {
        let mut rng = seed(6);
        assert_eq!(
            apply(
                Some(&n(1.0)),
                Some(&n(1.0)),
                Some(&n(5.0)),
                Some(&n(1.0)),
                None,
                &mut rng
            ),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn integer_requires_whole_bounds() {
        let mut rng = seed(7);
        assert_eq!(
            apply(
                Some(&n(1.0)),
                Some(&n(1.0)),
                Some(&n(1.5)),
                Some(&n(4.0)),
                Some(&ExcelValue::Bool(true)),
                &mut rng
            ),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            apply(
                Some(&n(1.0)),
                Some(&n(1.0)),
                Some(&n(1.0)),
                Some(&n(4.5)),
                Some(&ExcelValue::Bool(true)),
                &mut rng
            ),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn integers_stay_in_inclusive_range() {
        let mut rng = seed(8);
        let v = fill(20, 10, -2.0, 3.0, true, &mut rng);
        for x in nums(&v) {
            assert!(x == x.trunc(), "{x} not integer");
            assert!((-2.0..=3.0).contains(&x), "{x}");
        }
    }

    #[test]
    fn decimals_stay_in_half_open_range() {
        let mut rng = seed(9);
        let v = fill(50, 20, 5.0, 10.0, false, &mut rng);
        for x in nums(&v) {
            assert!((5.0..10.0).contains(&x), "{x}");
        }
    }

    #[test]
    fn same_seed_same_stream() {
        let a = fill(4, 3, 0.0, 1.0, false, &mut seed(42));
        let b = fill(4, 3, 0.0, 1.0, false, &mut seed(42));
        assert_eq!(a, b);
        let c = fill(4, 3, 0.0, 1.0, false, &mut seed(43));
        assert_ne!(a, c);
    }

    #[test]
    fn fast_matches_naive() {
        let a = fill(5, 4, -3.0, 8.0, true, &mut seed(11));
        let b = fill_naive(5, 4, -3.0, 8.0, true, &mut seed(11));
        assert_eq!(a, b);
        let c = apply(
            Some(&n(3.0)),
            Some(&n(2.0)),
            None,
            None,
            None,
            &mut seed(12),
        );
        let d = apply_naive(
            Some(&n(3.0)),
            Some(&n(2.0)),
            None,
            None,
            None,
            &mut seed(12),
        );
        assert_eq!(c, d);
    }

    #[test]
    fn first_of_taller_array_matches_1x1_same_seed() {
        let one = fill(1, 1, 0.0, 1.0, false, &mut seed(99));
        let many = fill(4, 1, 0.0, 1.0, false, &mut seed(99));
        assert_eq!(nums(&one)[0], nums(&many)[0]);
    }

    #[test]
    fn text_number_and_logical_coerce() {
        let mut rng = seed(13);
        let v = apply(
            Some(&ExcelValue::Text("2".into())),
            Some(&ExcelValue::Bool(true)),
            Some(&n(9.0)),
            Some(&n(9.0)),
            Some(&n(1.0)),
            &mut rng,
        );
        assert_eq!(v, ExcelValue::Array(vec![vec![n(9.0)], vec![n(9.0)]]));
    }

    #[test]
    fn bad_types_are_value() {
        let mut rng = seed(14);
        assert_eq!(
            apply(
                Some(&ExcelValue::Text("x".into())),
                None,
                None,
                None,
                None,
                &mut rng
            ),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            apply(
                Some(&n(1.0)),
                None,
                None,
                None,
                Some(&ExcelValue::Text("TRUE".into())),
                &mut rng
            ),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn eval_options_seed_is_repeatable_through_evaluate() {
        let mut opts = EvalOptions::default();
        opts.rng_seed = Some(123);
        let spec = EvalSpec {
            case_id: "randarray.seed".into(),
            workbook: Workbook::default(),
            target: xlsx_types::EvalTarget::formula("=RANDARRAY(2,2)"),
            options: opts,
        };
        let a = crate::CalcCoreEngine::new().evaluate(&spec).unwrap();
        let b = crate::CalcCoreEngine::new().evaluate(&spec).unwrap();
        assert_eq!(a, b);
        let rows = grid(&a);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].len(), 2);
        for x in nums(&a) {
            assert!((0.0..1.0).contains(&x), "{x}");
        }
    }

    #[test]
    fn unseeded_evaluate_is_not_stuck_on_one_value() {
        let spec = EvalSpec::formula("randarray.volatile", "=RANDARRAY(8,8)");
        let a = nums(&crate::CalcCoreEngine::new().evaluate(&spec).unwrap());
        let b = nums(&crate::CalcCoreEngine::new().evaluate(&spec).unwrap());
        // Astronomically unlikely that 64 independent [0,1) draws match.
        assert_ne!(a, b);
    }
}
