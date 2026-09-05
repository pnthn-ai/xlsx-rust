//! Excel `SWITCH` kernel.
//!
//! Semantics (desktop Excel, 2016+):
//!
//! ```text
//! SWITCH(expression, value1, result1, [value2, result2], …, [default])
//! ```
//!
//! - Expression is evaluated **once**, then compared to each `valueN` with
//!   Excel `=` (exact match: case-insensitive text, `TRUE=1` / `FALSE=0`,
//!   empty-cell duality, 15-digit numbers). `"2"=2` is FALSE — no arithmetic
//!   coercion, unlike `IF`'s truthiness (`IF(2, …)` is the true branch).
//! - First match wins. Unused later values **and** unused results are not
//!   evaluated (same short-circuit family as `IF` / `CHOOSE`).
//! - An unpaired last argument is the default. No match and no default → `#N/A`
//!   (a nested `IF` with a missing else returns `FALSE` instead).
//! - An error in the expression propagates immediately; it does **not** fall
//!   through to `default`. An error in a compared `valueN` also propagates
//!   (the `=` of an error is that error).
//! - `*` / `?` are literal characters (unlike exact `MATCH` / `VLOOKUP`).
//!
//! [`first_match`] is the production scan. [`first_match_naive`] rebuilds a
//! nested-`IF` comparison (`expr=value`) on every pair so the bench can print
//! a before/after against the specialized type dispatch.

use super::compare;
use xlsx_types::{ExcelError, ExcelValue};

/// Excel `=` between a SWITCH expression and one candidate value.
pub fn matches(expr: &ExcelValue, value: &ExcelValue) -> bool {
    first_match(expr, std::slice::from_ref(value)).is_some()
}

/// First index in `values` that Excel-equals `expr`, or `None`.
///
/// `expr` must already be a non-error scalar. An error in `values[i]` is
/// **not** handled here — the evaluator surfaces it before calling this.
pub fn first_match(expr: &ExcelValue, values: &[ExcelValue]) -> Option<usize> {
    match expr {
        ExcelValue::Number(n) => values.iter().position(|v| number_eq(*n, v)),
        ExcelValue::Text(s) => values.iter().position(|v| text_eq(s, v)),
        ExcelValue::Bool(b) => values.iter().position(|v| bool_eq(*b, v)),
        ExcelValue::Empty => values.iter().position(|v| empty_eq(v)),
        _ => values.iter().position(|v| compare::equal(expr, v)),
    }
}

/// Nested-`IF` baseline: `IF(expr=v1, …, IF(expr=v2, …))` re-runs the full
/// equality kernel (including a cloned expression) on every pair.
pub fn first_match_naive(expr: &ExcelValue, values: &[ExcelValue]) -> Option<usize> {
    values.iter().position(|v| {
        let lhs = expr.clone();
        compare::equal(&lhs, v)
    })
}

/// Pick a result after every argument has already been evaluated.
///
/// `args[0]` is the expression. Pair layout and default follow Excel
/// (`len` even → last arg is default). Used by the eager bench path and tests.
/// Errors in `args[0]` or in a compared value win left-to-right.
pub fn pick_evaluated(args: &[ExcelValue]) -> ExcelValue {
    if args.len() < 3 {
        return ExcelValue::Error(ExcelError::Value);
    }
    let expr = &args[0];
    if let ExcelValue::Error(e) = expr {
        return ExcelValue::Error(*e);
    }
    let has_default = args.len() % 2 == 0;
    let pair_end = if has_default {
        args.len() - 1
    } else {
        args.len()
    };
    let mut i = 1;
    while i < pair_end {
        match &args[i] {
            ExcelValue::Error(e) => return ExcelValue::Error(*e),
            value if compare::equal(expr, value) => return args[i + 1].clone(),
            _ => i += 2,
        }
    }
    if has_default {
        args[args.len() - 1].clone()
    } else {
        ExcelValue::Error(ExcelError::Na)
    }
}

fn number_eq(n: f64, v: &ExcelValue) -> bool {
    match v {
        ExcelValue::Number(m) => compare::num_eq(n, *m),
        ExcelValue::Bool(b) => compare::num_eq(n, if *b { 1.0 } else { 0.0 }),
        ExcelValue::Empty => compare::num_eq(n, 0.0),
        _ => false,
    }
}

fn text_eq(s: &str, v: &ExcelValue) -> bool {
    match v {
        ExcelValue::Text(t) => s.eq_ignore_ascii_case(t),
        ExcelValue::Empty => s.is_empty(),
        _ => false,
    }
}

