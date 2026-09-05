//! Seed-scoped evaluator with Excel-like and intentionally naive semantics.

use crate::parse::{parse, BinOp, Expr, UnaryOp};
use std::collections::HashSet;
use xlsx_types::{
    excel_num_eq, CellRef, EvalError, EvalSpec, EvalTarget, ExcelError, ExcelValue, RangeRef,
    Workbook,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Semantics {
    /// Excel-compatible coercion / errors for the seed corpus.
    ExcelSeed,
    /// IEEE / Rust-like behavior that *should* fail several quirk fixtures.
    Naive,
}

pub struct Interpreter {
    pub semantics: Semantics,
}

struct Ctx<'a> {
    spec: &'a EvalSpec,
    current_sheet: String,
    depth: usize,
    visiting: HashSet<String>,
}

impl Interpreter {
    pub fn new(semantics: Semantics) -> Self {
        Self { semantics }
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

    fn eval_expr(&self, expr: &Expr, ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
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
            Expr::Call { name, args } => self.eval_call(name, args, ctx),
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
        let sheet = ctx
            .spec
            .workbook
            .sheet(Some(&sheet_name))
            .map_err(|e| EvalError::Workbook(e.to_string()))?;
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
        let def = ctx
            .spec
            .workbook
            .defined_name(name)
            .map_err(|e| EvalError::Workbook(e.to_string()))?;
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
        let v = self.scalarize(self.eval_expr(expr, ctx)?);
        if let ExcelValue::Error(e) = v {
            return Ok(ExcelValue::Error(e));
        }
        match op {
            UnaryOp::Plus => match self.as_number(&v) {
                Ok(n) => Ok(ExcelValue::Number(n)),
                Err(e) => Ok(ExcelValue::Error(e)),
            },
            UnaryOp::Minus => match self.as_number(&v) {
                Ok(n) => Ok(ExcelValue::Number(-n)),
                Err(e) => Ok(ExcelValue::Error(e)),
            },
            UnaryOp::Percent => match self.as_number(&v) {
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
        let l = self.scalarize(self.eval_expr(left, ctx)?);
        let r = self.scalarize(self.eval_expr(right, ctx)?);
        if let ExcelValue::Error(e) = l {
            return Ok(ExcelValue::Error(e));
        }
        if let ExcelValue::Error(e) = r {
            return Ok(ExcelValue::Error(e));
        }
        Ok(match op {
            BinOp::Add => self.arith(l, r, |a, b| a + b),
            BinOp::Sub => self.arith(l, r, |a, b| a - b),
            BinOp::Mul => self.arith(l, r, |a, b| a * b),
            BinOp::Div => self.div(l, r),
            BinOp::Pow => self.arith(l, r, |a, b| a.powf(b)),
            BinOp::Concat => self.concat(l, r),
            BinOp::Eq => self.cmp_eq(l, r, true),
            BinOp::Ne => match self.cmp_eq(l, r, true) {
                ExcelValue::Bool(b) => ExcelValue::Bool(!b),
                other => other,
            },
            BinOp::Lt => self.cmp_ord(l, r, std::cmp::Ordering::Less, false),
            BinOp::Gt => self.cmp_ord(l, r, std::cmp::Ordering::Greater, false),
            BinOp::Le => self.cmp_ord(l, r, std::cmp::Ordering::Greater, true),
            BinOp::Ge => self.cmp_ord(l, r, std::cmp::Ordering::Less, true),
        })
    }

    fn arith(&self, l: ExcelValue, r: ExcelValue, f: impl Fn(f64, f64) -> f64) -> ExcelValue {
        match (self.as_number(&l), self.as_number(&r)) {
            (Ok(a), Ok(b)) => ExcelValue::Number(f(a, b)),
            (Err(e), _) | (_, Err(e)) => ExcelValue::Error(e),
        }
    }

    fn div(&self, l: ExcelValue, r: ExcelValue) -> ExcelValue {
        match (self.as_number(&l), self.as_number(&r)) {
            (Ok(a), Ok(b)) => {
                if b == 0.0 {
                    match self.semantics {
                        Semantics::ExcelSeed => ExcelValue::Error(ExcelError::Div0),
                        Semantics::Naive => {
                            if a == 0.0 {
                                ExcelValue::Number(f64::NAN)
                            } else {
                                ExcelValue::Number(a / 0.0)
                            }
                        }
                    }
                } else {
                    ExcelValue::Number(a / b)
                }
            }
            (Err(e), _) | (_, Err(e)) => ExcelValue::Error(e),
        }
    }

    fn concat(&self, l: ExcelValue, r: ExcelValue) -> ExcelValue {
        match (self.as_text(&l), self.as_text(&r)) {
            (Ok(a), Ok(b)) => ExcelValue::Text(format!("{a}{b}")),
            (Err(e), _) | (_, Err(e)) => ExcelValue::Error(e),
        }
    }

    fn cmp_eq(&self, l: ExcelValue, r: ExcelValue, _eq: bool) -> ExcelValue {
        match self.semantics {
            Semantics::Naive => naive_eq(&l, &r),
            Semantics::ExcelSeed => excel_eq(&l, &r),
        }
    }

    fn cmp_ord(
        &self,
        l: ExcelValue,
        r: ExcelValue,
        want: std::cmp::Ordering,
        invert: bool,
    ) -> ExcelValue {
        match self.semantics {
            Semantics::Naive => naive_ord(&l, &r, want, invert),
            Semantics::ExcelSeed => excel_ord(&l, &r, want, invert),
        }
    }

    fn eval_call(
        &self,
        name: &str,
        args: &[Expr],
        ctx: &mut Ctx<'_>,
    ) -> Result<ExcelValue, EvalError> {
        let uname = name.to_ascii_uppercase();
        match uname.as_str() {
            "SUM" => self.fn_sum(args, ctx),
            "IF" => self.fn_if(args, ctx),
            "IFERROR" => self.fn_iferror(args, ctx),
            "VLOOKUP" => self.fn_vlookup(args, ctx),
            "ABS" => self.fn_abs(args, ctx),
            "N" => self.fn_n(args, ctx),
            "ISBLANK" => self.fn_is(args, ctx, |v| matches!(v, ExcelValue::Empty)),
            "ISNUMBER" => self.fn_is(args, ctx, |v| matches!(v, ExcelValue::Number(_))),
            "ISTEXT" => self.fn_is(args, ctx, |v| matches!(v, ExcelValue::Text(_))),
            "ISERROR" => self.fn_is(args, ctx, |v| matches!(v, ExcelValue::Error(_))),
            "TRUE" => Ok(ExcelValue::Bool(true)),
            "FALSE" => Ok(ExcelValue::Bool(false)),
            _ => Ok(ExcelValue::Error(ExcelError::Name)),
        }
    }

    fn fn_sum(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        let mut acc = 0.0;
        for arg in args {
            // Cell / range / name references are "range-like": SUM skips
            // logicals and text. Literals (`TRUE`, `"2"`) are coerced.
            let from_range = matches!(arg, Expr::Range(_) | Expr::Cell(_) | Expr::Name(_));
            let v = self.eval_expr(arg, ctx)?;
            if let Some(err) = add_sum(&mut acc, &v, from_range, self.semantics) {
                return Ok(ExcelValue::Error(err));
            }
        }
        Ok(ExcelValue::Number(acc))
    }

    fn fn_if(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.is_empty() {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let cond = self.scalarize(self.eval_expr(&args[0], ctx)?);
        let truth = match self.as_if_cond(&cond) {
            Ok(b) => b,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        if args.len() == 1 {
            return Ok(ExcelValue::Bool(truth));
        }
        if truth {
            self.eval_expr(&args[1], ctx)
        } else if args.len() >= 3 {
            self.eval_expr(&args[2], ctx)
        } else {
            Ok(ExcelValue::Bool(false))
        }
    }

    fn fn_iferror(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let v = self.eval_expr(&args[0], ctx)?;
        if v.is_error() {
            self.eval_expr(&args[1], ctx)
        } else {
            Ok(v)
        }
    }

    fn fn_vlookup(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let lookup = self.scalarize(self.eval_expr(&args[0], ctx)?);
        if let ExcelValue::Error(e) = lookup {
            return Ok(ExcelValue::Error(e));
        }
        let table = self.eval_expr(&args[1], ctx)?;
        let col = self.scalarize(self.eval_expr(&args[2], ctx)?);
        let approx = if args.len() >= 4 {
            match self.as_if_cond(&self.scalarize(self.eval_expr(&args[3], ctx)?)) {
                Ok(b) => b,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            true
        };
        let col_n = match self.as_number(&col) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        if col_n < 1.0 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let col_idx = col_n as usize;
        let rows = match table {
            ExcelValue::Array(rows) => rows,
            other => vec![vec![other]],
        };
        if rows.is_empty() {
            return Ok(ExcelValue::Error(ExcelError::Na));
        }
        let width = rows[0].len();
        if col_idx > width {
            return Ok(ExcelValue::Error(ExcelError::Ref));
        }
        if !approx {
            for row in &rows {
                if excel_lookup_key_eq(&lookup, &row[0], self.semantics) {
                    return Ok(row[col_idx - 1].clone());
                }
            }
            return Ok(ExcelValue::Error(ExcelError::Na));
        }
        // Approximate: last row whose first col <= lookup (numeric). Unsorted
        // tables produce Excel's well-known wrong answers.
        let mut found: Option<&Vec<ExcelValue>> = None;
        for row in &rows {
            if let (Ok(lv), Ok(kv)) = (self.as_number(&lookup), self.as_number(&row[0])) {
                if kv <= lv {
                    found = Some(row);
                }
            }
        }
        match found {
            Some(row) => Ok(row[col_idx - 1].clone()),
            None => Ok(ExcelValue::Error(ExcelError::Na)),
        }
    }

    fn fn_abs(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let v = self.scalarize(self.eval_expr(&args[0], ctx)?);
        match self.as_number(&v) {
            Ok(n) => Ok(ExcelValue::Number(n.abs())),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_n(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let v = self.scalarize(self.eval_expr(&args[0], ctx)?);
        Ok(match v {
            ExcelValue::Number(n) => ExcelValue::Number(n),
            ExcelValue::Bool(true) => ExcelValue::Number(1.0),
            ExcelValue::Bool(false) | ExcelValue::Empty | ExcelValue::Text(_) => {
                ExcelValue::Number(0.0)
            }
            ExcelValue::Error(e) => ExcelValue::Error(e),
            ExcelValue::Array(_) => ExcelValue::Number(0.0),
        })
    }

    fn fn_is(
        &self,
        args: &[Expr],
        ctx: &mut Ctx<'_>,
        pred: impl Fn(&ExcelValue) -> bool,
    ) -> Result<ExcelValue, EvalError> {
        if args.len() != 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let v = self.scalarize(self.eval_expr(&args[0], ctx)?);
        Ok(ExcelValue::Bool(pred(&v)))
    }

    fn as_number(&self, v: &ExcelValue) -> Result<f64, ExcelError> {
        match (self.semantics, v) {
            (_, ExcelValue::Error(e)) => Err(*e),
            (Semantics::ExcelSeed, ExcelValue::Number(n)) => Ok(*n),
            (Semantics::ExcelSeed, ExcelValue::Empty) => Ok(0.0),
            (Semantics::ExcelSeed, ExcelValue::Bool(true)) => Ok(1.0),
            (Semantics::ExcelSeed, ExcelValue::Bool(false)) => Ok(0.0),
            (Semantics::ExcelSeed, ExcelValue::Text(s)) => parse_excel_number(s),
            (Semantics::ExcelSeed, ExcelValue::Array(_)) => Err(ExcelError::Value),
            (Semantics::Naive, ExcelValue::Number(n)) => Ok(*n),
            (Semantics::Naive, _) => Err(ExcelError::Value),
        }
    }

    fn as_text(&self, v: &ExcelValue) -> Result<String, ExcelError> {
        match v {
            ExcelValue::Error(e) => Err(*e),
            ExcelValue::Text(s) => Ok(s.clone()),
            ExcelValue::Empty => Ok(String::new()),
            ExcelValue::Bool(true) => Ok("TRUE".into()),
            ExcelValue::Bool(false) => Ok("FALSE".into()),
            ExcelValue::Number(n) => Ok(format_plain(*n)),
            ExcelValue::Array(_) => Err(ExcelError::Value),
        }
    }

    fn as_if_cond(&self, v: &ExcelValue) -> Result<bool, ExcelError> {
        match (self.semantics, v) {
            (_, ExcelValue::Error(e)) => Err(*e),
            (_, ExcelValue::Bool(b)) => Ok(*b),
            (Semantics::ExcelSeed, ExcelValue::Number(n)) => Ok(*n != 0.0),
            (Semantics::ExcelSeed, ExcelValue::Empty) => Ok(false),
            (Semantics::ExcelSeed, ExcelValue::Text(_)) => Err(ExcelError::Value),
            (Semantics::ExcelSeed, ExcelValue::Array(_)) => Err(ExcelError::Value),
            (Semantics::Naive, ExcelValue::Number(n)) => Ok(*n != 0.0),
            (Semantics::Naive, _) => Err(ExcelError::Value),
        }
    }

    fn scalarize(&self, v: ExcelValue) -> ExcelValue {
        match v {
            ExcelValue::Array(rows) => rows
                .first()
                .and_then(|r| r.first())
                .cloned()
                .unwrap_or(ExcelValue::Empty),
            other => other,
        }
    }
}

fn add_sum(acc: &mut f64, v: &ExcelValue, from_range: bool, sem: Semantics) -> Option<ExcelError> {
    match (sem, v, from_range) {
        (_, ExcelValue::Error(e), _) => Some(*e),
        (_, ExcelValue::Number(n), _) => {
            *acc += *n;
            None
        }
        (_, ExcelValue::Empty, _) => None,
        (Semantics::ExcelSeed, ExcelValue::Bool(b), false) => {
            *acc += if *b { 1.0 } else { 0.0 };
            None
        }
        (Semantics::ExcelSeed, ExcelValue::Bool(_), true) => None,
        (Semantics::ExcelSeed, ExcelValue::Text(s), false) => match parse_excel_number(s) {
            Ok(n) => {
                *acc += n;
                None
            }
            Err(e) => Some(e),
        },
        (Semantics::ExcelSeed, ExcelValue::Text(_), true) => None,
        (Semantics::ExcelSeed, ExcelValue::Array(rows), _) => {
            for row in rows {
                for c in row {
                    if let Some(e) = add_sum(acc, c, true, sem) {
                        return Some(e);
                    }
                }
            }
            None
        }
        (Semantics::Naive, ExcelValue::Bool(b), _) => {
            *acc += if *b { 1.0 } else { 0.0 };
            None
        }
        (Semantics::Naive, ExcelValue::Text(_), _) => None,
        (Semantics::Naive, ExcelValue::Array(rows), _) => {
            for row in rows {
                for c in row {
                    if let Some(e) = add_sum(acc, c, true, sem) {
                        return Some(e);
                    }
                }
            }
            None
        }
    }
}

fn parse_excel_number(s: &str) -> Result<f64, ExcelError> {
    let t = s.trim();
    if t.is_empty() {
        return Err(ExcelError::Value);
    }
    t.parse::<f64>().map_err(|_| ExcelError::Value)
}

fn format_plain(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{n:.0}")
    } else {
        format!("{n}")
    }
}

fn excel_eq(l: &ExcelValue, r: &ExcelValue) -> ExcelValue {
    if let ExcelValue::Error(e) = l {
        return ExcelValue::Error(*e);
    }
    if let ExcelValue::Error(e) = r {
        return ExcelValue::Error(*e);
    }
    ExcelValue::Bool(match (l, r) {
        (ExcelValue::Empty, ExcelValue::Empty) => true,
        (ExcelValue::Empty, ExcelValue::Number(n)) | (ExcelValue::Number(n), ExcelValue::Empty) => {
            excel_num_eq(*n, 0.0)
        }
        (ExcelValue::Empty, ExcelValue::Text(s)) | (ExcelValue::Text(s), ExcelValue::Empty) => {
            s.is_empty()
        }
        (ExcelValue::Empty, ExcelValue::Bool(b)) | (ExcelValue::Bool(b), ExcelValue::Empty) => !b,
        (ExcelValue::Number(a), ExcelValue::Number(b)) => excel_num_eq(*a, *b),
        (ExcelValue::Text(a), ExcelValue::Text(b)) => a.eq_ignore_ascii_case(b),
        (ExcelValue::Bool(a), ExcelValue::Bool(b)) => a == b,
        (ExcelValue::Number(n), ExcelValue::Bool(b))
        | (ExcelValue::Bool(b), ExcelValue::Number(n)) => {
            excel_num_eq(*n, if *b { 1.0 } else { 0.0 })
        }
        _ => false,
    })
}

fn naive_eq(l: &ExcelValue, r: &ExcelValue) -> ExcelValue {
    ExcelValue::Bool(match (l, r) {
        (ExcelValue::Number(a), ExcelValue::Number(b)) => a == b, // IEEE, NaN ≠
        (ExcelValue::Text(a), ExcelValue::Text(b)) => a == b,     // case-sensitive
        (ExcelValue::Bool(a), ExcelValue::Bool(b)) => a == b,
        (ExcelValue::Empty, ExcelValue::Empty) => true,
        (ExcelValue::Number(n), ExcelValue::Text(s))
        | (ExcelValue::Text(s), ExcelValue::Number(n)) => {
            s.parse::<f64>().ok().is_some_and(|p| p == *n)
        }
        _ => false,
    })
}

fn excel_ord(
    l: &ExcelValue,
    r: &ExcelValue,
    want: std::cmp::Ordering,
    invert_want: bool,
) -> ExcelValue {
    if let ExcelValue::Error(e) = l {
        return ExcelValue::Error(*e);
    }
    if let ExcelValue::Error(e) = r {
        return ExcelValue::Error(*e);
    }
    let rank = |v: &ExcelValue| -> u8 {
        match v {
            ExcelValue::Number(_) | ExcelValue::Empty => 0,
            ExcelValue::Text(_) => 1,
            ExcelValue::Bool(_) => 2,
            ExcelValue::Error(_) | ExcelValue::Array(_) => 9,
        }
    };
    let rl = rank(l);
    let rr = rank(r);
    let ord = if rl != rr {
        rl.cmp(&rr)
    } else {
        match (l, r) {
            (ExcelValue::Number(a), ExcelValue::Number(b)) => {
                a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
            }
            (ExcelValue::Empty, ExcelValue::Number(b)) => {
                0.0.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
            }
            (ExcelValue::Number(a), ExcelValue::Empty) => {
                a.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal)
            }
            (ExcelValue::Empty, ExcelValue::Empty) => std::cmp::Ordering::Equal,
            (ExcelValue::Text(a), ExcelValue::Text(b)) => {
                a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase())
            }
            (ExcelValue::Bool(a), ExcelValue::Bool(b)) => a.cmp(b),
            _ => std::cmp::Ordering::Equal,
        }
    };
    let hit = if invert_want {
        ord != want
    } else {
        ord == want
    };
    ExcelValue::Bool(hit)
}

fn naive_ord(
    l: &ExcelValue,
    r: &ExcelValue,
    want: std::cmp::Ordering,
    invert_want: bool,
) -> ExcelValue {
    // Numeric-only; refuse type ranking (this is the point of the naive stub).
    let num = |v: &ExcelValue| match v {
        ExcelValue::Number(n) => Some(*n),
        ExcelValue::Text(s) => s.parse().ok(),
        ExcelValue::Bool(true) => Some(1.0),
        ExcelValue::Bool(false) => Some(0.0),
        _ => None,
    };
    match (num(l), num(r)) {
        (Some(a), Some(b)) => {
            let ord = a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal);
            let hit = if invert_want {
                ord != want
            } else {
                ord == want
            };
            ExcelValue::Bool(hit)
        }
        _ => ExcelValue::Error(ExcelError::Value),
    }
}

fn excel_lookup_key_eq(lookup: &ExcelValue, key: &ExcelValue, sem: Semantics) -> bool {
    match sem {
        Semantics::ExcelSeed => matches!(excel_eq(lookup, key), ExcelValue::Bool(true)),
        Semantics::Naive => matches!(naive_eq(lookup, key), ExcelValue::Bool(true)),
    }
}

/// Used by tests that want a workbook-backed evaluation without the Candidate trait.
pub fn eval_formula_in(
    workbook: &Workbook,
    formula: &str,
    sem: Semantics,
) -> Result<ExcelValue, EvalError> {
    let spec = EvalSpec {
        case_id: "adhoc".into(),
        workbook: workbook.clone(),
        target: EvalTarget::formula(formula),
        options: Default::default(),
    };
    Interpreter::new(sem).eval_spec(&spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excel_div0_and_naive_inf() {
        let wb = Workbook::default();
        let e = eval_formula_in(&wb, "=1/0", Semantics::ExcelSeed).unwrap();
        assert_eq!(e, ExcelValue::Error(ExcelError::Div0));
        let n = eval_formula_in(&wb, "=1/0", Semantics::Naive).unwrap();
        assert!(matches!(n, ExcelValue::Number(x) if x.is_infinite()));
    }

    #[test]
    fn excel_text_plus() {
        let wb = Workbook::default();
        let e = eval_formula_in(&wb, "=\"2\"+1", Semantics::ExcelSeed).unwrap();
        assert_eq!(e, ExcelValue::Number(3.0));
        let n = eval_formula_in(&wb, "=\"2\"+1", Semantics::Naive).unwrap();
        assert_eq!(n, ExcelValue::Error(ExcelError::Value));
    }

    #[test]
    fn excel_fuzzy_eq() {
        let wb = Workbook::default();
        let e = eval_formula_in(&wb, "=0.1+0.2=0.3", Semantics::ExcelSeed).unwrap();
        assert_eq!(e, ExcelValue::Bool(true));
        let n = eval_formula_in(&wb, "=0.1+0.2=0.3", Semantics::Naive).unwrap();
        assert_eq!(n, ExcelValue::Bool(false));
    }
}
