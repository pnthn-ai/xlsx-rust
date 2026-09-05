//! Workbook-backed formula evaluator.
//!
//! Operators call into [`coerce`] / [`compare`] / [`empty`] for Excel quirks.
//! Worksheet functions live in [`functions`].

pub mod coerce;
pub mod compare;
pub mod empty;
pub mod functions;

use crate::ast::{BinOp, Expr, UnaryOp};
use crate::parse::parse;
use std::collections::HashSet;
use xlsx_types::{
    CellRef, EvalError, EvalSpec, EvalTarget, ExcelError, ExcelValue, RangeRef, Workbook,
};

pub struct Evaluator;

pub(crate) struct Ctx<'a> {
    spec: &'a EvalSpec,
    current_sheet: String,
    depth: usize,
    visiting: HashSet<String>,
}

impl Evaluator {
    pub fn new() -> Self {
        Self
    }

    pub fn eval_spec(&self, spec: &EvalSpec) -> Result<ExcelValue, EvalError> {
        let current_sheet = spec
            .default_cell()
            .sheet
            .clone()
            .unwrap_or_else(|| spec.workbook.default_sheet_name().to_string());
        let mut ctx = Ctx {
            spec,
            current_sheet,
            depth: 0,
            visiting: HashSet::new(),
        };
        match &spec.target {
            EvalTarget::Formula { formula, at } => {
                if let Some(at) = at {
                    if let Some(sheet) = &at.sheet {
                        ctx.current_sheet = sheet.clone();
                    }
                }
                self.eval_formula(formula, &mut ctx)
            }
            EvalTarget::Cell { cell } => self.eval_cell(cell, &mut ctx),
            EvalTarget::Named { name } => self.eval_named(name, &mut ctx),
        }
    }

    fn eval_formula(&self, formula: &str, ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        let ast = parse(formula)?;
        self.eval_expr(&ast, ctx)
    }

    pub(crate) fn eval_expr(
        &self,
        expr: &Expr,
        ctx: &mut Ctx<'_>,
    ) -> Result<ExcelValue, EvalError> {
        if ctx.depth > 64 {
            return Ok(ExcelValue::Error(ExcelError::Circular));
        }
        ctx.depth += 1;
        let out = match expr {
            Expr::Number(n) => Ok(ExcelValue::Number(*n)),
            Expr::Text(s) => Ok(ExcelValue::Text(s.clone())),
            Expr::Bool(b) => Ok(ExcelValue::Bool(*b)),
            Expr::Error(e) => Ok(ExcelValue::Error(*e)),
            Expr::Cell(r) => self.eval_cell(r, ctx),
            Expr::Range(r) => self.eval_range(r, ctx),
            Expr::Name(n) => self.eval_named(n, ctx),
            Expr::Unary { op, expr } => self.eval_unary(*op, expr, ctx),
            Expr::Binary { op, left, right } => self.eval_binary(*op, left, right, ctx),
            Expr::Call { name, args } => functions::dispatch(self, name, args, ctx),
            Expr::Array(rows) => {
                let mut out = Vec::new();
                for row in rows {
                    let mut r = Vec::new();
                    for c in row {
                        r.push(self.eval_expr(c, ctx)?);
                    }
                    out.push(r);
                }
                Ok(ExcelValue::Array(out))
            }
        };
        ctx.depth -= 1;
        out
    }

