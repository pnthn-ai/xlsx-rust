//! Workbook-backed formula evaluator.
//!
//! Operators call into [`coerce`] / [`compare`] / [`empty`] for Excel quirks.
//! Worksheet functions live in [`functions`].

pub mod averageif;
pub mod coerce;
pub mod compare;
pub mod concat;
pub mod empty;
pub mod find;
pub mod functions;
pub mod substitute;
pub mod sumif;
pub mod sumproduct;
pub mod replace;
pub mod sumifs;
pub mod textjoin;
pub mod round;
pub mod search;

use crate::ast::{BinOp, Expr, UnaryOp};
use crate::parse::parse;
use std::collections::HashSet;
use xlsx_types::{
    count_matches, ArrayMode, CellAddr, CellRef, Criterion, EvalError, EvalSpec, EvalTarget,
    ExcelError, ExcelValue, RangeRef, Workbook,
};

pub struct Evaluator;

pub(crate) struct Ctx<'a> {
    pub(crate) spec: &'a EvalSpec,
    pub(crate) current_sheet: String,
    depth: usize,
    visiting: HashSet<String>,
    host: CellAddr,
}

impl Evaluator {
    pub fn new() -> Self {
        Self
    }

    pub fn eval_spec(&self, spec: &EvalSpec) -> Result<ExcelValue, EvalError> {
        let mut ctx = self.ctx_from_spec(spec);
        match &spec.target {
            EvalTarget::Formula { formula, at } => {
                if let Some(at) = at {
                    if let Some(sheet) = &at.sheet {
                        ctx.current_sheet = sheet.clone();
                    }
                    ctx.host = at.addr;
                }
                self.eval_formula(formula, &mut ctx)
            }
            EvalTarget::Cell { cell } => {
                ctx.host = cell.addr;
                self.eval_cell(cell, &mut ctx)
            }
            EvalTarget::Named { name } => self.eval_named(name, &mut ctx),
        }
    }

