//! Formula AST. The parser produces these nodes; the evaluator walks them.

use xlsx_types::{CellRef, ExcelError, RangeRef};

/// A parsed Excel formula expression.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Number(f64),
    Text(String),
    Bool(bool),
    Error(ExcelError),
    Cell(CellRef),
    Range(RangeRef),
    Name(String),
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Call {
        name: String,
        args: Vec<Expr>,
    },
    /// Row-major array literal (`{1,2;3,4}`).
    Array(Vec<Vec<Expr>>),
    /// Omitted function argument (`TEXTSPLIT(text,,row)`).
    ///
    /// Evaluates to [`xlsx_types::ExcelValue::Empty`] unless a function
    /// special-cases it (TEXTSPLIT treats a missing delimiter as “no split
    /// on that axis”, which is not the same as an empty-string delimiter).
    Missing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Plus,
    Minus,
    Percent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Concat,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    /// Space (intersect) operator: `A1:B2 B2`.
    Intersect,
}

impl Expr {
    /// Cell, range, or defined-name reference. `SUM` treats these as
    /// range-like (skip text / logicals) rather than coercing them as scalars.
    pub fn is_reference(&self) -> bool {
        matches!(self, Expr::Cell(_) | Expr::Range(_) | Expr::Name(_))
    }

    /// `TRUE` for a skipped call argument (`FOO(a,,b)` → the middle slot).
    pub fn is_omitted(&self) -> bool {
        matches!(self, Expr::Missing)
    }
}
