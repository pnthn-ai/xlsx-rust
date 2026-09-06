//! Shared Excel criteria for `SUMIF` / `COUNTIF` / `SUMIFS` / `COUNTIFS` / `AVERAGEIF` / `AVERAGEIFS`.
//!
//! Two constructors preserve function-family semantics:
//! - [`Criterion::compile`] — SUMIF-family: error criteria propagate, number
//!   literals are type-strict, `"TRUE"` is text.
//! - [`Criterion::parse`] — COUNTIF-family: number literals match numeric text,
//!   `"TRUE"` is logical, error criteria match error cells.
//!
//! The matcher never reads fixture expected values.

use crate::error::ExcelError;
use crate::value::ExcelValue;


mod sumif_style {
//! Excel `SUMIF` / `COUNTIF`-style criteria.
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

/// Compiled Excel criteria used by `SUMIF` (and later `COUNTIF` / `SUMIFS`).
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

    /// Whether `cell` satisfies this criteria (Excel SUMIF/COUNTIF rules).
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

}

mod countif_style {
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

}

/// Compiled Excel criteria used by SUMIF / COUNTIF / SUMIFS / COUNTIFS / AVERAGEIF / AVERAGEIFS.
#[derive(Clone, Debug)]
pub struct Criterion {
    inner: Inner,
}

#[derive(Clone, Debug)]
enum Inner {
    SumIf(sumif_style::Criterion),
    CountIf(countif_style::Criterion),
}

impl Criterion {
    /// SUMIF / SUMIFS / AVERAGEIF / AVERAGEIFS constructor.
    pub fn compile(v: &ExcelValue) -> Result<Self, ExcelError> {
        Ok(Self {
            inner: Inner::SumIf(sumif_style::Criterion::compile(v)?),
        })
    }

    /// COUNTIF / COUNTIFS constructor.
    pub fn parse(v: &ExcelValue) -> Self {
        Self {
            inner: Inner::CountIf(countif_style::Criterion::parse(v)),
        }
    }

    /// Whether `cell` satisfies this criteria.
    pub fn matches(&self, cell: &ExcelValue) -> bool {
        match &self.inner {
            Inner::SumIf(c) => c.matches(cell),
            Inner::CountIf(c) => c.matches(cell),
        }
    }
}

pub use countif_style::excel_wildcard;
pub use sumif_style::looks_like_wildcard_pat;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn n(v: f64) -> ExcelValue {
        ExcelValue::Number(v)
    }
    fn t(s: &str) -> ExcelValue {
        ExcelValue::Text(s.into())
    }
    fn b(v: bool) -> ExcelValue {
        ExcelValue::Bool(v)
    }

    fn matches_crit(crit: ExcelValue, cell: ExcelValue) -> bool {
        Criterion::compile(&crit).unwrap().matches(&cell)
    }

    fn m(crit: ExcelValue, cell: ExcelValue) -> bool {
        Criterion::parse(&crit).matches(&cell)
    }

    #[test]
    fn sumif_number_criteria_is_type_strict() {
        assert!(matches_crit(n(5.0), n(5.0)));
        assert!(!matches_crit(n(5.0), t("5")));
        assert!(!matches_crit(n(0.0), ExcelValue::Empty));
        assert!(!matches_crit(n(1.0), ExcelValue::Bool(true)));
    }

    #[test]
    fn countif_number_matches_numeric_text() {
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
    fn sumif_text_numeric_criteria_dual_matches() {
        assert!(matches_crit(t("5"), n(5.0)));
        assert!(matches_crit(t("5"), t("5")));
        assert!(matches_crit(t("=5"), n(5.0)));
        assert!(matches_crit(t("=5"), t("5")));
        assert!(!matches_crit(t("5"), ExcelValue::Empty));
    }

    #[test]
    fn countif_true_text_is_logical() {
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
    fn sumif_true_text_is_text() {
        assert!(matches_crit(ExcelValue::Bool(true), ExcelValue::Bool(true)));
        assert!(!matches_crit(ExcelValue::Bool(true), n(1.0)));
        assert!(!matches_crit(t("TRUE"), ExcelValue::Bool(true)));
        assert!(Criterion::compile(&ExcelValue::Error(ExcelError::Div0)).is_err());
    }

    #[test]
    fn blanks_and_wildcards() {
        assert!(matches_crit(t(""), ExcelValue::Empty));
        assert!(matches_crit(t("="), ExcelValue::Empty));
        assert!(m(t(""), ExcelValue::Empty));
        assert!(m(t("<>"), n(1.0)));
        assert!(matches_crit(t("*a*"), t("cat")));
        assert!(m(t("a*"), t("APPLE")));
        assert!(looks_like_wildcard_pat("*a*"));
        assert!(excel_wildcard("a?", "ab"));
    }

    #[test]
    fn count_flattens_arrays() {
        let arr = ExcelValue::Array(vec![vec![n(1.0), n(2.0), n(2.0)]]);
        assert_eq!(count_matches(&arr, &Criterion::parse(&n(2.0))), 2);
    }
}
