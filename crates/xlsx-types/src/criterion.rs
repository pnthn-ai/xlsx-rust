//! Excel `SUMIF` / `COUNTIF` / `AVERAGEIF`-style criteria.
//!
//! Compiled once per call. The matcher never reads fixture expected values.

use crate::error::ExcelError;
use crate::value::{excel_num_eq, ExcelValue};

/// Comparison extracted from a criteria string (`">5"`, `"<>"`, `"apple"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RelOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Right-hand side after the optional operator is stripped.
#[derive(Clone, Debug)]
enum Rhs {
    Number(f64),
    Text(String),
    Wildcard(String),
    Blank,
    Error(ExcelError),
}

#[derive(Clone, Debug)]
enum Inner {
    /// Criteria was a number literal / numeric cell — numbers only.
    NumberEq(f64),
    /// Criteria was a boolean literal / logical cell — logicals only.
    BoolEq(bool),
    /// Parsed from a criteria string (operators, wildcards, dual `="5"`).
    Textual { op: RelOp, rhs: Rhs },
}

/// Compiled Excel criteria used by `AVERAGEIF` / `SUMIF` / `COUNTIF`.
#[derive(Clone, Debug)]
pub struct Criterion {
    inner: Inner,
}

impl Criterion {
    /// Compile a scalar criteria value. `Err` means the function should return
    /// that Excel error (criteria itself is an error, or a non-scalar).
    pub fn compile(v: &ExcelValue) -> Result<Self, ExcelError> {
        match v {
            ExcelValue::Error(e) => Err(*e),
            ExcelValue::Array(_) => Err(ExcelError::Value),
            ExcelValue::Number(n) => Ok(Self {
                inner: Inner::NumberEq(*n),
            }),
            ExcelValue::Bool(b) => Ok(Self {
                inner: Inner::BoolEq(*b),
            }),
            ExcelValue::Empty => Ok(Self {
                inner: Inner::Textual {
                    op: RelOp::Eq,
                    rhs: Rhs::Blank,
                },
            }),
            ExcelValue::Text(s) => Ok(parse_text_criterion(s)),
        }
    }

    /// Whether `cell` satisfies this criteria (Excel SUMIF/COUNTIF/AVERAGEIF rules).
    pub fn matches(&self, cell: &ExcelValue) -> bool {
        match &self.inner {
            Inner::NumberEq(n) => match cell {
                ExcelValue::Number(c) => excel_num_eq(*c, *n),
                _ => false,
            },
            Inner::BoolEq(b) => matches!(cell, ExcelValue::Bool(c) if c == b),
            Inner::Textual { op, rhs } => textual_match(*op, rhs, cell),
        }
    }
}

fn parse_text_criterion(s: &str) -> Criterion {
    let (op, rest) = split_op(s);
    let rhs = if rest.is_empty() {
        Rhs::Blank
    } else if rest.starts_with('#') {
        if let Some(e) = ExcelError::parse(rest) {
            Rhs::Error(e)
        } else if looks_like_wildcard(rest) {
            Rhs::Wildcard(rest.to_string())
        } else {
            Rhs::Text(rest.to_string())
        }
    } else if let Ok(n) = parse_numeric(rest) {
        Rhs::Number(n)
    } else if looks_like_wildcard(rest) {
        Rhs::Wildcard(rest.to_string())
    } else {
        Rhs::Text(rest.to_string())
    };
    Criterion {
        inner: Inner::Textual { op, rhs },
    }
}

fn split_op(s: &str) -> (RelOp, &str) {
    if let Some(rest) = s.strip_prefix("<=") {
        (RelOp::Le, rest)
    } else if let Some(rest) = s.strip_prefix(">=") {
        (RelOp::Ge, rest)
    } else if let Some(rest) = s.strip_prefix("<>") {
        (RelOp::Ne, rest)
    } else if let Some(rest) = s.strip_prefix('<') {
        (RelOp::Lt, rest)
    } else if let Some(rest) = s.strip_prefix('>') {
        (RelOp::Gt, rest)
    } else if let Some(rest) = s.strip_prefix('=') {
        (RelOp::Eq, rest)
    } else {
        (RelOp::Eq, s)
    }
}

fn parse_numeric(s: &str) -> Result<f64, ()> {
    let t = s.trim();
    if t.is_empty() {
        return Err(());
    }
    t.parse::<f64>().map_err(|_| ())
}

fn looks_like_wildcard(s: &str) -> bool {
    looks_like_wildcard_pat(s)
}

