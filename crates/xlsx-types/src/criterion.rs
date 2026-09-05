//! Excel `COUNTIF` / `SUMIF`-style criterion matching.
//!
//! Parsed once, then applied per cell. Candidates use this so goldens stay
//! aligned; the matcher never reads fixture expected values.
//!
//! Semantics follow Excel's COUNTIF (not ordinary `=`):
//! - `"2"` / `2` match both the number 2 and numeric text `"2"`
//! - `TRUE=1` does **not** apply: `COUNTIF(range, TRUE)` counts logicals only
//! - `"TRUE"` is treated as the logical TRUE (not the text), unless a wildcard
//!   forces a text match (`"TRUE*"`)
//! - `""` / `"="` match blanks **and** stored `""`; `"<>"` matches the rest
//!   (except error cells)
//! - `"<>5"` counts blanks (blank is not 5) but skips error cells
//! - `COUNTIF(range, 0)` does **not** count blanks
//! - inequalities with a numeric remainder compare numbers only
//! - inequalities with a text remainder compare text only (case-insensitive)
//! - wildcards (`*` / `?` / `~`) apply to text `=` / `<>` only
//! - error cells are ignored unless the criterion is that error (`#N/A`, `NA()`)

use crate::error::ExcelError;
use crate::value::{excel_num_eq, ExcelValue};

