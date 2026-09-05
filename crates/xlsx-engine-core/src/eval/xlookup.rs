//! Excel `XLOOKUP(lookup_value, lookup_array, return_array, [if_not_found], [match_mode], [search_mode])`.
//!
//! Documented Excel quirks this module implements:
//!
//! - Default `match_mode` is **0** (exact). Unlike `VLOOKUP`, omitted mode is
//!   not approximate, and `*` / `?` / `~` are **literal** unless `match_mode`
//!   is 2.
//! - Exact match is type-strict (`1` ≠ `"1"` ≠ `TRUE`) and case-insensitive
//!   for text. Blank ≠ `0` ≠ `""`. 15-digit numeric equality applies.
//! - `if_not_found` is used only on a miss. Omitted → `#N/A`. A provided blank
//!   or `""` is returned as-is. An unused `1/0` does not replace a hit.
//! - `match_mode`: `0` exact, `-1` exact or next smaller, `1` exact or next
//!   larger, `2` wildcard (`*` / `?` / `~`). Other integers → `#VALUE!`.
//!   Non-integers truncate toward zero (`-1.9` → `-1`).
//! - `search_mode`: `1` first-to-last (default), `-1` last-to-first, `2`
//!   binary search on an ascending list, `-2` binary search on a descending
//!   list. Other integers → `#VALUE!`.
//! - Wildcard (`match_mode` 2) + binary search (`search_mode` ±2) is `#VALUE!`.
//! - Wildcard mode matches **text** keys only (a number `123` is not `"123*"`).
//! - `lookup_array` must be a vector. `return_array` must share that length
//!   on the search axis (extra return columns/rows become a result array).
//! - Binary search on unsorted data can miss a present key or return the
//!   wrong approximate row (same family of quirk as approximate `VLOOKUP`).
//!
//! [`xlookup`] walks the lookup axis in place and clones only the matched
//! return slice (or binary-searches when `search_mode` is ±2).
//! [`xlookup_naive`] flattens both arrays first and always scans linearly —
//! same answers, more allocation / more compares. Used as the bench "before".