    fn eval_cell(&self, cell: &CellRef, ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        let sheet_name = cell
            .sheet
            .clone()
            .unwrap_or_else(|| ctx.current_sheet.clone());
        let key = format!("{}!{}", sheet_name, cell.addr.a1());
        if !ctx.visiting.insert(key.clone()) {
            return Ok(ExcelValue::Error(ExcelError::Circular));
        }
        let sheet = match ctx.spec.workbook.sheet(Some(&sheet_name)) {
            Ok(s) => s,
            Err(_) => {
                ctx.visiting.remove(&key);
                return Ok(ExcelValue::Error(ExcelError::Ref));
            }
        };
        let stored = sheet.get(cell.addr).cloned();
        let result = if let Some(c) = stored {
            if let Some(formula) = c.formula {
                let prev = ctx.current_sheet.clone();
                ctx.current_sheet = sheet_name;
                let v = self.eval_formula(&formula, ctx)?;
                ctx.current_sheet = prev;
                Ok(v)
            } else {
                Ok(c.value.unwrap_or(ExcelValue::Empty))
            }
        } else {
            Ok(ExcelValue::Empty)
        };
        ctx.visiting.remove(&key);
        result
    }

    fn eval_range(&self, range: &RangeRef, ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        let sheet = range
            .sheet
            .clone()
            .unwrap_or_else(|| ctx.current_sheet.clone());
        let mut rows: Vec<Vec<ExcelValue>> = Vec::new();
        let mut row: Vec<ExcelValue> = Vec::new();
        let mut last = range.start.row;
        for addr in range.cells() {
            if addr.row != last {
                rows.push(std::mem::take(&mut row));
                last = addr.row;
            }
            row.push(self.eval_cell(
                &CellRef {
                    sheet: Some(sheet.clone()),
                    addr,
                },
                ctx,
            )?);
        }
        rows.push(row);
        Ok(ExcelValue::Array(rows))
    }

    fn eval_named(&self, name: &str, ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        let def = match ctx.spec.workbook.defined_name(name) {
            Ok(d) => d,
            Err(_) => return Ok(ExcelValue::Error(ExcelError::Name)),
        };
        let refers = def.refers_to.trim();
        if refers.starts_with('=') {
            return self.eval_formula(refers, ctx);
        }
        if let Ok(range) = RangeRef::parse(refers) {
            return self.eval_range(&range, ctx);
        }
        if let Ok(cell) = CellRef::parse(refers) {
            return self.eval_cell(&cell, ctx);
        }
        self.eval_formula(refers, ctx)
    }

    fn eval_unary(
        &self,
        op: UnaryOp,
        expr: &Expr,
        ctx: &mut Ctx<'_>,
    ) -> Result<ExcelValue, EvalError> {
        let v = coerce::scalarize(self.eval_expr(expr, ctx)?);
        if let ExcelValue::Error(e) = v {
            return Ok(ExcelValue::Error(e));
        }
        match op {
            UnaryOp::Plus => match coerce::to_number(&v) {
                Ok(n) => Ok(ExcelValue::Number(n)),
                Err(e) => Ok(ExcelValue::Error(e)),
            },
            UnaryOp::Minus => match coerce::to_number(&v) {
                Ok(n) => Ok(ExcelValue::Number(-n)),
                Err(e) => Ok(ExcelValue::Error(e)),
            },
            UnaryOp::Percent => match coerce::to_number(&v) {
                Ok(n) => Ok(ExcelValue::Number(n / 100.0)),
                Err(e) => Ok(ExcelValue::Error(e)),
            },
        }
    }

    fn eval_binary(
        &self,
        op: BinOp,
        left: &Expr,
        right: &Expr,
        ctx: &mut Ctx<'_>,
    ) -> Result<ExcelValue, EvalError> {
        let l = coerce::scalarize(self.eval_expr(left, ctx)?);
        let r = coerce::scalarize(self.eval_expr(right, ctx)?);
        if let ExcelValue::Error(e) = l {
            return Ok(ExcelValue::Error(e));
        }
        if let ExcelValue::Error(e) = r {
            return Ok(ExcelValue::Error(e));
        }
        Ok(match op {
            BinOp::Add => arith(&l, &r, |a, b| a + b),
            BinOp::Sub => arith(&l, &r, |a, b| a - b),
            BinOp::Mul => arith(&l, &r, |a, b| a * b),
            BinOp::Div => div(&l, &r),
            BinOp::Pow => arith(&l, &r, |a, b| a.powf(b)),
            BinOp::Concat => concat(&l, &r),
            BinOp::Eq => ExcelValue::Bool(compare::equal(&l, &r)),
            BinOp::Ne => ExcelValue::Bool(!compare::equal(&l, &r)),
            BinOp::Lt => {
                ExcelValue::Bool(compare::ordered(&l, &r, std::cmp::Ordering::Less, false))
            }
            BinOp::Gt => {
                ExcelValue::Bool(compare::ordered(&l, &r, std::cmp::Ordering::Greater, false))
            }
            BinOp::Le => {
                ExcelValue::Bool(compare::ordered(&l, &r, std::cmp::Ordering::Greater, true))
            }
            BinOp::Ge => ExcelValue::Bool(compare::ordered(&l, &r, std::cmp::Ordering::Less, true)),
        })
    }
}