/// A compiled COUNTIF criterion.
#[derive(Clone, Debug, PartialEq)]
pub struct Criterion {
    op: CritOp,
    kind: CritKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CritOp {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

#[derive(Clone, Debug, PartialEq)]
enum CritKind {
    Number(f64),
    Text { pat: String, wildcard: bool },
    Bool(bool),
    Error(ExcelError),
    Empty,
}

impl Criterion {
    /// Parse a scalar criterion value (already implicit-intersected).
    pub fn parse(v: &ExcelValue) -> Self {
        match v {
            ExcelValue::Number(n) => Self {
                op: CritOp::Eq,
                kind: CritKind::Number(*n),
            },
            ExcelValue::Bool(b) => Self {
                op: CritOp::Eq,
                kind: CritKind::Bool(*b),
            },
            ExcelValue::Error(e) => Self {
                op: CritOp::Eq,
                kind: CritKind::Error(*e),
            },
            ExcelValue::Empty => Self {
                op: CritOp::Eq,
                kind: CritKind::Empty,
            },
            ExcelValue::Array(rows) => rows
                .first()
                .and_then(|r| r.first())
                .map(Self::parse)
                .unwrap_or(Self {
                    op: CritOp::Eq,
                    kind: CritKind::Empty,
                }),
            ExcelValue::Text(s) => parse_text_criterion(s),
        }
    }

    /// Whether `cell` satisfies the criterion.
    pub fn matches(&self, cell: &ExcelValue) -> bool {
        if let ExcelValue::Array(rows) = cell {
            return rows.iter().flatten().any(|c| self.matches(c));
        }

        // Error cells match only an error criterion.
        if let ExcelValue::Error(got) = cell {
            return match (&self.op, &self.kind) {
                (CritOp::Eq, CritKind::Error(want)) => want == got,
                (CritOp::Ne, CritKind::Error(want)) => want != got,
                _ => false,
            };
        }

        match (&self.op, &self.kind) {
            (CritOp::Eq, CritKind::Empty) => is_blank_like(cell),
            (CritOp::Ne, CritKind::Empty) => !is_blank_like(cell),
            (CritOp::Gt | CritOp::Ge | CritOp::Lt | CritOp::Le, CritKind::Empty) => false,

            (op, CritKind::Number(n)) => match_number(*op, *n, cell),
            (op, CritKind::Bool(b)) => match_bool(*op, *b, cell),
            (op, CritKind::Text { pat, wildcard }) => match_text(*op, pat, *wildcard, cell),
            (CritOp::Eq, CritKind::Error(want)) => match_error_text(cell, *want),
            (CritOp::Ne, CritKind::Error(want)) => !match_error_text(cell, *want),
            (_, CritKind::Error(_)) => false,
        }
    }
}

/// Count how many values in `v` (scalar or array) match `criterion`.
pub fn count_matches(v: &ExcelValue, criterion: &Criterion) -> u64 {
    match v {
        ExcelValue::Array(rows) => rows
            .iter()
            .flatten()
            .map(|c| count_matches(c, criterion))
            .sum(),
        other => u64::from(criterion.matches(other)),
    }
}

fn parse_text_criterion(s: &str) -> Criterion {
    let (op, rest) = split_op(s);
    if rest.is_empty() {
        return Criterion {
            op,
            kind: CritKind::Empty,
        };
    }
    if let Some(n) = parse_numeric_text(rest) {
        return Criterion {
            op,
            kind: CritKind::Number(n),
        };
    }
    if matches!(op, CritOp::Eq | CritOp::Ne) {
        if rest.eq_ignore_ascii_case("TRUE") {
            return Criterion {
                op,
                kind: CritKind::Bool(true),
            };
        }
        if rest.eq_ignore_ascii_case("FALSE") {
            return Criterion {
                op,
                kind: CritKind::Bool(false),
            };
        }
        if rest.starts_with('#') {
            if let Some(e) = ExcelError::parse(rest) {
                return Criterion {
                    op,
                    kind: CritKind::Error(e),
                };
            }
        }
    }
    Criterion {
        op,
        kind: CritKind::Text {
            wildcard: looks_like_wildcard(rest),
            pat: rest.to_string(),
        },
    }
}

fn split_op(s: &str) -> (CritOp, &str) {
    if let Some(rest) = s.strip_prefix(">=") {
        (CritOp::Ge, rest)
    } else if let Some(rest) = s.strip_prefix("<=") {
        (CritOp::Le, rest)
    } else if let Some(rest) = s.strip_prefix("<>") {
        (CritOp::Ne, rest)
    } else if let Some(rest) = s.strip_prefix('>') {
        (CritOp::Gt, rest)
    } else if let Some(rest) = s.strip_prefix('<') {
        (CritOp::Lt, rest)
    } else if let Some(rest) = s.strip_prefix('=') {
        (CritOp::Eq, rest)
    } else {
        (CritOp::Eq, s)
    }
}

fn is_blank_like(cell: &ExcelValue) -> bool {
    match cell {
        ExcelValue::Empty => true,
        ExcelValue::Text(s) if s.is_empty() => true,
        _ => false,
    }
}

fn parse_numeric_text(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    t.parse::<f64>().ok()
}

fn cmp_num(cell: f64, crit: f64) -> std::cmp::Ordering {
    if excel_num_eq(cell, crit) {
        std::cmp::Ordering::Equal
    } else {
        cell.partial_cmp(&crit).unwrap_or(std::cmp::Ordering::Equal)
    }
}

fn match_number(op: CritOp, n: f64, cell: &ExcelValue) -> bool {
    match (op, cell) {
        (CritOp::Eq, ExcelValue::Number(c)) => excel_num_eq(*c, n),
        (CritOp::Eq, ExcelValue::Text(s)) => parse_numeric_text(s)
            .map(|c| excel_num_eq(c, n))
            .unwrap_or(false),
        (CritOp::Eq, _) => false,

        (CritOp::Ne, ExcelValue::Number(c)) => !excel_num_eq(*c, n),
        (CritOp::Ne, ExcelValue::Text(s)) => match parse_numeric_text(s) {
            Some(c) => !excel_num_eq(c, n),
            None => true,
        },
        (CritOp::Ne, _) => true,

        (CritOp::Gt, ExcelValue::Number(c)) => cmp_num(*c, n).is_gt(),
        (CritOp::Ge, ExcelValue::Number(c)) => cmp_num(*c, n).is_ge(),
        (CritOp::Lt, ExcelValue::Number(c)) => cmp_num(*c, n).is_lt(),
        (CritOp::Le, ExcelValue::Number(c)) => cmp_num(*c, n).is_le(),
        (CritOp::Gt | CritOp::Ge | CritOp::Lt | CritOp::Le, _) => false,
    }
}

fn match_bool(op: CritOp, b: bool, cell: &ExcelValue) -> bool {
    match (op, cell) {
        (CritOp::Eq, ExcelValue::Bool(c)) => *c == b,
        (CritOp::Eq, _) => false,
        (CritOp::Ne, ExcelValue::Bool(c)) => *c != b,
        (CritOp::Ne, _) => true,
        (CritOp::Gt, ExcelValue::Bool(c)) => *c && !b,
        (CritOp::Ge, ExcelValue::Bool(c)) => *c >= b,
        (CritOp::Lt, ExcelValue::Bool(c)) => !*c && b,
        (CritOp::Le, ExcelValue::Bool(c)) => *c <= b,
        (CritOp::Gt | CritOp::Ge | CritOp::Lt | CritOp::Le, _) => false,
    }
}

fn match_text(op: CritOp, pat: &str, wildcard: bool, cell: &ExcelValue) -> bool {
    match (op, cell) {
        (CritOp::Eq, ExcelValue::Text(s)) => text_eq(pat, s, wildcard),
        (CritOp::Eq, _) => false,
        (CritOp::Ne, ExcelValue::Text(s)) => !text_eq(pat, s, wildcard),
        (CritOp::Ne, _) => true,
        (CritOp::Gt, ExcelValue::Text(s)) => text_cmp(s, pat).is_gt(),
        (CritOp::Ge, ExcelValue::Text(s)) => text_cmp(s, pat).is_ge(),
        (CritOp::Lt, ExcelValue::Text(s)) => text_cmp(s, pat).is_lt(),
        (CritOp::Le, ExcelValue::Text(s)) => text_cmp(s, pat).is_le(),
        (CritOp::Gt | CritOp::Ge | CritOp::Lt | CritOp::Le, _) => false,
    }
}

fn match_error_text(cell: &ExcelValue, want: ExcelError) -> bool {
    match cell {
        ExcelValue::Error(e) => *e == want,
        ExcelValue::Text(s) => s.eq_ignore_ascii_case(want.excel_text()),
        _ => false,
    }
}

fn text_eq(pat: &str, text: &str, wildcard: bool) -> bool {
    if wildcard {
        excel_wildcard(pat, text)
    } else {
        pat.eq_ignore_ascii_case(text)
    }
}

fn text_cmp(cell: &str, crit: &str) -> std::cmp::Ordering {
    cell.to_ascii_lowercase().cmp(&crit.to_ascii_lowercase())
}

fn looks_like_wildcard(s: &str) -> bool {
    s.contains('*') || s.contains('?') || s.contains('~')
}

/// Excel `*` / `?` / `~` wildcard match (case-insensitive).
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

#[cfg(test)]
mod tests {
    use super::*;

    fn n(x: f64) -> ExcelValue {
        ExcelValue::Number(x)
    }
    fn t(s: &str) -> ExcelValue {
        ExcelValue::Text(s.into())
    }
    fn b(v: bool) -> ExcelValue {
        ExcelValue::Bool(v)
    }

    fn m(crit: ExcelValue, cell: ExcelValue) -> bool {
        Criterion::parse(&crit).matches(&cell)
    }

    #[test]
    fn number_matches_numeric_text() {
        assert!(m(n(5.0), n(5.0)));
        assert!(m(n(5.0), t("5")));
        assert!(m(t("5"), n(5.0)));
        assert!(m(t("=5"), n(5.0)));
        assert!(!m(n(5.0), t("x")));
        assert!(!m(n(5.0), ExcelValue::Empty));
        assert!(!m(n(0.0), ExcelValue::Empty));
        assert!(!m(n(1.0), b(true)));
    }

    #[test]
    fn operators() {
        assert!(m(t(">5"), n(10.0)));
        assert!(!m(t(">5"), n(5.0)));
        assert!(!m(t(">5"), t("6")));
        assert!(m(t(">=5"), n(5.0)));
        assert!(m(t("<5"), n(1.0)));
        assert!(m(t("<=5"), n(5.0)));
        assert!(m(t("<>5"), n(1.0)));
        assert!(!m(t("<>5"), n(5.0)));
        assert!(!m(t("<>5"), t("5")));
        assert!(m(t("<>5"), ExcelValue::Empty));
        assert!(!m(t("<>5"), ExcelValue::Error(ExcelError::Na)));
    }

    #[test]
    fn blanks() {
        assert!(m(t(""), ExcelValue::Empty));
        assert!(m(t(""), t("")));
        assert!(m(t("="), ExcelValue::Empty));
        assert!(!m(t(""), n(0.0)));
        assert!(!m(t("<>"), ExcelValue::Empty));
        assert!(!m(t("<>"), t("")));
        assert!(m(t("<>"), n(1.0)));
        assert!(m(t("<>"), t("x")));
        assert!(!m(t("<>"), ExcelValue::Error(ExcelError::Div0)));
    }

    #[test]
    fn bools_and_true_text() {
        assert!(m(b(true), b(true)));
        assert!(!m(b(true), n(1.0)));
        assert!(!m(b(true), t("TRUE")));
        assert!(m(t("TRUE"), b(true)));
        assert!(!m(t("TRUE"), t("TRUE")));
        assert!(m(t("TRUE*"), t("TRUE")));
        assert!(!m(t("TRUE*"), b(true)));
        assert!(m(t("FALSE"), b(false)));
    }

    #[test]
    fn wildcards_and_casefold() {
        assert!(m(t("a*"), t("APPLE")));
        assert!(m(t("c?t"), t("Cat")));
        assert!(!m(t("c?t"), t("coat")));
        assert!(m(t("*"), t("")));
        assert!(m(t("*"), t("x")));
        assert!(!m(t("*"), n(1.0)));
        assert!(m(t("~*"), t("*")));
        assert!(!m(t("~*"), t("star")));
        assert!(m(t("apple"), t("APPLE")));
    }

    #[test]
    fn text_inequality() {
        assert!(m(t(">b"), t("Banana")));
        assert!(!m(t(">b"), t("apple")));
        assert!(!m(t(">b"), n(100.0)));
    }

    #[test]
    fn errors() {
        assert!(m(
            ExcelValue::Error(ExcelError::Na),
            ExcelValue::Error(ExcelError::Na)
        ));
        assert!(m(t("#N/A"), ExcelValue::Error(ExcelError::Na)));
        assert!(!m(t("#N/A"), n(1.0)));
        assert!(!m(t(">0"), ExcelValue::Error(ExcelError::Div0)));
    }

    #[test]
    fn count_flattens_arrays() {
        let arr = ExcelValue::Array(vec![vec![n(1.0), n(2.0), n(2.0)]]);
        assert_eq!(count_matches(&arr, &Criterion::parse(&n(2.0))), 2);
    }
}