fn is_blank(cell: &ExcelValue) -> bool {
    match cell {
        ExcelValue::Empty => true,
        ExcelValue::Text(s) if s.is_empty() => true,
        _ => false,
    }
}

fn textual_match(op: RelOp, rhs: &Rhs, cell: &ExcelValue) -> bool {
    match (op, rhs) {
        (RelOp::Eq, Rhs::Blank) => is_blank(cell),
        (RelOp::Ne, Rhs::Blank) => !is_blank(cell),
        (RelOp::Lt | RelOp::Le | RelOp::Gt | RelOp::Ge, Rhs::Blank) => false,

        (RelOp::Eq, Rhs::Number(n)) => match_number_or_numeric_text(cell, *n),
        (RelOp::Ne, Rhs::Number(n)) => !match_number_or_numeric_text(cell, *n),
        (RelOp::Lt | RelOp::Le | RelOp::Gt | RelOp::Ge, Rhs::Number(n)) => match cell {
            ExcelValue::Number(c) => num_rel(*c, op, *n),
            _ => false,
        },

        (RelOp::Eq, Rhs::Text(pat)) => text_eq_cell(cell, pat),
        (RelOp::Ne, Rhs::Text(pat)) => !text_eq_cell(cell, pat),
        (RelOp::Lt | RelOp::Le | RelOp::Gt | RelOp::Ge, Rhs::Text(pat)) => match cell {
            ExcelValue::Text(s) => text_rel(s, op, pat),
            _ => false,
        },

        (RelOp::Eq, Rhs::Wildcard(pat)) => wildcard_cell(cell, pat),
        (RelOp::Ne, Rhs::Wildcard(pat)) => !wildcard_cell(cell, pat),
        (RelOp::Lt | RelOp::Le | RelOp::Gt | RelOp::Ge, Rhs::Wildcard(_)) => false,

        (RelOp::Eq, Rhs::Error(e)) => match cell {
            ExcelValue::Error(c) => c == e,
            ExcelValue::Text(s) => ExcelError::parse(s) == Some(*e) && s.starts_with('#'),
            _ => false,
        },
        (RelOp::Ne, Rhs::Error(e)) => !matches!(cell, ExcelValue::Error(c) if c == e),
        (RelOp::Lt | RelOp::Le | RelOp::Gt | RelOp::Ge, Rhs::Error(_)) => false,
    }
}

fn match_number_or_numeric_text(cell: &ExcelValue, n: f64) -> bool {
    match cell {
        ExcelValue::Number(c) => excel_num_eq(*c, n),
        ExcelValue::Text(s) => parse_numeric(s).ok().is_some_and(|t| excel_num_eq(t, n)),
        _ => false,
    }
}

fn text_eq_cell(cell: &ExcelValue, pat: &str) -> bool {
    match cell {
        ExcelValue::Text(s) => s.eq_ignore_ascii_case(pat),
        _ => false,
    }
}

fn wildcard_cell(cell: &ExcelValue, pat: &str) -> bool {
    match cell {
        ExcelValue::Text(s) => excel_wildcard(pat, s),
        _ => false,
    }
}

fn num_rel(cell: f64, op: RelOp, rhs: f64) -> bool {
    let eq = excel_num_eq(cell, rhs);
    match op {
        RelOp::Eq => eq,
        RelOp::Ne => !eq,
        RelOp::Lt => cell < rhs && !eq,
        RelOp::Gt => cell > rhs && !eq,
        RelOp::Le => cell < rhs || eq,
        RelOp::Ge => cell > rhs || eq,
    }
}

fn text_rel(cell: &str, op: RelOp, rhs: &str) -> bool {
    let a = cell.to_ascii_lowercase();
    let b = rhs.to_ascii_lowercase();
    match op {
        RelOp::Eq => a == b,
        RelOp::Ne => a != b,
        RelOp::Lt => a < b,
        RelOp::Gt => a > b,
        RelOp::Le => a <= b,
        RelOp::Ge => a >= b,
    }
}

/// Excel `*` / `?` / `~` wildcard match, case-insensitive (ASCII).
pub fn excel_wildcard(pat: &str, text: &str) -> bool {
    fn rec(p: &[u8], t: &[u8]) -> bool {
        if p.is_empty() {
            return t.is_empty();
        }
        if p[0] == b'~' && p.len() >= 2 {
            return !t.is_empty() && t[0] == p[1] && rec(&p[2..], &t[1..]);
        }
        if p[0] == b'*' {
            return rec(&p[1..], t) || (!t.is_empty() && rec(p, &t[1..]));
        }
        if p[0] == b'?' {
            return !t.is_empty() && rec(&p[1..], &t[1..]);
        }
        !t.is_empty() && p[0] == t[0] && rec(&p[1..], &t[1..])
    }
    rec(
        pat.to_ascii_lowercase().as_bytes(),
        text.to_ascii_lowercase().as_bytes(),
    )
}