fn arith(l: &ExcelValue, r: &ExcelValue, f: impl Fn(f64, f64) -> f64) -> ExcelValue {
    match (coerce::to_number(l), coerce::to_number(r)) {
        (Ok(a), Ok(b)) => ExcelValue::Number(f(a, b)),
        (Err(e), _) | (_, Err(e)) => ExcelValue::Error(e),
    }
}

fn div(l: &ExcelValue, r: &ExcelValue) -> ExcelValue {
    match (coerce::to_number(l), coerce::to_number(r)) {
        (Ok(_), Ok(b)) if b == 0.0 => ExcelValue::Error(ExcelError::Div0),
        (Ok(a), Ok(b)) => ExcelValue::Number(a / b),
        (Err(e), _) | (_, Err(e)) => ExcelValue::Error(e),
    }
}

fn concat(l: &ExcelValue, r: &ExcelValue) -> ExcelValue {
    match (coerce::to_text(l), coerce::to_text(r)) {
        (Ok(a), Ok(b)) => ExcelValue::Text(format!("{a}{b}")),
        (Err(e), _) | (_, Err(e)) => ExcelValue::Error(e),
    }
}

/// Ad-hoc evaluation helper for unit tests (no Candidate trait required).
pub fn eval_formula_in(workbook: &Workbook, formula: &str) -> Result<ExcelValue, EvalError> {
    let spec = EvalSpec {
        case_id: "adhoc".into(),
        workbook: workbook.clone(),
        target: EvalTarget::formula(formula),
        options: Default::default(),
    };
    Evaluator::new().eval_spec(&spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn div0_is_excel_error() {
        let e = eval_formula_in(&Workbook::default(), "=1/0").unwrap();
        assert_eq!(e, ExcelValue::Error(ExcelError::Div0));
        let z = eval_formula_in(&Workbook::default(), "=0/0").unwrap();
        assert_eq!(z, ExcelValue::Error(ExcelError::Div0));
    }

    #[test]
    fn text_plus_and_true_plus() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=\"2\"+1").unwrap(),
            ExcelValue::Number(3.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=TRUE+1").unwrap(),
            ExcelValue::Number(2.0)
        );
    }

    #[test]
    fn fuzzy_eq_and_no_eq_coercion() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=0.1+0.2=0.3").unwrap(),
            ExcelValue::Bool(true)
        );
        assert_eq!(
            eval_formula_in(&wb, "=\"2\"=2").unwrap(),
            ExcelValue::Bool(false)
        );
    }

    #[test]
    fn if_short_circuit() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=IF(TRUE, 1, 1/0)").unwrap(),
            ExcelValue::Number(1.0)
        );
    }

    #[test]
    fn unknown_function_is_name() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=NOTAFUNCTION(1)").unwrap(),
            ExcelValue::Error(ExcelError::Name)
        );
    }

    #[test]
    fn percent_and_unary() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=50%").unwrap(),
            ExcelValue::Number(0.5)
        );
        assert_eq!(
            eval_formula_in(&wb, "=-5+2").unwrap(),
            ExcelValue::Number(-3.0)
        );
    }
}