    fn ctx_from_spec<'a>(&self, spec: &'a EvalSpec) -> Ctx<'a> {
        let current_sheet = spec
            .default_cell()
            .sheet
            .clone()
            .unwrap_or_else(|| spec.workbook.default_sheet_name().to_string());
        Ctx {
            spec,
            current_sheet,
            depth: 0,
            visiting: HashSet::new(),
            host: spec.default_cell().addr,
        }
    }

    /// COUNTIF that materializes the range as an array first (bench baseline).
    pub fn countif_materialized(&self, spec: &EvalSpec) -> Result<ExcelValue, EvalError> {
        let EvalTarget::Formula { formula, .. } = &spec.target else {
            return Err(EvalError::Other(
                "countif_materialized expects a formula target".into(),
            ));
        };
        let ast = parse(formula)?;
        let crate::ast::Expr::Call { name, args } = ast else {
            return Err(EvalError::Other("expected COUNTIF(...)".into()));
        };
        if !name.eq_ignore_ascii_case("COUNTIF") || args.len() != 2 {
            return Err(EvalError::Other("expected COUNTIF(range, criteria)".into()));
        }
        let mut ctx = self.ctx_from_spec(spec);
        let crit = Criterion::parse(&self.eval_scalar(&args[1], &mut ctx)?);
        let v = self.eval_expr(&args[0], &mut ctx)?;
        Ok(ExcelValue::Number(count_matches(&v, &crit) as f64))
    }

    fn eval_formula(&self, formula: &str, ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        let ast = parse(formula)?;
        if matches!(ctx.spec.options.array_mode, ArrayMode::Scalar) {
            if let Expr::Range(r) = &ast {
                return self.implicit_intersect_range(r, ctx);
            }
        }
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

    pub(crate) fn countif_range(
        &self,
        range: &RangeRef,
        ctx: &mut Ctx<'_>,
        crit: &Criterion,
    ) -> Result<ExcelValue, EvalError> {
        let sheet_name = range
            .sheet
            .clone()
            .unwrap_or_else(|| ctx.current_sheet.clone());
        if ctx.spec.workbook.sheet(Some(&sheet_name)).is_err() {
            return Ok(ExcelValue::Error(ExcelError::Ref));
        }
        // Hold the sheet while counting stored values so we do one lookup, no
        // per-cell clone, and a reused A1 buffer. Formula cells are deferred
        // so we can drop the sheet borrow before `eval_cell`.
        let mut formula_addrs = Vec::new();
        let mut count = 0u64;
        let mut a1 = String::with_capacity(8);
        {
            let sheet = ctx.spec.workbook.sheet(Some(&sheet_name)).unwrap();
            for addr in range.cells() {
                a1.clear();
                addr.write_a1(&mut a1);
                match sheet.cells.get(&a1) {
                    None => {
                        if crit.matches(&ExcelValue::Empty) {
                            count += 1;
                        }
                    }
                    Some(c) if c.formula.is_some() => formula_addrs.push(addr),
                    Some(c) => {
                        let v = c.value.as_ref().unwrap_or(&ExcelValue::Empty);
                        if crit.matches(v) {
                            count += 1;
                        }
                    }
                }
            }
        }
        for addr in formula_addrs {
            let v = self.eval_cell(
                &CellRef {
                    sheet: Some(sheet_name.clone()),
                    addr,
                },
                ctx,
            )?;
            if crit.matches(&v) {
                count += 1;
            }
        }
        Ok(ExcelValue::Number(count as f64))
    }
    pub(crate) fn eval_cell(
        &self,
        cell: &CellRef,
        ctx: &mut Ctx<'_>,
    ) -> Result<ExcelValue, EvalError> {
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
                let prev_sheet = ctx.current_sheet.clone();
                let prev_host = ctx.host;
                ctx.current_sheet = sheet_name;
                ctx.host = cell.addr;
                let v = self.eval_formula(&formula, ctx)?;
                ctx.current_sheet = prev_sheet;
                ctx.host = prev_host;
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

    pub(crate) fn eval_range(
        &self,
        range: &RangeRef,
        ctx: &mut Ctx<'_>,
    ) -> Result<ExcelValue, EvalError> {
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

    pub(crate) fn eval_named(&self, name: &str, ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
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
        let v = self.eval_scalar(expr, ctx)?;
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
        if op == BinOp::Intersect {
            return self.eval_intersect(left, right, ctx);
        }
        let l = self.eval_scalar(left, ctx)?;
        let r = self.eval_scalar(right, ctx)?;
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
            BinOp::Pow => excel_pow(&l, &r),
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
            BinOp::Intersect => unreachable!("intersect handled above"),
        })
    }

    pub(crate) fn eval_scalar(
        &self,
        expr: &Expr,
        ctx: &mut Ctx<'_>,
    ) -> Result<ExcelValue, EvalError> {
        match expr {
            Expr::Range(r) => {
                if matches!(ctx.spec.options.array_mode, ArrayMode::DynamicArray) {
                    Ok(coerce::scalarize(self.eval_range(r, ctx)?))
                } else {
                    self.implicit_intersect_range(r, ctx)
                }
            }
            other => Ok(coerce::scalarize(self.eval_expr(other, ctx)?)),
        }
    }

    fn implicit_intersect_range(
        &self,
        range: &RangeRef,
        ctx: &mut Ctx<'_>,
    ) -> Result<ExcelValue, EvalError> {
        let host = ctx.host;
        let sc = range.start.col;
        let ec = range.end.col;
        let sr = range.start.row;
        let er = range.end.row;
        let picked = if sc == ec {
            if host.row >= sr && host.row <= er {
                Some(CellAddr::new(sc, host.row))
            } else {
                None
            }
        } else if sr == er {
            if host.col >= sc && host.col <= ec {
                Some(CellAddr::new(host.col, sr))
            } else {
                None
            }
        } else if host.col >= sc && host.col <= ec && host.row >= sr && host.row <= er {
            Some(CellAddr::new(host.col, host.row))
        } else {
            None
        };
        match picked {
            Some(addr) => self.eval_cell(
                &CellRef {
                    sheet: range.sheet.clone(),
                    addr,
                },
                ctx,
            ),
            None => Ok(ExcelValue::Error(ExcelError::Value)),
        }
    }

    pub(crate) fn eval_intersect(
        &self,
        left: &Expr,
        right: &Expr,
        ctx: &mut Ctx<'_>,
    ) -> Result<ExcelValue, EvalError> {
        match intersect_exprs(left, right) {
            Ok(range) => {
                if range.start == range.end {
                    self.eval_cell(
                        &CellRef {
                            sheet: range.sheet,
                            addr: range.start,
                        },
                        ctx,
                    )
                } else {
                    self.eval_range(&range, ctx)
                }
            }
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }
}

fn arith(l: &ExcelValue, r: &ExcelValue, f: impl Fn(f64, f64) -> f64) -> ExcelValue {
    match (coerce::to_number(l), coerce::to_number(r)) {
        (Ok(a), Ok(b)) => ExcelValue::Number(f(a, b)),
        (Err(e), _) | (_, Err(e)) => ExcelValue::Error(e),
    }
}

pub(crate) fn div(l: &ExcelValue, r: &ExcelValue) -> ExcelValue {
    match (coerce::to_number(l), coerce::to_number(r)) {
        (Ok(_), Ok(b)) if b == 0.0 => ExcelValue::Error(ExcelError::Div0),
        (Ok(a), Ok(b)) => ExcelValue::Number(a / b),
        (Err(e), _) | (_, Err(e)) => ExcelValue::Error(e),
    }
}

pub(crate) fn concat(l: &ExcelValue, r: &ExcelValue) -> ExcelValue {
    match (coerce::to_text(l), coerce::to_text(r)) {
        (Ok(a), Ok(b)) => ExcelValue::Text(format!("{a}{b}")),
        (Err(e), _) | (_, Err(e)) => ExcelValue::Error(e),
    }
}

/// Excel `^` / `POWER`: `0^0` and negative^non-integer are `#NUM!`.
pub(crate) fn excel_pow(l: &ExcelValue, r: &ExcelValue) -> ExcelValue {
    match (coerce::to_number(l), coerce::to_number(r)) {
        (Ok(a), Ok(b)) => {
            if a == 0.0 && b == 0.0 {
                return ExcelValue::Error(ExcelError::Num);
            }
            if a < 0.0 && b.fract() != 0.0 {
                return ExcelValue::Error(ExcelError::Num);
            }
            let n = a.powf(b);
            if !n.is_finite() {
                return ExcelValue::Error(ExcelError::Num);
            }
            ExcelValue::Number(n)
        }
        (Err(e), _) | (_, Err(e)) => ExcelValue::Error(e),
    }
}

fn intersect_exprs(left: &Expr, right: &Expr) -> Result<RangeRef, ExcelError> {
    let a = expr_as_range(left)?;
    let b = expr_as_range(right)?;
    intersect_ranges(&a, &b).ok_or(ExcelError::Null)
}

fn expr_as_range(expr: &Expr) -> Result<RangeRef, ExcelError> {
    match expr {
        Expr::Cell(c) => Ok(RangeRef::new(c.sheet.clone(), c.addr, c.addr)),
        Expr::Range(r) => Ok(r.clone()),
        Expr::Binary {
            op: BinOp::Intersect,
            left,
            right,
        } => intersect_exprs(left, right),
        _ => Err(ExcelError::Value),
    }
}

fn intersect_ranges(a: &RangeRef, b: &RangeRef) -> Option<RangeRef> {
    let sheet = match (&a.sheet, &b.sheet) {
        (Some(x), Some(y)) if !x.eq_ignore_ascii_case(y) => return None,
        (Some(x), _) => Some(x.clone()),
        (_, Some(y)) => Some(y.clone()),
        _ => None,
    };
    let c1 = a.start.col.max(b.start.col);
    let r1 = a.start.row.max(b.start.row);
    let c2 = a.end.col.min(b.end.col);
    let r2 = a.end.row.min(b.end.row);
    if c1 > c2 || r1 > r2 {
        return None;
    }
    Some(RangeRef::new(
        sheet,
        CellAddr::new(c1, r1),
        CellAddr::new(c2, r2),
    ))
}

/// Evaluate a `SUMIF(...)` formula with the materializing implementation.
/// Used only by the Criterion microbench as the pre-hill-climb baseline.
pub fn eval_sumif_materialized(
/// Evaluate a `SUMIFS(...)` formula with the materializing implementation.
/// Used only by the Criterion microbench as the pre-hill-climb baseline.
pub fn eval_sumifs_materialized(
/// Evaluate an `AVERAGEIF(...)` formula with the materializing implementation.
/// Used only by the Criterion microbench as the pre-hill-climb baseline.
pub fn eval_averageif_materialized(
    workbook: &Workbook,
    formula: &str,
) -> Result<ExcelValue, EvalError> {
    let spec = EvalSpec {
        case_id: "sumif-bench".into(),
        case_id: "sumifs-bench".into(),
        case_id: "averageif-bench".into(),
        workbook: workbook.clone(),
        target: EvalTarget::formula(formula),
        options: Default::default(),
    };
    let ast = parse(formula)?;
    let current_sheet = spec.workbook.default_sheet_name().to_string();
    let mut ctx = Ctx {
        spec: &spec,
        current_sheet,
        depth: 0,
        visiting: HashSet::new(),
        host: spec.default_cell().addr,
    };
    match ast {
        Expr::Call { name, args } if name.eq_ignore_ascii_case("SUMIF") => {
            sumif::sumif_materialized(&Evaluator, &args, &mut ctx)
        }
        _ => Err(EvalError::Other("expected SUMIF call".into())),
        Expr::Call { name, args } if name.eq_ignore_ascii_case("SUMIFS") => {
            sumifs::sumifs_materialized(&Evaluator, &args, &mut ctx)
        }
        _ => Err(EvalError::Other("expected SUMIFS call".into())),
        Expr::Call { name, args } if name.eq_ignore_ascii_case("AVERAGEIF") => {
            averageif::averageif_materialized(&Evaluator, &args, &mut ctx)
        }
        _ => Err(EvalError::Other("expected AVERAGEIF call".into())),
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

    #[test]
    fn pow_zero_zero_is_num() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=0^0").unwrap(),
            ExcelValue::Error(ExcelError::Num)
        );
    }

    #[test]
    fn sumif_gt_and_reshape() {
    fn sumifs_and_same_shape_and_no_reshape() {
    fn averageif_gt_reshape_and_div0() {
        let mut wb = Workbook::default();
        wb.set_value("Sheet1", "A1", ExcelValue::Number(1.0))
            .unwrap();
        wb.set_value("Sheet1", "A2", ExcelValue::Number(6.0))
            .unwrap();
        wb.set_value("Sheet1", "A3", ExcelValue::Number(3.0))
            .unwrap();
        wb.set_value("Sheet1", "B1", ExcelValue::Number(10.0))
            .unwrap();
        wb.set_value("Sheet1", "B2", ExcelValue::Number(20.0))
            .unwrap();
        wb.set_value("Sheet1", "B3", ExcelValue::Number(30.0))
            .unwrap();
        assert_eq!(
            eval_formula_in(&wb, "=SUMIF(A1:A3,\">2\")").unwrap(),
            ExcelValue::Number(9.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=SUMIF(A1:A3,\">2\",B1)").unwrap(),
            ExcelValue::Number(50.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=SUMIF({1,2,3},\">1\")").unwrap(),
    fn sumproduct_arrays_and_bool_coercion() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=SUMPRODUCT({1,2,3},{4,5,6})").unwrap(),
            ExcelValue::Number(32.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=SUMPRODUCT({TRUE,FALSE,TRUE})").unwrap(),
            ExcelValue::Number(0.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=SUMPRODUCT({--TRUE,--FALSE,--TRUE})").unwrap(),
            ExcelValue::Number(2.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=SUMPRODUCT({1,2},{1,2,3})").unwrap(),
    fn text_number_and_date_subset() {
        let wb = Workbook::default();
        assert_eq!(
            eval_formula_in(&wb, "=TEXT(1234.567,\"0.00\")").unwrap(),
            ExcelValue::Text("1234.57".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=TEXT(1234,\"#,##0\")").unwrap(),
            ExcelValue::Text("1,234".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=TEXT(0.285,\"0.0%\")").unwrap(),
            ExcelValue::Text("28.5%".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=TEXT(DATE(2024,3,15),\"yyyy-mm-dd\")").unwrap(),
            ExcelValue::Text("2024-03-15".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=TEXT(\"abc\",\"0.00\")").unwrap(),
            ExcelValue::Text("abc".into())
        );
        assert_eq!(
            eval_formula_in(&wb, "=TEXT(#DIV/0!,\"0\")").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=TEXT(1)").unwrap(),
        wb.set_value("Sheet1", "B1", ExcelValue::Text("x".into()))
            .unwrap();
        wb.set_value("Sheet1", "B2", ExcelValue::Text("x".into()))
            .unwrap();
        wb.set_value("Sheet1", "B3", ExcelValue::Text("y".into()))
            .unwrap();
        wb.set_value("Sheet1", "C1", ExcelValue::Number(10.0))
            .unwrap();
        wb.set_value("Sheet1", "C2", ExcelValue::Number(20.0))
            .unwrap();
        wb.set_value("Sheet1", "C3", ExcelValue::Number(30.0))
            .unwrap();
        assert_eq!(
            eval_formula_in(&wb, "=SUMIFS(C1:C3,A1:A3,\">2\",B1:B3,\"x\")").unwrap(),
            ExcelValue::Number(20.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=SUMIFS(C1,A1:A3,\">2\")").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            eval_formula_in(&wb, "=SUMIFS({1,2,3},A1:A3,\">1\")").unwrap(),
            eval_formula_in(&wb, "=AVERAGEIF(A1:A3,\">2\")").unwrap(),
            ExcelValue::Number(4.5)
        );
        assert_eq!(
            eval_formula_in(&wb, "=AVERAGEIF(A1:A3,\">2\",B1)").unwrap(),
            ExcelValue::Number(25.0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=AVERAGEIF(A1:A3,\">100\")").unwrap(),
            ExcelValue::Error(ExcelError::Div0)
        );
        assert_eq!(
            eval_formula_in(&wb, "=AVERAGEIF({1,2,3},\">1\")").unwrap(),
            ExcelValue::Error(ExcelError::Value)
        );
    }
}