/// Whether `pat` contains an unescaped `*` / `?` (or a `~` escape).
pub fn looks_like_wildcard_pat(s: &str) -> bool {
    s.contains('*') || s.contains('?') || s.contains('~')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(v: f64) -> ExcelValue {
        ExcelValue::Number(v)
    }
    fn t(s: &str) -> ExcelValue {
        ExcelValue::Text(s.into())
    }

    fn matches_crit(crit: ExcelValue, cell: ExcelValue) -> bool {
        Criterion::compile(&crit).unwrap().matches(&cell)
    }

    #[test]
    fn number_criteria_is_type_strict() {
        assert!(matches_crit(n(5.0), n(5.0)));
        assert!(!matches_crit(n(5.0), t("5")));
        assert!(!matches_crit(n(0.0), ExcelValue::Empty));
        assert!(!matches_crit(n(1.0), ExcelValue::Bool(true)));
    }

    #[test]
    fn text_numeric_criteria_dual_matches() {
        assert!(matches_crit(t("5"), n(5.0)));
        assert!(matches_crit(t("5"), t("5")));
        assert!(matches_crit(t("=5"), n(5.0)));
        assert!(matches_crit(t("=5"), t("5")));
        assert!(!matches_crit(t("5"), ExcelValue::Empty));
    }

    #[test]
    fn inequality_numbers_skip_text_and_blank() {
        assert!(matches_crit(t(">5"), n(6.0)));
        assert!(!matches_crit(t(">5"), n(5.0)));
        assert!(!matches_crit(t(">5"), t("10")));
        assert!(!matches_crit(t(">5"), ExcelValue::Empty));
        assert!(!matches_crit(t(">0"), ExcelValue::Bool(true)));
        assert!(matches_crit(t(">=5"), n(5.0)));
        assert!(matches_crit(t("<>5"), ExcelValue::Empty));
        assert!(matches_crit(t("<>5"), t("x")));
        assert!(!matches_crit(t("<>5"), n(5.0)));
        assert!(!matches_crit(t("<>5"), t("5")));
    }

    #[test]
    fn blank_and_not_blank() {
        assert!(matches_crit(t(""), ExcelValue::Empty));
        assert!(matches_crit(t("="), ExcelValue::Empty));
        assert!(matches_crit(t(""), t("")));
        assert!(!matches_crit(t(""), n(0.0)));
        assert!(matches_crit(t("<>"), n(0.0)));
        assert!(matches_crit(t("<>"), t("a")));
        assert!(!matches_crit(t("<>"), ExcelValue::Empty));
        assert!(!matches_crit(t("<>"), t("")));
    }

    #[test]
    fn text_casefold_and_wildcards() {
        assert!(matches_crit(t("apple"), t("APPLE")));
        assert!(matches_crit(t("*a*"), t("cat")));
        assert!(matches_crit(t("a?"), t("ab")));
        assert!(!matches_crit(t("a?"), t("a")));
        assert!(matches_crit(t("a~*"), t("a*")));
        assert!(!matches_crit(t("a~*"), t("ab")));
        assert!(!matches_crit(t("*"), n(5.0)));
        assert!(!matches_crit(t("*"), ExcelValue::Empty));
        assert!(matches_crit(t("<>*"), n(5.0)));
        assert!(matches_crit(t("<>*"), ExcelValue::Empty));
        assert!(!matches_crit(t("<>*"), t("x")));
    }

    #[test]
    fn bool_and_error_criteria() {
        assert!(matches_crit(ExcelValue::Bool(true), ExcelValue::Bool(true)));
        assert!(!matches_crit(ExcelValue::Bool(true), n(1.0)));
        assert!(!matches_crit(t("TRUE"), ExcelValue::Bool(true)));
        assert!(matches_crit(t("#N/A"), ExcelValue::Error(ExcelError::Na)));
        assert!(Criterion::compile(&ExcelValue::Error(ExcelError::Div0)).is_err());
    }

    #[test]
    fn text_inequality() {
        assert!(matches_crit(t(">a"), t("b")));
        assert!(!matches_crit(t(">a"), n(1.0)));
        assert!(matches_crit(t("<>apple"), n(1.0)));
        assert!(!matches_crit(t("<>apple"), t("APPLE")));
    }
}
