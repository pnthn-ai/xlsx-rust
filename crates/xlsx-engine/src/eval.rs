//! Seed-scoped evaluator with Excel-like and intentionally naive semantics.

use crate::dates::{date_serial, serial_to_ymd, time_fraction};
use crate::parse::{parse, BinOp, Expr, UnaryOp};
use std::collections::HashSet;
use xlsx_types::{
    excel_num_eq, ArrayMode, CellAddr, CellRef, EvalError, EvalSpec, EvalTarget, ExcelError,
    ExcelValue, RangeRef, Workbook,
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
    host: CellAddr,
}

impl Interpreter {
    pub fn new(semantics: Semantics) -> Self {
        Self { semantics }
    }

    pub fn eval_spec(&self, spec: &EvalSpec) -> Result<ExcelValue, EvalError> {
        let default_cell = spec.default_cell();
        let current_sheet = default_cell
            .sheet
            .clone()
            .unwrap_or_else(|| spec.workbook.default_sheet_name().to_string());
        let mut ctx = Ctx {
            spec,
            current_sheet,
            depth: 0,
            visiting: HashSet::new(),
            host: default_cell.addr,
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
        // Top-level range in scalar mode uses implicit intersection (ExcelSeed).
        if matches!(self.semantics, Semantics::ExcelSeed)
            && matches!(ctx.spec.options.array_mode, ArrayMode::Scalar)
        {
            if let Expr::Range(r) = &ast {
                return self.implicit_intersect_range(r, ctx);
            }
        }
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
            BinOp::Add => self.arith(l, r, |a, b| a + b),
            BinOp::Sub => self.arith(l, r, |a, b| a - b),
            BinOp::Mul => self.arith(l, r, |a, b| a * b),
            BinOp::Div => self.div(l, r),
            BinOp::Pow => self.pow(l, r),
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
            BinOp::Intersect => unreachable!("intersect handled above"),
        })
    }

    fn pow(&self, l: ExcelValue, r: ExcelValue) -> ExcelValue {
        match (self.as_number(&l), self.as_number(&r)) {
            (Ok(a), Ok(b)) => {
                if matches!(self.semantics, Semantics::ExcelSeed) {
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
                } else {
                    ExcelValue::Number(a.powf(b))
                }
            }
            (Err(e), _) | (_, Err(e)) => ExcelValue::Error(e),
        }
    }

    fn eval_intersect(
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
            "SUM" => self.fn_agg(args, ctx, AggKind::Sum),
            "PRODUCT" => self.fn_agg(args, ctx, AggKind::Product),
            "AVERAGE" => self.fn_agg(args, ctx, AggKind::Average),
            "MIN" => self.fn_agg(args, ctx, AggKind::Min),
            "MAX" => self.fn_agg(args, ctx, AggKind::Max),
            "COUNT" => self.fn_agg(args, ctx, AggKind::Count),
            "COUNTA" => self.fn_agg(args, ctx, AggKind::CountA),
            "COUNTBLANK" => self.fn_agg(args, ctx, AggKind::CountBlank),
            "IF" => self.fn_if(args, ctx),
            "IFERROR" => self.fn_iferror(args, ctx),
            "IFNA" => self.fn_ifna(args, ctx),
            "AND" => self.fn_and_or(args, ctx, AndOr::And),
            "OR" => self.fn_and_or(args, ctx, AndOr::Or),
            "XOR" => self.fn_and_or(args, ctx, AndOr::Xor),
            "NOT" => self.fn_not(args, ctx),
            "VLOOKUP" => self.fn_vlookup(args, ctx),
            "HLOOKUP" => self.fn_hlookup(args, ctx),
            "XLOOKUP" => self.fn_xlookup(args, ctx),
            "INDEX" => self.fn_index(args, ctx),
            "MATCH" => self.fn_match(args, ctx),
            "CHOOSE" => self.fn_choose(args, ctx),
            "ABS" => self.fn_abs(args, ctx),
            "SIGN" => self.fn_sign(args, ctx),
            "INT" => self.fn_int(args, ctx),
            "TRUNC" => self.fn_trunc(args, ctx),
            "ROUND" => self.fn_round(args, ctx),
            "MOD" => self.fn_mod(args, ctx),
            "SQRT" => self.fn_sqrt(args, ctx),
            "POWER" => self.fn_power(args, ctx),
            "PI" => Ok(ExcelValue::Number(std::f64::consts::PI)),
            "N" => self.fn_n(args, ctx),
            "NA" => Ok(ExcelValue::Error(ExcelError::Na)),
            "TYPE" => self.fn_type(args, ctx),
            "ERROR.TYPE" => self.fn_error_type(args, ctx),
            "ISBLANK" => self.fn_is(args, ctx, |v| matches!(v, ExcelValue::Empty)),
            "ISNUMBER" => self.fn_is(args, ctx, |v| matches!(v, ExcelValue::Number(_))),
            "ISTEXT" => self.fn_is(args, ctx, |v| matches!(v, ExcelValue::Text(_))),
            "ISLOGICAL" => self.fn_is(args, ctx, |v| matches!(v, ExcelValue::Bool(_))),
            "ISNONTEXT" => self.fn_is(args, ctx, |v| !matches!(v, ExcelValue::Text(_))),
            "ISERROR" => self.fn_is(args, ctx, |v| matches!(v, ExcelValue::Error(_))),
            "ISERR" => self.fn_is(
                args,
                ctx,
                |v| matches!(v, ExcelValue::Error(e) if *e != ExcelError::Na),
            ),
            "ISNA" => self.fn_is(args, ctx, |v| {
                matches!(v, ExcelValue::Error(ExcelError::Na))
            }),
            "ISEVEN" => self.fn_even_odd(args, ctx, true),
            "ISODD" => self.fn_even_odd(args, ctx, false),
            "DATE" => self.fn_date(args, ctx),
            "TIME" => self.fn_time(args, ctx),
            "YEAR" => self.fn_ymd(args, ctx, YmdPart::Year),
            "MONTH" => self.fn_ymd(args, ctx, YmdPart::Month),
            "DAY" => self.fn_ymd(args, ctx, YmdPart::Day),
            "LEFT" => self.fn_left_right(args, ctx, true),
            "RIGHT" => self.fn_left_right(args, ctx, false),
            "MID" => self.fn_mid(args, ctx),
            "LEN" => self.fn_len(args, ctx),
            "LOWER" => self.fn_case(args, ctx, true),
            "UPPER" => self.fn_case(args, ctx, false),
            "TRIM" => self.fn_trim(args, ctx),
            "EXACT" => self.fn_exact(args, ctx),
            "VALUE" => self.fn_value(args, ctx),
            "TEXT" => self.fn_text(args, ctx),
            "TRUE" => Ok(ExcelValue::Bool(true)),
            "FALSE" => Ok(ExcelValue::Bool(false)),
            _ => Ok(ExcelValue::Error(ExcelError::Name)),
        }
    }

    fn fn_agg(
        &self,
        args: &[Expr],
        ctx: &mut Ctx<'_>,
        kind: AggKind,
    ) -> Result<ExcelValue, EvalError> {
        let mut acc = AggAcc::new(kind);
        for arg in args {
            // Cell / range / name references are "range-like": aggregators skip
            // logicals and text. Literals (`TRUE`, `"2"`) are coerced.
            let from_range = matches!(arg, Expr::Range(_) | Expr::Cell(_) | Expr::Name(_));
            let v = self.eval_expr(arg, ctx)?;
            if let Some(err) = acc.fold(&v, from_range, self.semantics) {
                return Ok(ExcelValue::Error(err));
            }
        }
        Ok(acc.finish())
    }

    fn fn_if(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.is_empty() {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let cond = self.eval_scalar(&args[0], ctx)?;
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

    fn fn_ifna(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let v = self.eval_expr(&args[0], ctx)?;
        if matches!(v, ExcelValue::Error(ExcelError::Na)) {
            self.eval_expr(&args[1], ctx)
        } else {
            Ok(v)
        }
    }

    fn fn_and_or(
        &self,
        args: &[Expr],
        ctx: &mut Ctx<'_>,
        kind: AndOr,
    ) -> Result<ExcelValue, EvalError> {
        if args.is_empty() {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let mut seen = 0usize;
        let mut true_count = 0usize;
        for arg in args {
            let v = self.eval_expr(arg, ctx)?;
            if let Some(err) = fold_logicals(&v, &mut seen, &mut true_count, self.semantics) {
                return Ok(ExcelValue::Error(err));
            }
        }
        Ok(ExcelValue::Bool(match kind {
            AndOr::And => true_count == seen, // vacuous AND of only blanks is TRUE
            AndOr::Or => true_count > 0,
            AndOr::Xor => true_count % 2 == 1,
        }))
    }

    fn fn_not(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let v = self.eval_scalar(&args[0], ctx)?;
        match self.as_if_cond(&v) {
            Ok(b) => Ok(ExcelValue::Bool(!b)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_vlookup(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let lookup = self.eval_scalar(&args[0], ctx)?;
        if let ExcelValue::Error(e) = lookup {
            return Ok(ExcelValue::Error(e));
        }
        let table = self.eval_expr(&args[1], ctx)?;
        let col = self.eval_scalar(&args[2], ctx)?;
        let approx = if args.len() >= 4 {
            match self.as_if_cond(&self.eval_scalar(&args[3], ctx)?) {
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
                if lookup_key_match(&lookup, &row[0], self.semantics) {
                    return Ok(row[col_idx - 1].clone());
                }
            }
            return Ok(ExcelValue::Error(ExcelError::Na));
        }
        // Approximate: Excel binary-searches the first column. Unsorted tables
        // therefore return well-known wrong answers.
        match approx_upper_bound(&rows, &lookup, self.semantics) {
            Some(i) => Ok(rows[i][col_idx - 1].clone()),
            None => Ok(ExcelValue::Error(ExcelError::Na)),
        }
    }

    fn fn_hlookup(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let lookup = self.eval_scalar(&args[0], ctx)?;
        if let ExcelValue::Error(e) = lookup {
            return Ok(ExcelValue::Error(e));
        }
        let table = self.eval_expr(&args[1], ctx)?;
        let row = self.eval_scalar(&args[2], ctx)?;
        let approx = if args.len() >= 4 {
            match self.as_if_cond(&self.eval_scalar(&args[3], ctx)?) {
                Ok(b) => b,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            true
        };
        let row_n = match self.as_number(&row) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        if row_n < 1.0 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let row_idx = row_n as usize;
        let rows = match table {
            ExcelValue::Array(rows) => rows,
            other => vec![vec![other]],
        };
        if rows.is_empty() || row_idx > rows.len() {
            return Ok(ExcelValue::Error(if rows.is_empty() {
                ExcelError::Na
            } else {
                ExcelError::Ref
            }));
        }
        let width = rows[0].len();
        // Search the first row; return the matching column at row_idx.
        let header: Vec<ExcelValue> = (0..width).map(|c| rows[0][c].clone()).collect();
        let keys: Vec<Vec<ExcelValue>> = header.into_iter().map(|k| vec![k]).collect();
        let col = if !approx {
            keys.iter()
                .position(|k| lookup_key_match(&lookup, &k[0], self.semantics))
        } else {
            approx_upper_bound(&keys, &lookup, self.semantics)
        };
        match col {
            Some(c) => Ok(rows[row_idx - 1][c].clone()),
            None => Ok(ExcelValue::Error(ExcelError::Na)),
        }
    }

    fn fn_xlookup(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let lookup = self.eval_scalar(&args[0], ctx)?;
        if let ExcelValue::Error(e) = lookup {
            return Ok(ExcelValue::Error(e));
        }
        let lookup_vec = flatten_vector(self.eval_expr(&args[1], ctx)?);
        let return_vec = flatten_vector(self.eval_expr(&args[2], ctx)?);
        if lookup_vec.len() != return_vec.len() {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        for (k, v) in lookup_vec.iter().zip(return_vec.iter()) {
            if lookup_key_match(&lookup, k, self.semantics) {
                return Ok(v.clone());
            }
        }
        if args.len() >= 4 {
            return self.eval_expr(&args[3], ctx);
        }
        Ok(ExcelValue::Error(ExcelError::Na))
    }

    fn fn_index(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.is_empty() {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let array = match self.eval_expr(&args[0], ctx)? {
            ExcelValue::Array(rows) => rows,
            other => vec![vec![other]],
        };
        if array.is_empty() || array[0].is_empty() {
            return Ok(ExcelValue::Error(ExcelError::Ref));
        }
        let nrows = array.len();
        let ncols = array[0].len();
        let row_n = if args.len() >= 2 {
            match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            1.0
        };
        let col_n = if args.len() >= 3 {
            match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else if nrows == 1 {
            row_n
        } else {
            1.0
        };
        let (row_idx, col_idx) = if nrows == 1 && args.len() == 2 {
            (1usize, row_n as usize)
        } else if ncols == 1 && args.len() == 2 {
            (row_n as usize, 1usize)
        } else {
            (row_n as usize, col_n as usize)
        };
        if row_idx < 1 || col_idx < 1 || row_idx > nrows || col_idx > ncols {
            return Ok(ExcelValue::Error(ExcelError::Ref));
        }
        Ok(array[row_idx - 1][col_idx - 1].clone())
    }

    fn fn_match(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let lookup = self.eval_scalar(&args[0], ctx)?;
        if let ExcelValue::Error(e) = lookup {
            return Ok(ExcelValue::Error(e));
        }
        let vec = flatten_vector(self.eval_expr(&args[1], ctx)?);
        let match_type = if args.len() >= 3 {
            match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            1.0
        };
        let keys: Vec<Vec<ExcelValue>> = vec.into_iter().map(|k| vec![k]).collect();
        if match_type == 0.0 {
            for (i, row) in keys.iter().enumerate() {
                if lookup_key_match(&lookup, &row[0], self.semantics) {
                    return Ok(ExcelValue::Number((i + 1) as f64));
                }
            }
            return Ok(ExcelValue::Error(ExcelError::Na));
        }
        if match_type > 0.0 {
            return Ok(match approx_upper_bound(&keys, &lookup, self.semantics) {
                Some(i) => ExcelValue::Number((i + 1) as f64),
                None => ExcelValue::Error(ExcelError::Na),
            });
        }
        // match_type < 0: first key >= lookup on a descending list (linear).
        for (i, row) in keys.iter().enumerate() {
            if excel_geq(&row[0], &lookup) {
                return Ok(ExcelValue::Number((i + 1) as f64));
            }
        }
        Ok(ExcelValue::Error(ExcelError::Na))
    }

    fn fn_choose(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let idx = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let i = idx.trunc() as i64;
        if i < 1 || i as usize >= args.len() {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        self.eval_expr(&args[i as usize], ctx)
    }

    fn fn_abs(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        self.fn_unary_num(args, ctx, |n| ExcelValue::Number(n.abs()))
    }

    fn fn_sign(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        self.fn_unary_num(args, ctx, |n| {
            ExcelValue::Number(if n > 0.0 {
                1.0
            } else if n < 0.0 {
                -1.0
            } else {
                0.0
            })
        })
    }

    fn fn_int(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        self.fn_unary_num(args, ctx, |n| ExcelValue::Number(n.floor()))
    }

    fn fn_trunc(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.is_empty() {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let n = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let digits = if args.len() >= 2 {
            match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
                Ok(d) => d.trunc() as i32,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            0
        };
        Ok(ExcelValue::Number(excel_trunc(n, digits)))
    }

    fn fn_round(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let n = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let digits = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(d) => d.trunc() as i32,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        Ok(ExcelValue::Number(excel_round_half_away(n, digits)))
    }

    fn fn_mod(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let n = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let d = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        if d == 0.0 {
            return Ok(ExcelValue::Error(ExcelError::Div0));
        }
        // Excel: n - d * INT(n/d)
        Ok(ExcelValue::Number(n - d * (n / d).floor()))
    }

    fn fn_sqrt(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        self.fn_unary_num(args, ctx, |n| {
            if n < 0.0 {
                ExcelValue::Error(ExcelError::Num)
            } else {
                ExcelValue::Number(n.sqrt())
            }
        })
    }

    fn fn_power(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let l = self.eval_scalar(&args[0], ctx)?;
        let r = self.eval_scalar(&args[1], ctx)?;
        Ok(self.pow(l, r))
    }

    fn fn_unary_num(
        &self,
        args: &[Expr],
        ctx: &mut Ctx<'_>,
        f: impl Fn(f64) -> ExcelValue,
    ) -> Result<ExcelValue, EvalError> {
        if args.len() != 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let v = self.eval_scalar(&args[0], ctx)?;
        match self.as_number(&v) {
            Ok(n) => Ok(f(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_n(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let v = self.eval_scalar(&args[0], ctx)?;
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

    fn fn_type(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        // TYPE of an array is 64 even in scalar context — don't implicit-intersect.
        let v = self.eval_expr(&args[0], ctx)?;
        Ok(ExcelValue::Number(match v {
            ExcelValue::Number(_) => 1.0,
            ExcelValue::Text(_) => 2.0,
            ExcelValue::Bool(_) => 4.0,
            ExcelValue::Error(_) => 16.0,
            ExcelValue::Array(_) => 64.0,
            ExcelValue::Empty => 1.0, // blank coerces like a number for TYPE
        }))
    }

    fn fn_error_type(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let v = self.eval_scalar(&args[0], ctx)?;
        Ok(match v {
            ExcelValue::Error(e) => match e {
                ExcelError::Null => ExcelValue::Number(1.0),
                ExcelError::Div0 => ExcelValue::Number(2.0),
                ExcelError::Value => ExcelValue::Number(3.0),
                ExcelError::Ref => ExcelValue::Number(4.0),
                ExcelError::Name => ExcelValue::Number(5.0),
                ExcelError::Num => ExcelValue::Number(6.0),
                ExcelError::Na => ExcelValue::Number(7.0),
                ExcelError::GettingData => ExcelValue::Number(8.0),
                _ => ExcelValue::Error(ExcelError::Na),
            },
            _ => ExcelValue::Error(ExcelError::Na),
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
        let v = self.eval_scalar(&args[0], ctx)?;
        Ok(ExcelValue::Bool(pred(&v)))
    }

    fn fn_even_odd(
        &self,
        args: &[Expr],
        ctx: &mut Ctx<'_>,
        even: bool,
    ) -> Result<ExcelValue, EvalError> {
        if args.len() != 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => {
                let t = n.trunc() as i64;
                Ok(ExcelValue::Bool(if even { t % 2 == 0 } else { t % 2 != 0 }))
            }
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_date(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let y = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n.trunc() as i32,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let m = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n.trunc() as i32,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let d = match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
            Ok(n) => n.trunc() as i32,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        match date_serial(y, m, d, ctx.spec.options.date_system) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_time(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let h = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let m = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let s = match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        match time_fraction(h, m, s) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_ymd(
        &self,
        args: &[Expr],
        ctx: &mut Ctx<'_>,
        part: YmdPart,
    ) -> Result<ExcelValue, EvalError> {
        if args.len() != 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let n = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        match serial_to_ymd(n, ctx.spec.options.date_system) {
            Ok((y, m, d)) => Ok(ExcelValue::Number(match part {
                YmdPart::Year => y as f64,
                YmdPart::Month => m as f64,
                YmdPart::Day => d as f64,
            })),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_left_right(
        &self,
        args: &[Expr],
        ctx: &mut Ctx<'_>,
        left: bool,
    ) -> Result<ExcelValue, EvalError> {
        if args.is_empty() {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let s = match self.as_text(&self.eval_scalar(&args[0], ctx)?) {
            Ok(s) => s,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let n = if args.len() >= 2 {
            match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
                Ok(n) => n.trunc() as i64,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            1
        };
        if n < 0 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let chars: Vec<char> = s.chars().collect();
        let take = (n as usize).min(chars.len());
        let out: String = if left {
            chars.iter().take(take).collect()
        } else {
            chars
                .iter()
                .skip(chars.len().saturating_sub(take))
                .collect()
        };
        Ok(ExcelValue::Text(out))
    }

    fn fn_mid(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let s = match self.as_text(&self.eval_scalar(&args[0], ctx)?) {
            Ok(s) => s,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let start = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n.trunc() as i64,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let len = match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
            Ok(n) => n.trunc() as i64,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        if start < 1 || len < 0 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let chars: Vec<char> = s.chars().collect();
        let i = (start as usize) - 1;
        if i >= chars.len() {
            return Ok(ExcelValue::Text(String::new()));
        }
        Ok(ExcelValue::Text(
            chars.iter().skip(i).take(len as usize).collect(),
        ))
    }

    fn fn_len(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        match self.as_text(&self.eval_scalar(&args[0], ctx)?) {
            Ok(s) => Ok(ExcelValue::Number(s.chars().count() as f64)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_case(
        &self,
        args: &[Expr],
        ctx: &mut Ctx<'_>,
        lower: bool,
    ) -> Result<ExcelValue, EvalError> {
        if args.len() != 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        match self.as_text(&self.eval_scalar(&args[0], ctx)?) {
            Ok(s) => Ok(ExcelValue::Text(if lower {
                s.to_ascii_lowercase()
            } else {
                s.to_ascii_uppercase()
            })),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_trim(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        match self.as_text(&self.eval_scalar(&args[0], ctx)?) {
            Ok(s) => {
                let mut out = String::new();
                let mut prev_space = false;
                for c in s.trim_matches(' ').chars() {
                    if c == ' ' {
                        if !prev_space {
                            out.push(' ');
                        }
                        prev_space = true;
                    } else {
                        out.push(c);
                        prev_space = false;
                    }
                }
                Ok(ExcelValue::Text(out))
            }
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_exact(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let a = match self.as_text(&self.eval_scalar(&args[0], ctx)?) {
            Ok(s) => s,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let b = match self.as_text(&self.eval_scalar(&args[1], ctx)?) {
            Ok(s) => s,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        Ok(ExcelValue::Bool(a == b))
    }

    fn fn_text(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let value = self.eval_scalar(&args[0], ctx)?;
        if let ExcelValue::Error(e) = value {
            return Ok(ExcelValue::Error(e));
        }
        let fmt_v = self.eval_scalar(&args[1], ctx)?;
        if let ExcelValue::Error(e) = fmt_v {
            return Ok(ExcelValue::Error(e));
        }
        let fmt = match self.as_text(&fmt_v) {
            Ok(s) => s,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        match xlsx_engine_core::text_format::apply(&value, &fmt, ctx.spec.options.date_system) {
            Ok(s) => Ok(ExcelValue::Text(s)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_value(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let v = self.eval_scalar(&args[0], ctx)?;
        match v {
            ExcelValue::Number(n) => Ok(ExcelValue::Number(n)),
            ExcelValue::Bool(true) => Ok(ExcelValue::Number(1.0)),
            ExcelValue::Bool(false) => Ok(ExcelValue::Number(0.0)),
            ExcelValue::Empty => Ok(ExcelValue::Number(0.0)),
            ExcelValue::Text(s) => match parse_excel_number(&s) {
                Ok(n) => Ok(ExcelValue::Number(n)),
                Err(e) => Ok(ExcelValue::Error(e)),
            },
            ExcelValue::Error(e) => Ok(ExcelValue::Error(e)),
            ExcelValue::Array(_) => Ok(ExcelValue::Error(ExcelError::Value)),
        }
    }

    fn eval_scalar(&self, expr: &Expr, ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        match expr {
            Expr::Range(r) => match self.semantics {
                Semantics::ExcelSeed => self.implicit_intersect_range(r, ctx),
                Semantics::Naive => Ok(self.scalarize(self.eval_range(r, ctx)?)),
            },
            other => Ok(self.scalarize(self.eval_expr(other, ctx)?)),
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

#[derive(Clone, Copy)]
enum AggKind {
    Sum,
    Product,
    Average,
    Min,
    Max,
    Count,
    CountA,
    CountBlank,
}

#[derive(Clone, Copy)]
enum AndOr {
    And,
    Or,
    Xor,
}

#[derive(Clone, Copy)]
enum YmdPart {
    Year,
    Month,
    Day,
}

struct AggAcc {
    kind: AggKind,
    sum: f64,
    product: f64,
    min: Option<f64>,
    max: Option<f64>,
    count: usize,
    counta: usize,
    countblank: usize,
}

impl AggAcc {
    fn new(kind: AggKind) -> Self {
        Self {
            kind,
            sum: 0.0,
            product: 1.0,
            min: None,
            max: None,
            count: 0,
            counta: 0,
            countblank: 0,
        }
    }

    fn fold(&mut self, v: &ExcelValue, from_range: bool, sem: Semantics) -> Option<ExcelError> {
        match (sem, v, from_range) {
            (_, ExcelValue::Array(rows), _) => {
                for row in rows {
                    for c in row {
                        if let Some(e) = self.fold(c, true, sem) {
                            return Some(e);
                        }
                    }
                }
                return None;
            }
            (_, ExcelValue::Error(e), _) => match self.kind {
                AggKind::CountA => {
                    self.counta += 1;
                    None
                }
                AggKind::Count | AggKind::CountBlank => None,
                _ => Some(*e),
            },
            (_, ExcelValue::Number(n), _) => {
                self.add_number(*n);
                self.counta += 1;
                None
            }
            (_, ExcelValue::Empty, _) => {
                self.countblank += 1;
                None
            }
            (Semantics::ExcelSeed, ExcelValue::Bool(b), false) => {
                match self.kind {
                    AggKind::CountBlank => {}
                    AggKind::CountA => self.counta += 1,
                    AggKind::Count => {
                        self.count += 1;
                    }
                    _ => self.add_number(if *b { 1.0 } else { 0.0 }),
                }
                None
            }
            (Semantics::ExcelSeed, ExcelValue::Bool(_), true) => {
                if matches!(self.kind, AggKind::CountA) {
                    self.counta += 1;
                }
                None
            }
            (Semantics::ExcelSeed, ExcelValue::Text(s), false) => match self.kind {
                AggKind::CountBlank => {
                    if s.is_empty() {
                        self.countblank += 1;
                    }
                    None
                }
                AggKind::CountA => {
                    self.counta += 1;
                    None
                }
                AggKind::Count => {
                    if parse_excel_number(s).is_ok() {
                        self.count += 1;
                    }
                    None
                }
                _ => match parse_excel_number(s) {
                    Ok(n) => {
                        self.add_number(n);
                        None
                    }
                    Err(e) => Some(e),
                },
            },
            (Semantics::ExcelSeed, ExcelValue::Text(s), true) => {
                match self.kind {
                    AggKind::CountA => self.counta += 1,
                    AggKind::CountBlank if s.is_empty() => self.countblank += 1,
                    _ => {}
                }
                None
            }
            (Semantics::Naive, ExcelValue::Bool(b), _) => {
                self.add_number(if *b { 1.0 } else { 0.0 });
                self.counta += 1;
                None
            }
            (Semantics::Naive, ExcelValue::Text(s), _) => {
                if s.is_empty() {
                    self.countblank += 1;
                } else {
                    self.counta += 1;
                }
                None
            }
        }
    }

    fn add_number(&mut self, n: f64) {
        self.sum += n;
        self.product *= n;
        self.count += 1;
        self.min = Some(self.min.map_or(n, |m| m.min(n)));
        self.max = Some(self.max.map_or(n, |m| m.max(n)));
    }

    fn finish(self) -> ExcelValue {
        match self.kind {
            AggKind::Sum => ExcelValue::Number(self.sum),
            AggKind::Product => {
                ExcelValue::Number(if self.count == 0 { 0.0 } else { self.product })
            }
            AggKind::Average => {
                if self.count == 0 {
                    ExcelValue::Error(ExcelError::Div0)
                } else {
                    ExcelValue::Number(self.sum / self.count as f64)
                }
            }
            AggKind::Min => ExcelValue::Number(self.min.unwrap_or(0.0)),
            AggKind::Max => ExcelValue::Number(self.max.unwrap_or(0.0)),
            AggKind::Count => ExcelValue::Number(self.count as f64),
            AggKind::CountA => ExcelValue::Number(self.counta as f64),
            AggKind::CountBlank => ExcelValue::Number(self.countblank as f64),
        }
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

fn flatten_vector(v: ExcelValue) -> Vec<ExcelValue> {
    match v {
        ExcelValue::Array(rows) => {
            if rows.len() == 1 {
                rows.into_iter().next().unwrap_or_default()
            } else {
                rows.into_iter()
                    .filter_map(|r| r.into_iter().next())
                    .collect()
            }
        }
        other => vec![other],
    }
}

fn fold_logicals(
    v: &ExcelValue,
    seen: &mut usize,
    true_count: &mut usize,
    sem: Semantics,
) -> Option<ExcelError> {
    match v {
        ExcelValue::Array(rows) => {
            for row in rows {
                for c in row {
                    if let Some(e) = fold_logicals(c, seen, true_count, sem) {
                        return Some(e);
                    }
                }
            }
            None
        }
        ExcelValue::Error(e) => Some(*e),
        ExcelValue::Empty => None,
        ExcelValue::Bool(b) => {
            *seen += 1;
            if *b {
                *true_count += 1;
            }
            None
        }
        ExcelValue::Number(n) => {
            *seen += 1;
            if *n != 0.0 {
                *true_count += 1;
            }
            None
        }
        ExcelValue::Text(_) => {
            if matches!(sem, Semantics::ExcelSeed) {
                Some(ExcelError::Value)
            } else {
                None
            }
        }
    }
}

fn lookup_key_match(lookup: &ExcelValue, key: &ExcelValue, sem: Semantics) -> bool {
    if matches!(sem, Semantics::ExcelSeed) {
        if let ExcelValue::Text(pat) = lookup {
            if looks_like_wildcard(pat) {
                let key_text = match key {
                    ExcelValue::Text(s) => s.clone(),
                    ExcelValue::Number(n) => format_plain(*n),
                    ExcelValue::Bool(true) => "TRUE".into(),
                    ExcelValue::Bool(false) => "FALSE".into(),
                    ExcelValue::Empty => String::new(),
                    _ => return false,
                };
                return excel_wildcard(pat, &key_text);
            }
        }
    }
    match sem {
        Semantics::ExcelSeed => matches!(excel_eq(lookup, key), ExcelValue::Bool(true)),
        Semantics::Naive => matches!(naive_eq(lookup, key), ExcelValue::Bool(true)),
    }
}

fn looks_like_wildcard(s: &str) -> bool {
    s.contains('*') || s.contains('?') || s.contains('~')
}

fn excel_wildcard(pat: &str, text: &str) -> bool {
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

/// Binary-search last index whose first-column key is `<= lookup` (Excel approx).
fn approx_upper_bound(
    rows: &[Vec<ExcelValue>],
    lookup: &ExcelValue,
    sem: Semantics,
) -> Option<usize> {
    if rows.is_empty() {
        return None;
    }
    if matches!(sem, Semantics::Naive) {
        let mut found = None;
        for (i, row) in rows.iter().enumerate() {
            if let (ExcelValue::Number(lv), ExcelValue::Number(kv)) = (lookup, &row[0]) {
                if kv <= lv {
                    found = Some(i);
                }
            }
        }
        return found;
    }
    let mut lo = 0usize;
    let mut hi = rows.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        if excel_leq(&rows[mid][0], lookup) {
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

fn excel_leq(key: &ExcelValue, lookup: &ExcelValue) -> bool {
    matches!(
        excel_ord(key, lookup, std::cmp::Ordering::Greater, true),
        ExcelValue::Bool(true)
    )
}

fn excel_geq(key: &ExcelValue, lookup: &ExcelValue) -> bool {
    matches!(
        excel_ord(key, lookup, std::cmp::Ordering::Less, true),
        ExcelValue::Bool(true)
    )
}

fn excel_round_half_away(n: f64, digits: i32) -> f64 {
    let factor = 10f64.powi(digits);
    let x = n * factor;
    let rounded = if x >= 0.0 {
        (x + 0.5).floor()
    } else {
        (x - 0.5).ceil()
    };
    rounded / factor
}

fn excel_trunc(n: f64, digits: i32) -> f64 {
    let factor = 10f64.powi(digits);
    (n * factor).trunc() / factor
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
