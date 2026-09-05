//! Excel `IFS` selection kernel.
//!
//! Excel evaluates **every** `IFS` argument (unlike `IF` / `CHOOSE` /
//! `IFNA`). After that eager walk:
//!
//! - the first left-to-right error wins (argument error or a non-logical test);
//! - otherwise the value of the first TRUE test is returned;
//! - if no test is TRUE, the result is `#N/A`.
//!
//! `select` is a single pass that clones only the winning value.
//! `select_naive` materializes every pair, then scans — same answers, more
//! allocation. The production evaluator streams `eval_*` into [`fold_pair`]
//! so it never builds the argument vector.

use super::coerce;
use xlsx_types::{ExcelError, ExcelValue};

/// Excel `IFS` from already-evaluated arguments (even length, pairs of
/// `logical_test, value`).
///
/// Odd / empty argument lists are `#VALUE!` (too few arguments).
pub fn select(args: &[ExcelValue]) -> ExcelValue {
    if args.is_empty() || args.len() % 2 == 1 {
        return ExcelValue::Error(ExcelError::Value);
    }
    let mut first_err = None;
    let mut first_true = None;
    for pair in args.chunks_exact(2) {
        fold_pair(&pair[0], pair[1].clone(), &mut first_err, &mut first_true);
    }
    finish(first_err, first_true)
}

/// Allocation-heavy baseline: build `(logical, value)` pairs, then scan.
///
/// Same answers as [`select`]. Used as the bench "before".
pub fn select_naive(args: &[ExcelValue]) -> ExcelValue {
    if args.is_empty() || args.len() % 2 == 1 {
        return ExcelValue::Error(ExcelError::Value);
    }
    let mut pairs: Vec<(Result<bool, ExcelError>, ExcelValue)> = Vec::with_capacity(args.len() / 2);
    for pair in args.chunks_exact(2) {
        let logical = match &pair[0] {
            ExcelValue::Error(e) => Err(*e),
            other => coerce::to_logical(other),
        };
        pairs.push((logical, pair[1].clone()));
    }
    let mut first_err = None;
    let mut first_true = None;
    for (logical, val) in pairs {
        if first_err.is_some() {
            continue;
        }
        if let ExcelValue::Error(e) = val {
            first_err = Some(e);
            continue;
        }
        match logical {
            Ok(true) if first_true.is_none() => first_true = Some(val),
            Ok(_) => {}
            Err(e) => first_err = Some(e),
        }
    }
    finish(first_err, first_true)
}

/// Fold one evaluated `(test, value)` pair. Later pairs still run so unused
/// `#DIV/0!` / non-logical tests fire — Excel's no-short-circuit quirk.
pub fn fold_pair(
    cond: &ExcelValue,
    val: ExcelValue,
    first_err: &mut Option<ExcelError>,
    first_true: &mut Option<ExcelValue>,
) {
    if first_err.is_some() {
        return;
    }
    if let ExcelValue::Error(e) = cond {
        *first_err = Some(*e);
        return;
    }
    if let ExcelValue::Error(e) = val {
        *first_err = Some(e);
        return;
    }
    match coerce::to_logical(cond) {
        Ok(true) => {
            if first_true.is_none() {
                *first_true = Some(val);
            }
        }
        Ok(false) => {}
        Err(e) => *first_err = Some(e),
    }
}

pub fn finish(first_err: Option<ExcelError>, first_true: Option<ExcelValue>) -> ExcelValue {
    if let Some(e) = first_err {
        return ExcelValue::Error(e);
    }
    first_true.unwrap_or(ExcelValue::Error(ExcelError::Na))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(x: f64) -> ExcelValue {
        ExcelValue::Number(x)
    }

    #[test]
    fn first_true_wins() {
        let args = [
            ExcelValue::Bool(false),
            n(1.0),
            ExcelValue::Bool(true),
            n(2.0),
            ExcelValue::Bool(true),
            n(3.0),
        ];
        assert_eq!(select(&args), n(2.0));
        assert_eq!(select_naive(&args), n(2.0));
    }

    #[test]
    fn no_match_is_na() {
        let args = [
            ExcelValue::Bool(false),
            n(1.0),
            ExcelValue::Bool(false),
            n(2.0),
        ];
        assert_eq!(select(&args), ExcelValue::Error(ExcelError::Na));
        assert_eq!(select_naive(&args), ExcelValue::Error(ExcelError::Na));
    }

    #[test]
    fn unused_error_still_fires() {
        let args = [
            ExcelValue::Bool(true),
            n(1.0),
            ExcelValue::Bool(false),
            ExcelValue::Error(ExcelError::Div0),
        ];
        assert_eq!(select(&args), ExcelValue::Error(ExcelError::Div0));
        assert_eq!(select_naive(&args), ExcelValue::Error(ExcelError::Div0));
    }

    #[test]
    fn later_non_logical_is_value() {
        let args = [
            ExcelValue::Bool(true),
            n(1.0),
            ExcelValue::Text("x".into()),
            n(2.0),
        ];
        assert_eq!(select(&args), ExcelValue::Error(ExcelError::Value));
    }

    #[test]
    fn first_error_ltr() {
        let args = [
            ExcelValue::Error(ExcelError::Div0),
            n(1.0),
            ExcelValue::Error(ExcelError::Na),
            n(2.0),
        ];
        assert_eq!(select(&args), ExcelValue::Error(ExcelError::Div0));
    }

    #[test]
    fn odd_arity_is_value() {
        assert_eq!(
            select(&[ExcelValue::Bool(true)]),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(select(&[]), ExcelValue::Error(ExcelError::Value));
    }

    #[test]
    fn number_coercion() {
        let args = [n(0.0), n(1.0), n(2.0), n(3.0)];
        assert_eq!(select(&args), n(3.0));
    }
}