use super::{coerce, compare, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{excel_num_eq, excel_wildcard, EvalError, ExcelError, ExcelValue};

const EMPTY: ExcelValue = ExcelValue::Empty;

/// Excel `XLOOKUP` from already-evaluated arguments.
pub fn xlookup(
    lookup: &ExcelValue,
    lookup_array: &ExcelValue,
    return_array: &ExcelValue,
    if_not_found: Option<&ExcelValue>,
    match_mode: Option<&ExcelValue>,
    search_mode: Option<&ExcelValue>,
) -> ExcelValue {
    xlookup_apply(
        lookup,
        lookup_array,
        return_array,
        if_not_found,
        match_mode,
        search_mode,
        Strategy::Fast,
    )
}

/// Flatten-both + always-linear baseline. Same answers as [`xlookup`].
pub fn xlookup_naive(
    lookup: &ExcelValue,
    lookup_array: &ExcelValue,
    return_array: &ExcelValue,
    if_not_found: Option<&ExcelValue>,
    match_mode: Option<&ExcelValue>,
    search_mode: Option<&ExcelValue>,
) -> ExcelValue {
    xlookup_apply(
        lookup,
        lookup_array,
        return_array,
        if_not_found,
        match_mode,
        search_mode,
        Strategy::Naive,
    )
}

#[derive(Clone, Copy)]
enum Strategy {
    Fast,
    Naive,
}

#[derive(Clone, Copy)]
enum MatchMode {
    Exact,
    NextSmaller,
    NextLarger,
    Wildcard,
}

#[derive(Clone, Copy)]
enum SearchMode {
    FirstToLast,
    LastToFirst,
    BinaryAsc,
    BinaryDesc,
}

enum Matrix<'a> {
    Array(&'a [Vec<ExcelValue>]),
    Scalar(&'a ExcelValue),
}

impl<'a> Matrix<'a> {
    fn from_value(v: &'a ExcelValue) -> Result<Self, ExcelError> {
        match v {
            ExcelValue::Error(e) => Err(*e),
            ExcelValue::Array(rows) => Ok(Self::Array(rows)),
            other => Ok(Self::Scalar(other)),
        }
    }

    fn dims(&self) -> (usize, usize) {
        match self {
            Self::Array(rows) => {
                let r = rows.len();
                let c = rows.first().map(|row| row.len()).unwrap_or(0);
                (r, c)
            }
            Self::Scalar(_) => (1, 1),
        }
    }

    fn get(&self, r: usize, c: usize) -> &ExcelValue {
        match self {
            Self::Array(rows) => rows.get(r).and_then(|row| row.get(c)).unwrap_or(&EMPTY),
            Self::Scalar(v) if r == 0 && c == 0 => v,
            Self::Scalar(_) => &EMPTY,
        }
    }
}

#[derive(Clone, Copy)]
enum Axis {
    /// Search down a column (`n×1` lookup).
    Vertical,
    /// Search across a row (`1×n` lookup).
    Horizontal,
}

pub(crate) fn eval(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() < 3 || args.len() > 6 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let lookup = ev.eval_scalar(&args[0], ctx)?;
    let lookup_array = ev.eval_expr(&args[1], ctx)?;
    let return_array = ev.eval_expr(&args[2], ctx)?;
    let if_not_found = if args.len() >= 4 {
        Some(ev.eval_expr(&args[3], ctx)?)
    } else {
        None
    };
    let match_mode = if args.len() >= 5 {
        Some(ev.eval_scalar(&args[4], ctx)?)
    } else {
        None
    };
    let search_mode = if args.len() >= 6 {
        Some(ev.eval_scalar(&args[5], ctx)?)
    } else {
        None
    };
    Ok(xlookup(
        &lookup,
        &lookup_array,
        &return_array,
        if_not_found.as_ref(),
        match_mode.as_ref(),
        search_mode.as_ref(),
    ))
}

fn xlookup_apply(
    lookup: &ExcelValue,
    lookup_array: &ExcelValue,
    return_array: &ExcelValue,
    if_not_found: Option<&ExcelValue>,
    match_mode: Option<&ExcelValue>,
    search_mode: Option<&ExcelValue>,
    strategy: Strategy,
) -> ExcelValue {
    if let ExcelValue::Error(e) = lookup {
        return ExcelValue::Error(*e);
    }
    let lookup_m = match Matrix::from_value(lookup_array) {
        Ok(m) => m,
        Err(e) => return ExcelValue::Error(e),
    };
    let return_m = match Matrix::from_value(return_array) {
        Ok(m) => m,
        Err(e) => return ExcelValue::Error(e),
    };
    let match_mode = match parse_match_mode(match_mode) {
        Ok(m) => m,
        Err(e) => return ExcelValue::Error(e),
    };
    let search_mode = match parse_search_mode(search_mode) {
        Ok(m) => m,
        Err(e) => return ExcelValue::Error(e),
    };
    if matches!(match_mode, MatchMode::Wildcard)
        && matches!(search_mode, SearchMode::BinaryAsc | SearchMode::BinaryDesc)
    {
        return ExcelValue::Error(ExcelError::Value);
    }

    let (lh, lw) = lookup_m.dims();
    let (rh, rw) = return_m.dims();
    if lh == 0 || lw == 0 || rh == 0 || rw == 0 {
        return miss(if_not_found);
    }
    let axis = if lw == 1 && rh == lh {
        Axis::Vertical
    } else if lh == 1 && rw == lw {
        Axis::Horizontal
    } else {
        return ExcelValue::Error(ExcelError::Value);
    };
    let n = match axis {
        Axis::Vertical => lh,
        Axis::Horizontal => lw,
    };

    let found = match strategy {
        Strategy::Fast => find_index(lookup, &lookup_m, axis, n, match_mode, search_mode),
        Strategy::Naive => find_index_naive(lookup, &lookup_m, axis, n, match_mode, search_mode),
    };
    match found {
        Some(i) => take_return(&return_m, axis, i),
        None => miss(if_not_found),
    }
}

fn miss(if_not_found: Option<&ExcelValue>) -> ExcelValue {
    match if_not_found {
        Some(v) => v.clone(),
        None => ExcelValue::Error(ExcelError::Na),
    }
}

fn parse_match_mode(v: Option<&ExcelValue>) -> Result<MatchMode, ExcelError> {
    let n = match v {
        None => 0.0,
        Some(v) => coerce::to_number(v)?,
    };
    match trunc_mode(n)? {
        0 => Ok(MatchMode::Exact),
        -1 => Ok(MatchMode::NextSmaller),
        1 => Ok(MatchMode::NextLarger),
        2 => Ok(MatchMode::Wildcard),
        _ => Err(ExcelError::Value),
    }
}

fn parse_search_mode(v: Option<&ExcelValue>) -> Result<SearchMode, ExcelError> {
    let n = match v {
        None => 1.0,
        Some(v) => coerce::to_number(v)?,
    };
    match trunc_mode(n)? {
        1 => Ok(SearchMode::FirstToLast),
        -1 => Ok(SearchMode::LastToFirst),
        2 => Ok(SearchMode::BinaryAsc),
        -2 => Ok(SearchMode::BinaryDesc),
        _ => Err(ExcelError::Value),
    }
}

fn trunc_mode(n: f64) -> Result<i32, ExcelError> {
    if !n.is_finite() || n.abs() > 1_000_000.0 {
        return Err(ExcelError::Value);
    }
    Ok(n.trunc() as i32)
}

fn lookup_at<'a>(m: &'a Matrix<'a>, axis: Axis, i: usize) -> &'a ExcelValue {
    match axis {
        Axis::Vertical => m.get(i, 0),
        Axis::Horizontal => m.get(0, i),
    }
}

fn take_return(m: &Matrix<'_>, axis: Axis, i: usize) -> ExcelValue {
    match axis {
        Axis::Vertical => {
            let w = m.dims().1;
            if w == 1 {
                m.get(i, 0).clone()
            } else {
                ExcelValue::Array(vec![(0..w).map(|c| m.get(i, c).clone()).collect()])
            }
        }
        Axis::Horizontal => {
            let h = m.dims().0;
            if h == 1 {
                m.get(0, i).clone()
            } else {
                ExcelValue::Array((0..h).map(|r| vec![m.get(r, i).clone()]).collect())
            }
        }
    }
}

fn find_index(
    lookup: &ExcelValue,
    keys: &Matrix<'_>,
    axis: Axis,
    n: usize,
    match_mode: MatchMode,
    search_mode: SearchMode,
) -> Option<usize> {
    match search_mode {
        SearchMode::BinaryAsc => binary_asc(lookup, keys, axis, n, match_mode),
        SearchMode::BinaryDesc => binary_desc(lookup, keys, axis, n, match_mode),
        SearchMode::FirstToLast => scan(lookup, keys, axis, n, match_mode, true),
        SearchMode::LastToFirst => scan(lookup, keys, axis, n, match_mode, false),
    }
}

fn find_index_naive(
    lookup: &ExcelValue,
    keys: &Matrix<'_>,
    axis: Axis,
    n: usize,
    match_mode: MatchMode,
    search_mode: SearchMode,
) -> Option<usize> {
    // Clone every lookup key first. Binary modes still go through the same
    // probe as the fast path so unsorted-binary goldens stay aligned; the
    // extra allocation is what the bench measures.
    let flat: Vec<ExcelValue> = (0..n).map(|i| lookup_at(keys, axis, i).clone()).collect();
    find_index_on_flat(lookup, &flat, match_mode, search_mode)
}

fn find_index_on_flat(
    lookup: &ExcelValue,
    flat: &[ExcelValue],
    match_mode: MatchMode,
    search_mode: SearchMode,
) -> Option<usize> {
    let col: Vec<Vec<ExcelValue>> = flat.iter().cloned().map(|v| vec![v]).collect();
    let m = Matrix::Array(&col);
    find_index(
        lookup,
        &m,
        Axis::Vertical,
        flat.len(),
        match_mode,
        search_mode,
    )
}

fn scan(
    lookup: &ExcelValue,
    keys: &Matrix<'_>,
    axis: Axis,
    n: usize,
    match_mode: MatchMode,
    first_to_last: bool,
) -> Option<usize> {
    let iter: Box<dyn Iterator<Item = usize>> = if first_to_last {
        Box::new(0..n)
    } else {
        Box::new((0..n).rev())
    };
    let mut best: Option<usize> = None;
    for i in iter {
        let key = lookup_at(keys, axis, i);
        if key_matches(lookup, key, match_mode) {
            return Some(i);
        }
        if matches!(match_mode, MatchMode::NextSmaller | MatchMode::NextLarger)
            && approx_candidate(lookup, key, match_mode)
            && better_approx(
                lookup,
                key,
                best.map(|b| lookup_at(keys, axis, b)),
                match_mode,
            )
        {
            best = Some(i);
        }
    }
    best
}

fn binary_asc(
    lookup: &ExcelValue,
    keys: &Matrix<'_>,
    axis: Axis,
    n: usize,
    match_mode: MatchMode,
) -> Option<usize> {
    match match_mode {
        MatchMode::Exact | MatchMode::Wildcard => {
            let i = bisect_left(keys, axis, n, lookup);
            if i < n && key_matches(lookup, lookup_at(keys, axis, i), MatchMode::Exact) {
                Some(i)
            } else {
                None
            }
        }
        MatchMode::NextSmaller => {
            let i = bisect_right_leq(keys, axis, n, lookup)?;
            let key = lookup_at(keys, axis, i);
            if leq(key, lookup) {
                Some(i)
            } else {
                None
            }
        }
        MatchMode::NextLarger => {
            let i = bisect_left(keys, axis, n, lookup);
            if i < n && geq(lookup_at(keys, axis, i), lookup) {
                Some(i)
            } else {
                None
            }
        }
    }
}

fn binary_desc(
    lookup: &ExcelValue,
    keys: &Matrix<'_>,
    axis: Axis,
    n: usize,
    match_mode: MatchMode,
) -> Option<usize> {
    match match_mode {
        MatchMode::Exact | MatchMode::Wildcard => {
            let i = bisect_left_desc(keys, axis, n, lookup);
            if i < n && key_matches(lookup, lookup_at(keys, axis, i), MatchMode::Exact) {
                Some(i)
            } else {
                None
            }
        }
        MatchMode::NextSmaller => {
            let i = bisect_left_desc_leq(keys, axis, n, lookup);
            if i < n && leq(lookup_at(keys, axis, i), lookup) {
                Some(i)
            } else {
                None
            }
        }
        MatchMode::NextLarger => {
            let i = bisect_right_desc_geq(keys, axis, n, lookup)?;
            if geq(lookup_at(keys, axis, i), lookup) {
                Some(i)
            } else {
                None
            }
        }
    }
}

/// First index where `key >= lookup` on an ascending list (standard lower-bound).
fn bisect_left(keys: &Matrix<'_>, axis: Axis, n: usize, lookup: &ExcelValue) -> usize {
    let mut lo = 0usize;
    let mut hi = n;
    while lo < hi {
        let mid = (lo + hi) / 2;
        if lt(lookup_at(keys, axis, mid), lookup) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

/// Last index where `key <= lookup` on an ascending list.
fn bisect_right_leq(keys: &Matrix<'_>, axis: Axis, n: usize, lookup: &ExcelValue) -> Option<usize> {
    let mut lo = 0usize;
    let mut hi = n;
    while lo < hi {
        let mid = (lo + hi) / 2;
        if leq(lookup_at(keys, axis, mid), lookup) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo == 0 {
        None
    } else {
        Some(lo - 1)
    }
}

/// First index where `key <= lookup` on a descending list.
fn bisect_left_desc(keys: &Matrix<'_>, axis: Axis, n: usize, lookup: &ExcelValue) -> usize {
    let mut lo = 0usize;
    let mut hi = n;
    while lo < hi {
        let mid = (lo + hi) / 2;
        if gt(lookup_at(keys, axis, mid), lookup) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

fn bisect_left_desc_leq(keys: &Matrix<'_>, axis: Axis, n: usize, lookup: &ExcelValue) -> usize {
    bisect_left_desc(keys, axis, n, lookup)
}

/// Last index where `key >= lookup` on a descending list.
fn bisect_right_desc_geq(
    keys: &Matrix<'_>,
    axis: Axis,
    n: usize,
    lookup: &ExcelValue,
) -> Option<usize> {
    let mut lo = 0usize;
    let mut hi = n;
    while lo < hi {
        let mid = (lo + hi) / 2;
        if geq(lookup_at(keys, axis, mid), lookup) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo == 0 {
        None
    } else {
        Some(lo - 1)
    }
}

fn key_matches(lookup: &ExcelValue, key: &ExcelValue, mode: MatchMode) -> bool {
    match mode {
        MatchMode::Wildcard => wildcard_match(lookup, key),
        _ => exact_match(lookup, key),
    }
}

/// Type-strict exact match (not Excel `=`): `TRUE` ≠ `1`, blank ≠ `0` ≠ `""`.
fn exact_match(lookup: &ExcelValue, key: &ExcelValue) -> bool {
    match (lookup, key) {
        (ExcelValue::Number(a), ExcelValue::Number(b)) => excel_num_eq(*a, *b),
        (ExcelValue::Text(a), ExcelValue::Text(b)) => a.eq_ignore_ascii_case(b),
        (ExcelValue::Bool(a), ExcelValue::Bool(b)) => a == b,
        (ExcelValue::Empty, ExcelValue::Empty) => true,
        (ExcelValue::Error(a), ExcelValue::Error(b)) => a == b,
        _ => false,
    }
}

fn wildcard_match(lookup: &ExcelValue, key: &ExcelValue) -> bool {
    match (lookup, key) {
        (ExcelValue::Text(pat), ExcelValue::Text(text)) => excel_wildcard(pat, text),
        // No wildcards in a non-text lookup: fall back to type-strict exact.
        _ => exact_match(lookup, key),
    }
}

fn approx_candidate(lookup: &ExcelValue, key: &ExcelValue, mode: MatchMode) -> bool {
    match mode {
        MatchMode::NextSmaller => leq(key, lookup),
        MatchMode::NextLarger => geq(key, lookup),
        _ => false,
    }
}

fn better_approx(
    _lookup: &ExcelValue,
    key: &ExcelValue,
    best: Option<&ExcelValue>,
    mode: MatchMode,
) -> bool {
    let Some(best) = best else {
        return true;
    };
    match mode {
        MatchMode::NextSmaller => gt(key, best),
        MatchMode::NextLarger => lt(key, best),
        _ => false,
    }
}

fn lt(l: &ExcelValue, r: &ExcelValue) -> bool {
    compare::ordered(l, r, std::cmp::Ordering::Less, false)
}

fn gt(l: &ExcelValue, r: &ExcelValue) -> bool {
    compare::ordered(l, r, std::cmp::Ordering::Greater, false)
}

fn leq(l: &ExcelValue, r: &ExcelValue) -> bool {
    compare::ordered(l, r, std::cmp::Ordering::Greater, true)
}

fn geq(l: &ExcelValue, r: &ExcelValue) -> bool {
    compare::ordered(l, r, std::cmp::Ordering::Less, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(x: f64) -> ExcelValue {
        ExcelValue::Number(x)
    }
    fn t(s: &str) -> ExcelValue {
        ExcelValue::Text(s.into())
    }
    fn col(vals: &[ExcelValue]) -> ExcelValue {
        ExcelValue::Array(vals.iter().cloned().map(|v| vec![v]).collect())
    }
    fn row(vals: &[ExcelValue]) -> ExcelValue {
        ExcelValue::Array(vec![vals.to_vec()])
    }

    fn xl(
        lookup: ExcelValue,
        keys: ExcelValue,
        ret: ExcelValue,
        if_not_found: Option<ExcelValue>,
        match_mode: Option<ExcelValue>,
        search_mode: Option<ExcelValue>,
    ) -> ExcelValue {
        xlookup(
            &lookup,
            &keys,
            &ret,
            if_not_found.as_ref(),
            match_mode.as_ref(),
            search_mode.as_ref(),
        )
    }

    fn both_eq(
        lookup: &ExcelValue,
        keys: &ExcelValue,
        ret: &ExcelValue,
        if_not_found: Option<&ExcelValue>,
        match_mode: Option<&ExcelValue>,
        search_mode: Option<&ExcelValue>,
    ) {
        assert_eq!(
            xlookup(lookup, keys, ret, if_not_found, match_mode, search_mode),
            xlookup_naive(lookup, keys, ret, if_not_found, match_mode, search_mode)
        );
    }

    #[test]
    fn exact_default_miss_is_na() {
        let keys = col(&[n(1.0), n(5.0), n(10.0)]);
        let ret = col(&[t("a"), t("b"), t("c")]);
        assert_eq!(
            xl(n(6.0), keys, ret, None, None, None),
            ExcelValue::Error(ExcelError::Na)
        );
    }

    #[test]
    fn exact_hit() {
        let keys = col(&[n(1.0), n(5.0), n(10.0)]);
        let ret = col(&[t("a"), t("b"), t("c")]);
        assert_eq!(xl(n(5.0), keys, ret, None, None, None), t("b"));
    }

    #[test]
    fn if_not_found_on_miss_only() {
        let keys = col(&[n(1.0), n(5.0)]);
        let ret = col(&[t("a"), t("b")]);
        assert_eq!(
            xl(
                n(9.0),
                keys.clone(),
                ret.clone(),
                Some(t("miss")),
                None,
                None
            ),
            t("miss")
        );
        assert_eq!(
            xl(
                n(1.0),
                keys,
                ret,
                Some(ExcelValue::Error(ExcelError::Div0)),
                None,
                None
            ),
            t("a")
        );
    }

    #[test]
    fn last_to_first_takes_last_duplicate() {
        let keys = col(&[n(5.0), n(5.0), n(5.0)]);
        let ret = col(&[t("a"), t("b"), t("c")]);
        assert_eq!(
            xl(
                n(5.0),
                keys.clone(),
                ret.clone(),
                None,
                Some(n(0.0)),
                Some(n(1.0))
            ),
            t("a")
        );
        assert_eq!(
            xl(n(5.0), keys, ret, None, Some(n(0.0)), Some(n(-1.0))),
            t("c")
        );
    }

    #[test]
    fn next_smaller_and_larger() {
        let keys = col(&[n(1.0), n(5.0), n(10.0)]);
        let ret = col(&[t("a"), t("b"), t("c")]);
        assert_eq!(
            xl(
                n(6.0),
                keys.clone(),
                ret.clone(),
                Some(t("miss")),
                Some(n(-1.0)),
                None
            ),
            t("b")
        );
        assert_eq!(
            xl(
                n(6.0),
                keys.clone(),
                ret.clone(),
                Some(t("miss")),
                Some(n(1.0)),
                None
            ),
            t("c")
        );
        assert_eq!(
            xl(
                n(0.0),
                keys.clone(),
                ret.clone(),
                Some(t("miss")),
                Some(n(-1.0)),
                None
            ),
            t("miss")
        );
        assert_eq!(
            xl(n(100.0), keys, ret, Some(t("miss")), Some(n(1.0)), None),
            t("miss")
        );
    }

    #[test]
    fn wildcard_is_opt_in() {
        let keys = col(&[t("apple"), t("a*")]);
        let ret = col(&[n(1.0), n(2.0)]);
        assert_eq!(
            xl(
                t("a*"),
                keys.clone(),
                ret.clone(),
                Some(t("miss")),
                Some(n(0.0)),
                None
            ),
            n(2.0)
        );
        assert_eq!(
            xl(t("a*"), keys, ret, Some(t("miss")), Some(n(2.0)), None),
            n(1.0)
        );
    }

    #[test]
    fn type_strict_exact() {
        let keys = col(&[ExcelValue::Bool(true), n(1.0), t("1")]);
        let ret = col(&[t("bool"), t("num"), t("text")]);
        assert_eq!(
            xl(n(1.0), keys.clone(), ret.clone(), None, None, None),
            t("num")
        );
        assert_eq!(
            xl(
                ExcelValue::Bool(true),
                keys.clone(),
                ret.clone(),
                None,
                None,
                None
            ),
            t("bool")
        );
        assert_eq!(xl(t("1"), keys, ret, None, None, None), t("text"));
    }

    #[test]
    fn blank_is_not_zero() {
        let keys = col(&[ExcelValue::Empty, n(0.0)]);
        let ret = col(&[t("blank"), t("zero")]);
        assert_eq!(
            xl(n(0.0), keys.clone(), ret.clone(), None, None, None),
            t("zero")
        );
        assert_eq!(
            xl(ExcelValue::Empty, keys, ret, None, None, None),
            t("blank")
        );
    }

    #[test]
    fn binary_unsorted_exact_can_miss() {
        let keys = col(&[n(1.0), n(10.0), n(5.0)]);
        let ret = col(&[t("a"), t("b"), t("c")]);
        assert_eq!(
            xl(
                n(5.0),
                keys.clone(),
                ret.clone(),
                Some(t("miss")),
                Some(n(0.0)),
                Some(n(1.0))
            ),
            t("c")
        );
        assert_eq!(
            xl(
                n(5.0),
                keys,
                ret,
                Some(t("miss")),
                Some(n(0.0)),
                Some(n(2.0))
            ),
            t("miss")
        );
    }

    #[test]
    fn binary_unsorted_approx_wrong_row() {
        let keys = col(&[n(1.0), n(10.0), n(5.0)]);
        let ret = col(&[t("a"), t("b"), t("c")]);
        assert_eq!(
            xl(
                n(6.0),
                keys,
                ret,
                Some(t("miss")),
                Some(n(-1.0)),
                Some(n(2.0))
            ),
            t("a")
        );
    }

    #[test]
    fn wildcard_plus_binary_is_value() {
        let keys = col(&[t("a"), t("b")]);
        let ret = col(&[n(1.0), n(2.0)]);
        assert_eq!(
            xl(t("a*"), keys, ret, None, Some(n(2.0)), Some(n(2.0))),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn return_row_from_2d() {
        let keys = col(&[n(1.0), n(5.0), n(10.0)]);
        let ret = ExcelValue::Array(vec![
            vec![t("a"), t("x")],
            vec![t("b"), t("y")],
            vec![t("c"), t("z")],
        ]);
        assert_eq!(
            xl(n(5.0), keys, ret, None, None, None),
            ExcelValue::Array(vec![vec![t("b"), t("y")]])
        );
    }

    #[test]
    fn horizontal_lookup() {
        let keys = row(&[n(1.0), n(5.0), n(10.0)]);
        let ret = row(&[t("a"), t("b"), t("c")]);
        assert_eq!(xl(n(5.0), keys, ret, None, None, None), t("b"));
    }

    #[test]
    fn dim_mismatch_is_value() {
        let keys = col(&[n(1.0), n(2.0), n(3.0)]);
        let ret = col(&[t("a"), t("b")]);
        assert_eq!(
            xl(n(1.0), keys, ret, None, None, None),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn invalid_modes_are_value() {
        let keys = col(&[n(1.0)]);
        let ret = col(&[n(2.0)]);
        assert_eq!(
            xl(n(1.0), keys.clone(), ret.clone(), None, Some(n(3.0)), None),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            xl(n(1.0), keys, ret, None, Some(n(0.0)), Some(n(0.0))),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn truncates_match_mode() {
        let keys = col(&[n(1.0), n(5.0), n(10.0)]);
        let ret = col(&[t("a"), t("b"), t("c")]);
        assert_eq!(
            xl(n(6.0), keys, ret, Some(t("miss")), Some(n(-1.9)), None),
            t("b")
        );
    }

    #[test]
    fn casefold_exact() {
        let keys = col(&[t("Apple"), t("Banana")]);
        let ret = col(&[n(1.0), n(2.0)]);
        assert_eq!(xl(t("apple"), keys, ret, None, None, None), n(1.0));
    }

    #[test]
    fn fifteen_digit_exact() {
        let keys = col(&[n(0.1 + 0.2)]);
        let ret = col(&[n(1.0)]);
        assert_eq!(xl(n(0.3), keys, ret, None, None, None), n(1.0));
    }

    #[test]
    fn descending_binary_exact() {
        let keys = col(&[n(10.0), n(5.0), n(1.0)]);
        let ret = col(&[t("c"), t("b"), t("a")]);
        assert_eq!(
            xl(n(5.0), keys, ret, None, Some(n(0.0)), Some(n(-2.0))),
            t("b")
        );
    }

    #[test]
    fn naive_matches_fast_on_linear_cases() {
        let keys = col(&[n(1.0), n(5.0), n(5.0), n(10.0)]);
        let ret = col(&[t("a"), t("b"), t("c"), t("d")]);
        for mm in [0.0, -1.0, 1.0] {
            for sm in [1.0, -1.0] {
                both_eq(
                    &n(5.0),
                    &keys,
                    &ret,
                    Some(&t("miss")),
                    Some(&n(mm)),
                    Some(&n(sm)),
                );
                both_eq(
                    &n(6.0),
                    &keys,
                    &ret,
                    Some(&t("miss")),
                    Some(&n(mm)),
                    Some(&n(sm)),
                );
            }
        }
        both_eq(
            &t("a*"),
            &col(&[t("apple"), t("a*")]),
            &col(&[n(1.0), n(2.0)]),
            None,
            Some(&n(2.0)),
            None,
        );
    }

    #[test]
    fn large_exact_and_binary_agree_with_naive_on_sorted() {
        let n_keys = 2_048usize;
        let keys = col(&(0..n_keys).map(|i| n(i as f64)).collect::<Vec<_>>());
        let ret = col(&(0..n_keys).map(|i| n((i * 3) as f64)).collect::<Vec<_>>());
        both_eq(&n(1_000.0), &keys, &ret, None, Some(&n(0.0)), Some(&n(1.0)));
        both_eq(&n(1_000.0), &keys, &ret, None, Some(&n(0.0)), Some(&n(2.0)));
        both_eq(
            &n(1_000.5),
            &keys,
            &ret,
            Some(&t("miss")),
            Some(&n(-1.0)),
            Some(&n(2.0)),
        );
        let got = xlookup(&n(1_000.0), &keys, &ret, None, Some(&n(0.0)), Some(&n(2.0)));
        assert_eq!(got, n(3_000.0));
    }
}