fn bool_eq(b: bool, v: &ExcelValue) -> bool {
    match v {
        ExcelValue::Bool(c) => b == *c,
        ExcelValue::Number(n) => compare::num_eq(*n, if b { 1.0 } else { 0.0 }),
        ExcelValue::Empty => !b,
        _ => false,
    }
}

fn empty_eq(v: &ExcelValue) -> bool {
    match v {
        ExcelValue::Empty => true,
        ExcelValue::Number(n) => compare::num_eq(*n, 0.0),
        ExcelValue::Text(s) => s.is_empty(),
        ExcelValue::Bool(b) => !*b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn both(expr: &ExcelValue, values: &[ExcelValue]) -> Option<usize> {
        let fast = first_match(expr, values);
        let slow = first_match_naive(expr, values);
        assert_eq!(fast, slow, "naive/fast mismatch for {expr:?} vs {values:?}");
        fast
    }

    fn n(x: f64) -> ExcelValue {
        ExcelValue::Number(x)
    }
    fn t(s: &str) -> ExcelValue {
        ExcelValue::Text(s.into())
    }

    #[test]
    fn first_match_numeric() {
        let vals = [n(1.0), n(2.0), n(3.0)];
        assert_eq!(both(&n(2.0), &vals), Some(1));
        assert_eq!(both(&n(9.0), &vals), None);
        assert_eq!(both(&n(0.1 + 0.2), &[n(0.3)]), Some(0));
    }

    #[test]
    fn first_match_wins() {
        let vals = [n(1.0), n(1.0)];
        assert_eq!(both(&n(1.0), &vals), Some(0));
    }

    #[test]
    fn exact_match_not_if_truthiness() {
        // IF(2, "yes") is true; SWITCH(2, TRUE, "yes") does not match.
        assert_eq!(both(&n(2.0), &[ExcelValue::Bool(true)]), None);
        assert_eq!(both(&n(1.0), &[ExcelValue::Bool(true)]), Some(0));
        assert_eq!(both(&ExcelValue::Bool(true), &[n(1.0)]), Some(0));
        assert_eq!(both(&ExcelValue::Bool(false), &[n(0.0)]), Some(0));
    }

    #[test]
    fn no_text_number_coercion() {
        assert_eq!(both(&t("2"), &[n(2.0)]), None);
        assert_eq!(both(&n(2.0), &[t("2")]), None);
        assert_eq!(both(&t("2"), &[t("2")]), Some(0));
    }

    #[test]
    fn case_insensitive_text() {
        assert_eq!(both(&t("A"), &[t("a")]), Some(0));
        assert_eq!(both(&t("AbC"), &[t("abc")]), Some(0));
    }

    #[test]
    fn empty_duality() {
        assert_eq!(both(&ExcelValue::Empty, &[n(0.0)]), Some(0));
        assert_eq!(both(&ExcelValue::Empty, &[t("")]), Some(0));
        assert_eq!(both(&ExcelValue::Empty, &[n(0.0), t("")]), Some(0));
        assert_eq!(both(&n(0.0), &[t("")]), None);
    }

    #[test]
    fn wildcards_are_literal() {
        assert_eq!(both(&t("abc"), &[t("a*")]), None);
        assert_eq!(both(&t("a*"), &[t("a*")]), Some(0));
        assert_eq!(both(&t("a?c"), &[t("a?c")]), Some(0));
    }

    #[test]
    fn pick_evaluated_default_and_na() {
        let hit = pick_evaluated(&[n(2.0), n(1.0), t("a"), n(2.0), t("b")]);
        assert_eq!(hit, t("b"));
        let miss = pick_evaluated(&[n(9.0), n(1.0), t("a"), n(2.0), t("b")]);
        assert_eq!(miss, ExcelValue::Error(ExcelError::Na));
        let def = pick_evaluated(&[n(9.0), n(1.0), t("a"), t("none")]);
        assert_eq!(def, t("none"));
    }

    #[test]
    fn pick_evaluated_arity_and_errors() {
        assert_eq!(
            pick_evaluated(&[n(1.0)]),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            pick_evaluated(&[ExcelValue::Error(ExcelError::Div0), n(1.0), t("a"), t("d")]),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            pick_evaluated(&[
                n(1.0),
                ExcelValue::Error(ExcelError::Na),
                t("a"),
                n(1.0),
                t("b")
            ]),
            ExcelValue::Error(ExcelError::Na)
        );
    }
}
