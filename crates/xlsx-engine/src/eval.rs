//! Seed-scoped evaluator with Excel-like and intentionally naive semantics.

use crate::dates::{
    date_serial, days360, eomonth_serial, isoweeknum, networkdays_count, networkdays_count_mask,
    serial_to_ymd, time_fraction, weekday, weekend_mask_from_code, weekend_mask_from_string,
    weeknum, workday_serial, yearfrac, WEEKEND_SAT_SUN,
};
use crate::parse::{parse, BinOp, Expr, UnaryOp};
use std::collections::{HashMap, HashSet};
use xlsx_types::{
    count_matches, excel_ceiling, excel_ceiling_math, excel_cumipmt, excel_cumprinc, excel_effect,
    excel_floor, excel_floor_math, excel_fv, excel_int, excel_ipmt, excel_mround, excel_nominal,
    excel_nper, excel_num_eq, excel_pduration, excel_pmt, excel_ppmt, excel_pv, excel_rate,
    excel_round, excel_round_15, excel_rri, ArrayMode, CellAddr, CellRef, Criterion, EvalError,
    EvalSpec, EvalTarget, ExcelError, ExcelValue, RangeRef, Workbook,
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
    rng: xlsx_engine_core::XorShift64,
    locals: Vec<xlsx_engine_core::eval::makearray::Local>,
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
            rng: xlsx_engine_core::XorShift64::from_eval_options(&spec.options),
            locals: Vec::new(),
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
            Expr::Apply { callee, args } => self.apply_callee(callee, args, ctx),
            Expr::Missing => Ok(ExcelValue::Empty),
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
        if let Some(v) = lookup_local(&ctx.locals, name) {
            return Ok(v);
        }
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
            "SUMPRODUCT" => self.fn_sumproduct(args, ctx),
            "PRODUCT" => self.fn_agg(args, ctx, AggKind::Product),
            "AVERAGE" => self.fn_agg(args, ctx, AggKind::Average),
            "MIN" => self.fn_agg(args, ctx, AggKind::Min),
            "MAX" => self.fn_agg(args, ctx, AggKind::Max),
            "COUNT" => self.fn_agg(args, ctx, AggKind::Count),
            "COUNTA" => self.fn_agg(args, ctx, AggKind::CountA),
            "COUNTBLANK" => self.fn_agg(args, ctx, AggKind::CountBlank),
            "SUMIF" => self.fn_sumif(args, ctx),
            "COUNTIF" => self.fn_countif(args, ctx),
            "SUMIFS" => self.fn_sumifs(args, ctx),
            "AVERAGEIF" => self.fn_averageif(args, ctx),
            "IF" => self.fn_if(args, ctx),
            "IFS" => self.fn_ifs(args, ctx),
            "IFERROR" => self.fn_iferror(args, ctx),
            "IFNA" => self.fn_ifna(args, ctx),
            "SWITCH" => self.fn_switch(args, ctx),
            "AND" => self.fn_and_or(args, ctx, AndOr::And),
            "OR" => self.fn_and_or(args, ctx, AndOr::Or),
            "XOR" => self.fn_and_or(args, ctx, AndOr::Xor),
            "NOT" => self.fn_not(args, ctx),
            "VLOOKUP" => self.fn_vlookup(args, ctx),
            "HLOOKUP" => self.fn_hlookup(args, ctx),
            "XLOOKUP" => self.fn_xlookup(args, ctx),
            "FILTER" => self.fn_filter(args, ctx),
            "INDEX" => self.fn_index(args, ctx),
            "MATCH" => self.fn_match(args, ctx),
            "CHOOSE" => self.fn_choose(args, ctx),
            "ABS" => self.fn_abs(args, ctx),
            "SIGN" => self.fn_sign(args, ctx),
            "INT" => self.fn_int(args, ctx),
            "TRUNC" => self.fn_trunc(args, ctx),
            "ROUND" => self.fn_round(args, ctx),
            "ROUNDUP" => self.fn_roundup(args, ctx),
            "ROUNDDOWN" => self.fn_rounddown(args, ctx),
            "FLOOR" => self.fn_floor_ceil(args, ctx, true),
            "CEILING" => self.fn_floor_ceil(args, ctx, false),
            "FLOOR.MATH" => self.fn_floor_ceil_math(args, ctx, true),
            "CEILING.MATH" => self.fn_floor_ceil_math(args, ctx, false),
            "MROUND" => self.fn_mround(args, ctx),
            "MOD" => self.fn_mod(args, ctx),
            "SQRT" => self.fn_sqrt(args, ctx),
            "POWER" => self.fn_power(args, ctx),
            "PI" => Ok(ExcelValue::Number(std::f64::consts::PI)),
            "N" => self.fn_n(args, ctx),
            "NA" => Ok(ExcelValue::Error(ExcelError::Na)),
            "TYPE" => self.fn_type(args, ctx),
            "ERROR.TYPE" => self.fn_error_type(args, ctx),
            "ISOMITTED" | "_XLFN.ISOMITTED" => self.fn_isomitted(args, ctx),
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
            "EDATE" => self.fn_edate(args, ctx),
            "EOMONTH" => self.fn_eomonth(args, ctx),
            "NETWORKDAYS" => self.fn_networkdays(args, ctx),
            "WORKDAY" => self.fn_workday(args, ctx),
            "YEAR" => self.fn_ymd(args, ctx, YmdPart::Year),
            "MONTH" => self.fn_ymd(args, ctx, YmdPart::Month),
            "DAY" => self.fn_ymd(args, ctx, YmdPart::Day),
            "WEEKDAY" => self.fn_weekday(args, ctx),
            "WEEKNUM" => self.fn_weeknum(args, ctx),
            "ISOWEEKNUM" => self.fn_isoweeknum(args, ctx),
            "DAYS360" => self.fn_days360(args, ctx),
            "LEFT" => self.fn_left(args, ctx),
            "RIGHT" => self.fn_right(args, ctx),
            "MID" => self.fn_mid(args, ctx),
            "LEN" => self.fn_len(args, ctx),
            "UNICODE" => self.fn_unicode(args, ctx),
            "LOWER" => self.fn_lower(args, ctx),
            "UPPER" => self.fn_upper(args, ctx),
            "PROPER" => self.fn_proper(args, ctx),
            "TRIM" => self.fn_trim(args, ctx),
            "CLEAN" => self.fn_clean(args, ctx),
            "CODE" => self.fn_code(args, ctx),
            "CHAR" => self.fn_char(args, ctx),
            "EXACT" => self.fn_exact(args, ctx),
            "FIND" => self.fn_find(args, ctx),
            "SEARCH" => self.fn_search(args, ctx),
            "VALUE" => self.fn_value(args, ctx),
            "SUBSTITUTE" => self.fn_substitute(args, ctx),
            "TEXT" => self.fn_text(args, ctx),
            "REPLACE" => self.fn_replace(args, ctx),
            "TEXTJOIN" => self.fn_textjoin(args, ctx),
            "CONCAT" => self.fn_concat(args, ctx),
            "REPT" => self.fn_rept(args, ctx),
            "UNICHAR" | "_XLFN.UNICHAR" => self.fn_unichar(args, ctx),
            "NPV" => self.fn_npv(args, ctx),
            "UNIQUE" => self.fn_unique(args, ctx),
            "IRR" => self.fn_irr(args, ctx),
            "TRUE" => Ok(ExcelValue::Bool(true)),
            "FALSE" => Ok(ExcelValue::Bool(false)),
            "PMT" => self.fn_pmt(args, ctx),
            "COUNTIFS" => self.fn_countifs(args, ctx),
            "AVERAGEIFS" => self.fn_averageifs(args, ctx),
            "SORT" => self.fn_sort(args, ctx),
            "SORTBY" => self.fn_sortby(args, ctx),
            "SEQUENCE" => self.fn_sequence(args, ctx),
            "VSTACK" => self.fn_vstack(args, ctx),
            "HSTACK" => self.fn_hstack(args, ctx),
            "TAKE" => self.fn_take(args, ctx),
            "DROP" => self.fn_drop(args, ctx),
            "CHOOSEROWS" => self.fn_chooserows(args, ctx),
            "MAKEARRAY" | "_XLFN.MAKEARRAY" => self.fn_makearray(args, ctx),
            "MAP" | "_XLFN.MAP" => self.fn_map(args, ctx),
            "SCAN" | "_XLFN.SCAN" => self.fn_scan(args, ctx),
            "BYROW" | "_XLFN.BYROW" => self.fn_byrow(args, ctx),
            "REDUCE" | "_XLFN.REDUCE" => self.fn_reduce(args, ctx),
            "BYCOL" | "_XLFN.BYCOL" => self.fn_bycol(args, ctx),
            "LAMBDA" | "_XLFN.LAMBDA" => Ok(ExcelValue::Error(ExcelError::Calc)),
            "LET" | "_XLFN.LET" => self.fn_let(args, ctx),
            "CHOOSECOLS" => self.fn_choosecols(args, ctx),
            "NETWORKDAYS.INTL" => self.fn_networkdays_intl(args, ctx),
            "WORKDAY.INTL" => self.fn_workday_intl(args, ctx),
            "YEARFRAC" => self.fn_yearfrac(args, ctx),
            "TEXTSPLIT" => self.fn_textsplit(args, ctx),
            "TEXTAFTER" => self.fn_textafter(args, ctx),
            "TEXTBEFORE" => self.fn_textbefore(args, ctx),
            "TOCOL" => self.fn_tocol(args, ctx),
            "TOROW" => self.fn_torow(args, ctx),
            "WRAPCOLS" => self.fn_wrapcols(args, ctx),
            "WRAPROWS" => self.fn_wraprows(args, ctx),
            "EXPAND" => self.fn_expand(args, ctx),
            "RANDARRAY" => self.fn_randarray(args, ctx),
            "XNPV" => self.fn_xnpv(args, ctx),
            "XIRR" => self.fn_xirr(args, ctx),
            "MIRR" => self.fn_mirr(args, ctx),
            "FV" => self.fn_fv(args, ctx),
            "PV" => self.fn_pv(args, ctx),
            "NPER" => self.fn_nper(args, ctx),
            "RATE" => self.fn_rate(args, ctx),
            "IPMT" => self.fn_ipmt(args, ctx),
            "PPMT" => self.fn_ppmt(args, ctx),
            "CUMPRINC" => self.fn_cumprinc(args, ctx),
            "CUMIPMT" => self.fn_cumipmt(args, ctx),
            "EFFECT" => self.fn_effect(args, ctx),
            "NOMINAL" => self.fn_nominal(args, ctx),
            "PDURATION" => self.fn_pduration(args, ctx),
            "RRI" => self.fn_rri(args, ctx),
            _ => self.apply_named_lambda(name, args, ctx),
        }
    }

    fn fn_sumproduct(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.is_empty() {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let mut grids = Vec::with_capacity(args.len());
        for arg in args {
            let v = self.eval_array_ctx(arg, ctx)?;
            if let ExcelValue::Error(e) = v {
                return Ok(ExcelValue::Error(e));
            }
            grids.push(v);
        }
        Ok(sumproduct_product_sum(&grids))
    }

    fn eval_array_ctx(&self, expr: &Expr, ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        match expr {
            Expr::Range(r) => self.eval_range(r, ctx),
            Expr::Unary { op, expr } => {
                let v = self.eval_array_ctx(expr, ctx)?;
                Ok(self.map_unary_array(*op, v))
            }
            Expr::Binary { op, left, right } => {
                if *op == BinOp::Intersect {
                    return self.eval_intersect(left, right, ctx);
                }
                let l = self.eval_array_ctx(left, ctx)?;
                let r = self.eval_array_ctx(right, ctx)?;
                Ok(self.zip_binary_array(*op, l, r))
            }
            Expr::Array(rows) => {
                let mut out = Vec::with_capacity(rows.len());
                for row in rows {
                    let mut r = Vec::with_capacity(row.len());
                    for c in row {
                        r.push(self.eval_array_ctx(c, ctx)?);
                    }
                    out.push(r);
                }
                Ok(ExcelValue::Array(out))
            }
            other => self.eval_expr(other, ctx),
        }
    }

    fn map_unary_array(&self, op: UnaryOp, v: ExcelValue) -> ExcelValue {
        match v {
            ExcelValue::Array(rows) => ExcelValue::Array(
                rows.into_iter()
                    .map(|row| {
                        row.into_iter()
                            .map(|c| self.apply_unary_array(op, c))
                            .collect()
                    })
                    .collect(),
            ),
            other => self.apply_unary_array(op, other),
        }
    }

    fn apply_unary_array(&self, op: UnaryOp, v: ExcelValue) -> ExcelValue {
        if let ExcelValue::Error(e) = v {
            return ExcelValue::Error(e);
        }
        match self.as_number(&v) {
            Ok(n) => ExcelValue::Number(match op {
                UnaryOp::Plus => n,
                UnaryOp::Minus => -n,
                UnaryOp::Percent => n / 100.0,
            }),
            Err(e) => ExcelValue::Error(e),
        }
    }

    fn zip_binary_array(&self, op: BinOp, l: ExcelValue, r: ExcelValue) -> ExcelValue {
        if let ExcelValue::Error(e) = l {
            return ExcelValue::Error(e);
        }
        if let ExcelValue::Error(e) = r {
            return ExcelValue::Error(e);
        }
        let (lr, lc) = sumproduct_shape(&l);
        let (rr, rc) = sumproduct_shape(&r);
        let l_scalar = !matches!(l, ExcelValue::Array(_)) || (lr == 1 && lc == 1);
        let r_scalar = !matches!(r, ExcelValue::Array(_)) || (rr == 1 && rc == 1);
        if l_scalar && r_scalar {
            return self.apply_bin_owned(op, l, r);
        }
        let (rows, cols) = if l_scalar {
            (rr, rc)
        } else if r_scalar {
            (lr, lc)
        } else if lr == rr && lc == rc {
            (lr, lc)
        } else {
            return ExcelValue::Error(ExcelError::Value);
        };
        let mut out = Vec::with_capacity(rows);
        for i in 0..rows {
            let mut row = Vec::with_capacity(cols);
            for j in 0..cols {
                let lv = sumproduct_get(&l, i, j, l_scalar).clone();
                let rv = sumproduct_get(&r, i, j, r_scalar).clone();
                row.push(self.apply_bin_owned(op, lv, rv));
            }
            out.push(row);
        }
        ExcelValue::Array(out)
    }

    fn apply_bin_owned(&self, op: BinOp, l: ExcelValue, r: ExcelValue) -> ExcelValue {
        if let ExcelValue::Error(e) = l {
            return ExcelValue::Error(e);
        }
        if let ExcelValue::Error(e) = r {
            return ExcelValue::Error(e);
        }
        match op {
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
            BinOp::Intersect => ExcelValue::Error(ExcelError::Value),
        }
    }

    fn fn_sumifs(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 3 || args.len() % 2 == 0 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let sum = match resolve_sumifs_range(&args[0], ctx) {
            Ok(r) => r,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let mut pairs: Vec<(RangeRef, Criterion)> = Vec::with_capacity(args.len() / 2);
        let mut i = 1;
        while i < args.len() {
            let range = match resolve_sumifs_range(&args[i], ctx) {
                Ok(r) => r,
                Err(e) => return Ok(ExcelValue::Error(e)),
            };
            if range.row_count() != sum.row_count() || range.col_count() != sum.col_count() {
                return Ok(ExcelValue::Error(ExcelError::Value));
            }
            let crit_val = self.eval_scalar(&args[i + 1], ctx)?;
            let criterion = match Criterion::compile(&crit_val) {
                Ok(c) => c,
                Err(e) => return Ok(ExcelValue::Error(e)),
            };
            pairs.push((range, criterion));
            i += 2;
        }

        let sum_sheet = sum
            .sheet
            .clone()
            .unwrap_or_else(|| ctx.current_sheet.clone());
        let height = sum.row_count();
        let width = sum.col_count();
        let mut acc = 0.0;
        for dr in 0..height {
            for dc in 0..width {
                let mut ok = true;
                for (range, criterion) in &pairs {
                    let sheet = range
                        .sheet
                        .clone()
                        .unwrap_or_else(|| ctx.current_sheet.clone());
                    let addr = CellAddr::new(range.start.col + dc, range.start.row + dr);
                    let v = self.eval_cell(
                        &CellRef {
                            sheet: Some(sheet),
                            addr,
                        },
                        ctx,
                    )?;
                    if !criterion.matches(&v) {
                        ok = false;
                        break;
                    }
                }
                if !ok {
                    continue;
                }
                let sum_addr = CellAddr::new(sum.start.col + dc, sum.start.row + dr);
                match self.eval_cell(
                    &CellRef {
                        sheet: Some(sum_sheet.clone()),
                        addr: sum_addr,
                    },
                    ctx,
                )? {
                    ExcelValue::Error(e) => return Ok(ExcelValue::Error(e)),
                    ExcelValue::Number(n) => acc += n,
                    _ => {}
                }
            }
        }
        Ok(ExcelValue::Number(acc))
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
            // LET / LAMBDA locals are values, not worksheet refs: SUM(TRUE) is 1.
            let from_range = match arg {
                Expr::Range(_) | Expr::Cell(_) => true,
                Expr::Name(n) => !xlsx_engine_core::eval::excel_let::is_bound(&ctx.locals, n),
                _ => false,
            };
            let v = self.eval_expr(arg, ctx)?;
            if let Some(err) = acc.fold(&v, from_range, self.semantics) {
                return Ok(ExcelValue::Error(err));
            }
        }
        Ok(acc.finish())
    }

    fn fn_sumif(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 || args.len() > 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let crit_val = self.eval_scalar(&args[1], ctx)?;
        let criterion = match Criterion::compile(&crit_val) {
            Ok(c) => c,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let range = match seed_sumif_range(&args[0], ctx) {
            Ok(r) => r,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let sum_origin = if args.len() == 3 {
            match seed_sumif_range(&args[2], ctx) {
                Ok(r) => r,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            range.clone()
        };
        let crit_sheet = range
            .sheet
            .clone()
            .unwrap_or_else(|| ctx.current_sheet.clone());
        let sum_sheet = sum_origin
            .sheet
            .clone()
            .unwrap_or_else(|| ctx.current_sheet.clone());
        let mut acc = 0.0;
        for dr in 0..range.row_count() {
            for dc in 0..range.col_count() {
                let crit_addr = CellAddr::new(range.start.col + dc, range.start.row + dr);
                let crit_v = self.eval_cell(
                    &CellRef {
                        sheet: Some(crit_sheet.clone()),
                        addr: crit_addr,
                    },
                    ctx,
                )?;
                if !criterion.matches(&crit_v) {
                    continue;
                }
                let sum_addr = CellAddr::new(sum_origin.start.col + dc, sum_origin.start.row + dr);
                let sum_v = self.eval_cell(
                    &CellRef {
                        sheet: Some(sum_sheet.clone()),
                        addr: sum_addr,
                    },
                    ctx,
                )?;
                match sum_v {
                    ExcelValue::Error(e) => return Ok(ExcelValue::Error(e)),
                    ExcelValue::Number(n) => acc += n,
                    _ => {}
                }
            }
        }
        Ok(ExcelValue::Number(acc))
    }

    fn fn_averageif(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 || args.len() > 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let crit_raw = self.eval_scalar(&args[1], ctx)?;
        // Microsoft: empty criteria cell is treated as 0.
        let crit_val = match crit_raw {
            ExcelValue::Empty => ExcelValue::Number(0.0),
            other => other,
        };
        let criterion = match Criterion::compile(&crit_val) {
            Ok(c) => c,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let range = match seed_if_range(&args[0], ctx) {
            Ok(r) => r,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let avg_origin = if args.len() == 3 {
            match seed_if_range(&args[2], ctx) {
                Ok(r) => r,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            range.clone()
        };
        let crit_sheet = range
            .sheet
            .clone()
            .unwrap_or_else(|| ctx.current_sheet.clone());
        let avg_sheet = avg_origin
            .sheet
            .clone()
            .unwrap_or_else(|| ctx.current_sheet.clone());
        let mut sum = 0.0;
        let mut count = 0u64;
        for dr in 0..range.row_count() {
            for dc in 0..range.col_count() {
                let crit_addr = CellAddr::new(range.start.col + dc, range.start.row + dr);
                let crit_v = self.eval_cell(
                    &CellRef {
                        sheet: Some(crit_sheet.clone()),
                        addr: crit_addr,
                    },
                    ctx,
                )?;
                if !criterion.matches(&crit_v) {
                    continue;
                }
                let avg_addr = CellAddr::new(avg_origin.start.col + dc, avg_origin.start.row + dr);
                let avg_v = self.eval_cell(
                    &CellRef {
                        sheet: Some(avg_sheet.clone()),
                        addr: avg_addr,
                    },
                    ctx,
                )?;
                match avg_v {
                    ExcelValue::Error(e) => return Ok(ExcelValue::Error(e)),
                    ExcelValue::Number(n) => {
                        sum += n;
                        count += 1;
                    }
                    _ => {}
                }
            }
        }
        if count == 0 {
            Ok(ExcelValue::Error(ExcelError::Div0))
        } else {
            Ok(ExcelValue::Number(sum / count as f64))
        }
    }

    fn fn_countif(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        if let Some(sheet) = countif_range_sheet(&args[0], ctx) {
            if ctx.spec.workbook.sheet(Some(sheet)).is_err() {
                return Ok(ExcelValue::Error(ExcelError::Ref));
            }
        }
        let crit = Criterion::parse(&self.eval_scalar(&args[1], ctx)?);
        let v = self.eval_expr(&args[0], ctx)?;
        Ok(ExcelValue::Number(count_matches(&v, &crit) as f64))
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

    fn fn_ifs(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.is_empty() || args.len() % 2 == 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let mut first_err = None;
        let mut first_true = None;
        let mut i = 0;
        while i + 1 < args.len() {
            let cond = self.eval_scalar(&args[i], ctx)?;
            let val = self.eval_expr(&args[i + 1], ctx)?;
            if first_err.is_none() {
                if let ExcelValue::Error(e) = cond {
                    first_err = Some(e);
                } else if let ExcelValue::Error(e) = val {
                    first_err = Some(e);
                } else {
                    match self.as_if_cond(&cond) {
                        Ok(true) if first_true.is_none() => first_true = Some(val),
                        Ok(_) => {}
                        Err(e) => first_err = Some(e),
                    }
                }
            }
            i += 2;
        }
        if let Some(e) = first_err {
            return Ok(ExcelValue::Error(e));
        }
        Ok(first_true.unwrap_or(ExcelValue::Error(ExcelError::Na)))
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

    fn fn_unique(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.is_empty() || args.len() > 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let array = self.eval_expr(&args[0], ctx)?;
        if let ExcelValue::Error(e) = array {
            return Ok(ExcelValue::Error(e));
        }
        let by_col = if args.len() >= 2 {
            match self.as_if_cond(&self.eval_scalar(&args[1], ctx)?) {
                Ok(b) => b,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            false
        };
        let exactly_once = if args.len() >= 3 {
            match self.as_if_cond(&self.eval_scalar(&args[2], ctx)?) {
                Ok(b) => b,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            false
        };
        let grid = match unique_to_grid(array) {
            Ok(g) => g,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        Ok(unique_apply_seed(&grid, by_col, exactly_once))
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

    fn fn_switch(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let expr = self.eval_scalar(&args[0], ctx)?;
        if let ExcelValue::Error(e) = expr {
            return Ok(ExcelValue::Error(e));
        }
        let has_default = args.len() % 2 == 0;
        let pair_end = if has_default {
            args.len() - 1
        } else {
            args.len()
        };
        let mut i = 1;
        while i < pair_end {
            let value = self.eval_scalar(&args[i], ctx)?;
            if let ExcelValue::Error(e) = value {
                return Ok(ExcelValue::Error(e));
            }
            if matches!(excel_eq(&expr, &value), ExcelValue::Bool(true)) {
                return self.eval_expr(&args[i + 1], ctx);
            }
            i += 2;
        }
        if has_default {
            self.eval_expr(&args[args.len() - 1], ctx)
        } else {
            Ok(ExcelValue::Error(ExcelError::Na))
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

    fn fn_filter(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 || args.len() > 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let array = self.eval_expr(&args[0], ctx)?;
        let include = self.eval_expr(&args[1], ctx)?;
        let if_empty = if args.len() >= 3 {
            Some(self.eval_expr(&args[2], ctx)?)
        } else {
            None
        };
        Ok(xlsx_engine_core::excel_filter(
            &array,
            &include,
            if_empty.as_ref(),
        ))
    }

    fn fn_xlookup(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 3 || args.len() > 6 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let lookup = self.eval_scalar(&args[0], ctx)?;
        let lookup_array = self.eval_expr(&args[1], ctx)?;
        let return_array = self.eval_expr(&args[2], ctx)?;
        let if_not_found = if args.len() >= 4 {
            Some(self.eval_expr(&args[3], ctx)?)
        } else {
            None
        };
        let match_mode = if args.len() >= 5 {
            Some(self.eval_scalar(&args[4], ctx)?)
        } else {
            None
        };
        let search_mode = if args.len() >= 6 {
            Some(self.eval_scalar(&args[5], ctx)?)
        } else {
            None
        };
        Ok(xlsx_engine_core::excel_xlookup(
            &lookup,
            &lookup_array,
            &return_array,
            if_not_found.as_ref(),
            match_mode.as_ref(),
            search_mode.as_ref(),
        ))
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
        if args.len() != 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        Ok(xlsx_engine_core::excel_abs_value(
            &self.eval_scalar(&args[0], ctx)?,
        ))
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
        self.fn_unary_num(args, ctx, |n| ExcelValue::Number(excel_int(n)))
    }

    fn fn_trunc(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.is_empty() || args.len() > 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let n = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let digits = if args.len() == 2 && !matches!(args[1], Expr::Missing) {
            match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
                Ok(d) => d.trunc() as i32,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            0
        };
        Ok(ExcelValue::Number(xlsx_engine_core::excel_trunc(n, digits)))
    }

    fn fn_floor_ceil(
        &self,
        args: &[Expr],
        ctx: &mut Ctx<'_>,
        floor: bool,
    ) -> Result<ExcelValue, EvalError> {
        if args.len() != 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let n = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let s = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let r = if floor {
            excel_floor(n, s)
        } else {
            excel_ceiling(n, s)
        };
        Ok(match r {
            Ok(v) => ExcelValue::Number(v),
            Err(e) => ExcelValue::Error(e),
        })
    }

    fn fn_floor_ceil_math(
        &self,
        args: &[Expr],
        ctx: &mut Ctx<'_>,
        floor: bool,
    ) -> Result<ExcelValue, EvalError> {
        if args.is_empty() || args.len() > 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let n = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let s = if args.len() >= 2 {
            match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            1.0
        };
        let mode = if args.len() >= 3 {
            match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            0.0
        };
        let r = if floor {
            excel_floor_math(n, s, mode)
        } else {
            excel_ceiling_math(n, s, mode)
        };
        Ok(match r {
            Ok(v) => ExcelValue::Number(v),
            Err(e) => ExcelValue::Error(e),
        })
    }

    fn fn_mround(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let n = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let m = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        Ok(match excel_mround(n, m) {
            Ok(v) => ExcelValue::Number(v),
            Err(e) => ExcelValue::Error(e),
        })
    }

    fn fn_round(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.is_empty() || args.len() > 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let n = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let digits = if args.len() >= 2 {
            match &args[1] {
                Expr::Missing => 0,
                other => match self.as_number(&self.eval_scalar(other, ctx)?) {
                    Ok(d) => d.trunc() as i32,
                    Err(e) => return Ok(ExcelValue::Error(e)),
                },
            }
        } else {
            0
        };
        Ok(ExcelValue::Number(excel_round(n, digits)))
    }

    fn fn_roundup(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.is_empty() || args.len() > 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let n = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let digits = if args.len() == 2 {
            match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
                Ok(d) => d.trunc() as i32,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            0
        };
        Ok(ExcelValue::Number(xlsx_engine_core::excel_roundup(
            n, digits,
        )))
    }

    fn fn_rounddown(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.is_empty() || args.len() > 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let n = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let digits = if args.len() == 2 && !matches!(args[1], Expr::Missing) {
            match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
                Ok(d) => d.trunc() as i32,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            0
        };
        Ok(ExcelValue::Number(xlsx_engine_core::excel_rounddown(
            n, digits,
        )))
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

    fn fn_isomitted(&self, args: &[Expr], ctx: &Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        Ok(ExcelValue::Bool(seed_is_omitted(&args[0], &ctx.locals)))
    }

    fn apply_named_lambda(
        &self,
        name: &str,
        args: &[Expr],
        ctx: &mut Ctx<'_>,
    ) -> Result<ExcelValue, EvalError> {
        if ctx.spec.workbook.defined_name(name).is_err() {
            return Ok(ExcelValue::Error(ExcelError::Name));
        }
        self.apply_callee(&Expr::Name(name.to_string()), args, ctx)
    }

    fn apply_callee(
        &self,
        callee: &Expr,
        args: &[Expr],
        ctx: &mut Ctx<'_>,
    ) -> Result<ExcelValue, EvalError> {
        let (params, body) = match resolve_seed_lambda_any(callee, ctx, 0) {
            Ok(v) => v,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        if args.len() > params.len() {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let base = ctx.locals.len();
        for (i, name) in params.iter().enumerate() {
            if i < args.len() && !matches!(args[i], Expr::Missing) {
                let v = self.eval_expr(&args[i], ctx)?;
                ctx.locals
                    .push(xlsx_engine_core::eval::makearray::Local::provided(
                        name.clone(),
                        v,
                    ));
            } else {
                ctx.locals
                    .push(xlsx_engine_core::eval::makearray::Local::missing(
                        name.clone(),
                    ));
            }
        }
        let out = self.eval_expr(&body, ctx);
        ctx.locals.truncate(base);
        out
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

    fn fn_edate(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let start = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let months = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        match xlsx_engine_core::dates::edate_serial(start, months, ctx.spec.options.date_system) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_eomonth(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let start = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let months = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        match eomonth_serial(start, months, ctx.spec.options.date_system) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_networkdays(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 || args.len() > 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let start_v = self.eval_scalar(&args[0], ctx)?;
        let end_v = self.eval_scalar(&args[1], ctx)?;
        let hol_v = if args.len() == 3 {
            Some(self.eval_expr(&args[2], ctx)?)
        } else {
            None
        };
        let start = match self.as_number(&start_v) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let end = match self.as_number(&end_v) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let mut holidays = Vec::new();
        if let Some(v) = hol_v {
            if let Some(e) = self.collect_holiday_serials(&v, &mut holidays) {
                return Ok(ExcelValue::Error(e));
            }
        }
        match networkdays_count(start, end, &holidays, ctx.spec.options.date_system) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_workday(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 || args.len() > 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let start_v = self.eval_scalar(&args[0], ctx)?;
        let days_v = self.eval_scalar(&args[1], ctx)?;
        let hol_v = if args.len() == 3 {
            Some(self.eval_expr(&args[2], ctx)?)
        } else {
            None
        };
        let start = match self.as_number(&start_v) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let days = match self.as_number(&days_v) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let mut holidays = Vec::new();
        if let Some(v) = hol_v {
            if let Some(e) = self.collect_holiday_serials(&v, &mut holidays) {
                return Ok(ExcelValue::Error(e));
            }
        }
        match workday_serial(start, days, &holidays, ctx.spec.options.date_system) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_weekday(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.is_empty() || args.len() > 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let serial = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let return_type = if args.len() >= 2 {
            match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
                Ok(n) => {
                    if !n.is_finite() {
                        return Ok(ExcelValue::Error(ExcelError::Num));
                    }
                    let t = n.trunc();
                    if t < i32::MIN as f64 || t > i32::MAX as f64 {
                        return Ok(ExcelValue::Error(ExcelError::Num));
                    }
                    t as i32
                }
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            1
        };
        match weekday(serial, return_type, ctx.spec.options.date_system) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_weeknum(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.is_empty() || args.len() > 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let serial = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let return_type = if args.len() >= 2 {
            match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
                Ok(n) => {
                    if !n.is_finite() {
                        return Ok(ExcelValue::Error(ExcelError::Num));
                    }
                    let t = n.trunc();
                    if t < i32::MIN as f64 || t > i32::MAX as f64 {
                        return Ok(ExcelValue::Error(ExcelError::Num));
                    }
                    t as i32
                }
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            1
        };
        match weeknum(serial, return_type, ctx.spec.options.date_system) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_isoweeknum(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let serial = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        match isoweeknum(serial, ctx.spec.options.date_system) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn collect_holiday_serials(&self, v: &ExcelValue, out: &mut Vec<f64>) -> Option<ExcelError> {
        match v {
            ExcelValue::Array(rows) => {
                for row in rows {
                    for c in row {
                        if let Some(e) = self.collect_holiday_serials(c, out) {
                            return Some(e);
                        }
                    }
                }
                None
            }
            ExcelValue::Empty => None,
            ExcelValue::Error(e) => Some(*e),
            other => match self.as_number(other) {
                Ok(n) => {
                    out.push(n);
                    None
                }
                Err(e) => Some(e),
            },
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

    fn fn_left(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.is_empty() || args.len() > 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let s = match self.as_text(&self.eval_scalar(&args[0], ctx)?) {
            Ok(s) => s,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let n = if args.len() >= 2 {
            match &args[1] {
                Expr::Missing => 1,
                other => match self.as_number(&self.eval_scalar(other, ctx)?) {
                    Ok(n) => match xlsx_engine_core::left_trunc_num_chars(n) {
                        Ok(n) => n,
                        Err(e) => return Ok(ExcelValue::Error(e)),
                    },
                    Err(e) => return Ok(ExcelValue::Error(e)),
                },
            }
        } else {
            1
        };
        Ok(ExcelValue::Text(xlsx_engine_core::excel_left_owned(s, n)))
    }

    fn fn_right(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.is_empty() || args.len() > 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let text = match self.as_text(&self.eval_scalar(&args[0], ctx)?) {
            Ok(s) => s,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let n = if args.len() == 2 {
            match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
                Ok(n) => match xlsx_engine_core::right_trunc_num_chars(n) {
                    Ok(n) => n,
                    Err(e) => return Ok(ExcelValue::Error(e)),
                },
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            1
        };
        Ok(ExcelValue::Text(xlsx_engine_core::excel_right_owned(
            text, n,
        )))
    }

    fn fn_mid(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let text = self.eval_scalar(&args[0], ctx)?;
        if let ExcelValue::Error(e) = text {
            return Ok(ExcelValue::Error(e));
        }
        let start_num = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => match xlsx_engine_core::excel_mid_trunc_start_num(n) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            },
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let num_chars = match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
            Ok(n) => match xlsx_engine_core::excel_mid_trunc_num_chars(n) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            },
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        match xlsx_engine_core::excel_mid_value(&text, start_num, num_chars) {
            Ok(s) => Ok(ExcelValue::Text(s)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_len(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        match xlsx_engine_core::excel_len_value(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_unicode(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        match xlsx_engine_core::excel_unicode_value(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_lower(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        match self.as_text(&self.eval_scalar(&args[0], ctx)?) {
            Ok(s) => Ok(ExcelValue::Text(xlsx_engine_core::excel_lower_owned(s))),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_upper(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        match self.as_text(&self.eval_scalar(&args[0], ctx)?) {
            Ok(s) => Ok(ExcelValue::Text(xlsx_engine_core::excel_upper(&s))),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_proper(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        match self.as_text(&self.eval_scalar(&args[0], ctx)?) {
            Ok(s) => Ok(ExcelValue::Text(xlsx_engine_core::excel_proper(&s))),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_trim(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        match self.as_text(&self.eval_scalar(&args[0], ctx)?) {
            Ok(s) => Ok(ExcelValue::Text(xlsx_engine_core::excel_trim(&s))),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_clean(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        match self.as_text(&self.eval_scalar(&args[0], ctx)?) {
            Ok(s) => Ok(ExcelValue::Text(xlsx_engine_core::excel_clean(&s))),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_code(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        match xlsx_engine_core::excel_code_value(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_char(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => match xlsx_engine_core::excel_char(n) {
                Ok(s) => Ok(ExcelValue::Text(s.to_owned())),
                Err(e) => Ok(ExcelValue::Error(e)),
            },
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_exact(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let a = self.eval_scalar(&args[0], ctx)?;
        let b = self.eval_scalar(&args[1], ctx)?;
        match xlsx_engine_core::excel_exact(&a, &b) {
            Ok(eq) => Ok(ExcelValue::Bool(eq)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_rept(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let text = match self.as_text(&self.eval_scalar(&args[0], ctx)?) {
            Ok(s) => s,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let times = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => match xlsx_engine_core::rept_trunc_times(n) {
                Ok(t) => t,
                Err(e) => return Ok(ExcelValue::Error(e)),
            },
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        match xlsx_engine_core::excel_rept(&text, times) {
            Ok(s) => Ok(ExcelValue::Text(s)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_unichar(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 1 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => match xlsx_engine_core::excel_unichar(n) {
                Ok(s) => Ok(ExcelValue::Text(s)),
                Err(e) => Ok(ExcelValue::Error(e)),
            },
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_substitute(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 3 || args.len() > 4 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let text = match self.as_text(&self.eval_scalar(&args[0], ctx)?) {
            Ok(s) => s,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let old_text = match self.as_text(&self.eval_scalar(&args[1], ctx)?) {
            Ok(s) => s,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let new_text = match self.as_text(&self.eval_scalar(&args[2], ctx)?) {
            Ok(s) => s,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let instance = if args.len() == 4 {
            match self.as_number(&self.eval_scalar(&args[3], ctx)?) {
                Ok(n) => {
                    if !n.is_finite() {
                        return Ok(ExcelValue::Error(ExcelError::Value));
                    }
                    let t = n.trunc();
                    if t < 1.0 {
                        return Ok(ExcelValue::Error(ExcelError::Value));
                    }
                    if t > u32::MAX as f64 {
                        return Ok(ExcelValue::Text(text));
                    }
                    Some(t as u32)
                }
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            None
        };
        Ok(ExcelValue::Text(excel_substitute(
            &text, &old_text, &new_text, instance,
        )))
    }

    fn fn_find(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 || args.len() > 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let find_text = self.eval_scalar(&args[0], ctx)?;
        let within_text = self.eval_scalar(&args[1], ctx)?;
        let start_num = if args.len() == 3 && !matches!(args[2], Expr::Missing) {
            match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
                Ok(n) => {
                    if !n.is_finite() {
                        return Ok(ExcelValue::Error(ExcelError::Value));
                    }
                    n.trunc() as i64
                }
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            1
        };
        match xlsx_engine_core::excel_find_value(&find_text, &within_text, start_num) {
            Ok(pos) => Ok(ExcelValue::Number(pos)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_search(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 || args.len() > 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let find_text = self.eval_scalar(&args[0], ctx)?;
        let within_text = self.eval_scalar(&args[1], ctx)?;
        let start_num = if args.len() == 3 && !matches!(args[2], Expr::Missing) {
            match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
                Ok(n) => {
                    if !n.is_finite() {
                        return Ok(ExcelValue::Error(ExcelError::Value));
                    }
                    n.trunc() as i64
                }
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            1
        };
        match xlsx_engine_core::excel_search_value(&find_text, &within_text, start_num) {
            Ok(pos) => Ok(ExcelValue::Number(pos)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
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

    fn fn_replace(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 4 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let old_text = match self.as_text(&self.eval_scalar(&args[0], ctx)?) {
            Ok(s) => s,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let start_num = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => match trunc_start_num(n) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            },
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let num_chars = match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
            Ok(n) => match trunc_num_chars(n) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            },
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let new_text = match self.as_text(&self.eval_scalar(&args[3], ctx)?) {
            Ok(s) => s,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        Ok(ExcelValue::Text(xlsx_engine_core::excel_replace(
            &old_text, start_num, num_chars, &new_text,
        )))
    }

    fn fn_npv(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let rate = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        if !rate.is_finite() {
            return Ok(ExcelValue::Error(ExcelError::Num));
        }
        let mut factor = 1.0;
        let one = 1.0 + rate;
        let mut sum = 0.0;
        for arg in &args[1..] {
            let from_range = matches!(arg, Expr::Range(_) | Expr::Cell(_) | Expr::Name(_));
            let v = self.eval_expr(arg, ctx)?;
            if let Some(e) = npv_feed(&v, from_range, rate, one, &mut factor, &mut sum) {
                return Ok(ExcelValue::Error(e));
            }
        }
        if !sum.is_finite() {
            return Ok(ExcelValue::Error(ExcelError::Num));
        }
        Ok(ExcelValue::Number(sum))
    }

    fn fn_pmt(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 3 || args.len() > 5 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let rate = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let nper = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let pv = match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let fv = if args.len() >= 4 {
            match self.as_number(&self.eval_scalar(&args[3], ctx)?) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            0.0
        };
        let typ = if args.len() >= 5 {
            match self.as_number(&self.eval_scalar(&args[4], ctx)?) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            0.0
        };
        match excel_pmt(rate, nper, pv, fv, typ) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_irr(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.is_empty() || args.len() > 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let from_range = matches!(args[0], Expr::Range(_) | Expr::Cell(_) | Expr::Name(_));
        let values_v = self.eval_expr(&args[0], ctx)?;
        let flows = match collect_irr_cashflows(&values_v, from_range, self.semantics) {
            Ok(v) => v,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let guess = if args.len() == 2 {
            match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            0.1
        };
        match xlsx_engine_core::excel_irr(&flows, guess) {
            Some(r) => Ok(ExcelValue::Number(r)),
            None => Ok(ExcelValue::Error(ExcelError::Num)),
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
            ExcelValue::Text(s) => {
                match xlsx_engine_core::excel_value(&s, ctx.spec.options.date_system) {
                    Ok(n) => Ok(ExcelValue::Number(n)),
                    Err(e) => Ok(ExcelValue::Error(e)),
                }
            }
            ExcelValue::Error(e) => Ok(ExcelValue::Error(e)),
            ExcelValue::Array(_) => Ok(ExcelValue::Error(ExcelError::Value)),
        }
    }

    fn fn_textjoin(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let delim_v = self.eval_expr(&args[0], ctx)?;
        if let ExcelValue::Error(e) = delim_v {
            return Ok(ExcelValue::Error(e));
        }
        let mut delims = Vec::new();
        if let Err(e) = flatten_join_texts(&delim_v, &mut delims, self) {
            return Ok(ExcelValue::Error(e));
        }
        if delims.is_empty() {
            delims.push(String::new());
        }
        let ie = self.eval_scalar(&args[1], ctx)?;
        let ignore_empty = match self.as_if_cond(&ie) {
            Ok(b) => b,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let mut parts = Vec::new();
        for arg in &args[2..] {
            if let Expr::Range(r) = arg {
                let sheet = r.sheet.as_deref().unwrap_or(ctx.current_sheet.as_str());
                if ctx.spec.workbook.sheet(Some(sheet)).is_err() {
                    return Ok(ExcelValue::Error(ExcelError::Ref));
                }
            }
            let v = self.eval_expr(arg, ctx)?;
            if let Err(e) = flatten_join_texts(&v, &mut parts, self) {
                return Ok(ExcelValue::Error(e));
            }
        }
        let kept: Vec<&str> = if ignore_empty {
            parts
                .iter()
                .map(String::as_str)
                .filter(|s| !s.is_empty())
                .collect()
        } else {
            parts.iter().map(String::as_str).collect()
        };
        if kept.is_empty() {
            return Ok(ExcelValue::Text(String::new()));
        }
        let mut out = String::new();
        let mut utf16 = 0usize;
        for (i, part) in kept.iter().enumerate() {
            if i > 0 {
                let d = delims[(i - 1) % delims.len()].as_str();
                utf16 += d.encode_utf16().count();
                if utf16 > 32767 {
                    return Ok(ExcelValue::Error(ExcelError::Value));
                }
                out.push_str(d);
            }
            utf16 += part.encode_utf16().count();
            if utf16 > 32767 {
                return Ok(ExcelValue::Error(ExcelError::Value));
            }
            out.push_str(part);
        }
        Ok(ExcelValue::Text(out))
    }

    fn fn_concat(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.is_empty() {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let mut builder = xlsx_engine_core::ConcatBuilder::new();
        for arg in args {
            if let Expr::Range(r) = arg {
                let sheet = r.sheet.as_deref().unwrap_or(ctx.current_sheet.as_str());
                if ctx.spec.workbook.sheet(Some(sheet)).is_err() {
                    return Ok(ExcelValue::Error(ExcelError::Ref));
                }
            }
            if let Expr::Cell(c) = arg {
                let sheet = c.sheet.as_deref().unwrap_or(ctx.current_sheet.as_str());
                if ctx.spec.workbook.sheet(Some(sheet)).is_err() {
                    return Ok(ExcelValue::Error(ExcelError::Ref));
                }
            }
            let v = self.eval_expr(arg, ctx)?;
            if let Err(e) = xlsx_engine_core::concat_feed_value(&mut builder, &v) {
                return Ok(ExcelValue::Error(e));
            }
        }
        Ok(ExcelValue::Text(builder.finish()))
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

    /// Shared `rate, nper, third, [fv], [type]` coerce for `PV`.
    fn tvm5(
        &self,
        args: &[Expr],
        ctx: &mut Ctx<'_>,
    ) -> Result<Result<(f64, f64, f64, f64, f64), ExcelError>, EvalError> {
        if args.len() < 3 || args.len() > 5 {
            return Ok(Err(ExcelError::Value));
        }
        let rate = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(Err(e)),
        };
        let nper = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(Err(e)),
        };
        let third = match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(Err(e)),
        };
        let fv = if args.len() >= 4 {
            match self.as_number(&self.eval_scalar(&args[3], ctx)?) {
                Ok(n) => n,
                Err(e) => return Ok(Err(e)),
            }
        } else {
            0.0
        };
        let typ = if args.len() >= 5 {
            match self.as_number(&self.eval_scalar(&args[4], ctx)?) {
                Ok(n) => n,
                Err(e) => return Ok(Err(e)),
            }
        } else {
            0.0
        };
        Ok(Ok((rate, nper, third, fv, typ)))
    }

    fn parse_weekend_arg(&self, v: &ExcelValue) -> Result<u8, ExcelError> {
        match v {
            ExcelValue::Error(e) => Err(*e),
            ExcelValue::Text(s) => weekend_mask_from_string(s),
            other => {
                let n = self.as_number(other)?;
                if !n.is_finite() {
                    return Err(ExcelError::Num);
                }
                let t = n.trunc();
                if t < i32::MIN as f64 || t > i32::MAX as f64 {
                    return Err(ExcelError::Num);
                }
                weekend_mask_from_code(t as i32)
            }
        }
    }

    fn fn_fv(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 3 || args.len() > 5 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let rate = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let nper = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let pmt = match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let pv = if args.len() >= 4 {
            match self.as_number(&self.eval_scalar(&args[3], ctx)?) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            0.0
        };
        let typ = if args.len() >= 5 {
            match self.as_number(&self.eval_scalar(&args[4], ctx)?) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            0.0
        };
        match excel_fv(rate, nper, pmt, pv, typ) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_pv(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        match self.tvm5(args, ctx)? {
            Ok((rate, nper, pmt, fv, typ)) => match excel_pv(rate, nper, pmt, fv, typ) {
                Ok(n) => Ok(ExcelValue::Number(n)),
                Err(e) => Ok(ExcelValue::Error(e)),
            },
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_nper(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 3 || args.len() > 5 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let rate = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let pmt = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let pv = match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let fv = if args.len() >= 4 {
            match self.as_number(&self.eval_scalar(&args[3], ctx)?) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            0.0
        };
        let typ = if args.len() >= 5 {
            match self.as_number(&self.eval_scalar(&args[4], ctx)?) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            0.0
        };
        match excel_nper(rate, pmt, pv, fv, typ) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_sort(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.is_empty() || args.len() > 4 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let array = self.eval_expr(&args[0], ctx)?;
        if let ExcelValue::Error(e) = array {
            return Ok(ExcelValue::Error(e));
        }
        let sort_index = if args.len() >= 2 {
            Some(self.eval_scalar(&args[1], ctx)?)
        } else {
            None
        };
        let sort_order = if args.len() >= 3 {
            Some(self.eval_scalar(&args[2], ctx)?)
        } else {
            None
        };
        let by_col = if args.len() >= 4 {
            Some(self.eval_scalar(&args[3], ctx)?)
        } else {
            None
        };
        Ok(xlsx_engine_core::excel_sort(
            &array,
            sort_index.as_ref(),
            sort_order.as_ref(),
            by_col.as_ref(),
        ))
    }

    fn fn_countifs(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 || args.len() % 2 != 0 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let first = match resolve_sumifs_range(&args[0], ctx) {
            Ok(r) => r,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let mut pairs: Vec<(RangeRef, Criterion)> = Vec::with_capacity(args.len() / 2);
        let mut i = 0;
        while i < args.len() {
            let range = match resolve_sumifs_range(&args[i], ctx) {
                Ok(r) => r,
                Err(e) => return Ok(ExcelValue::Error(e)),
            };
            if range.row_count() != first.row_count() || range.col_count() != first.col_count() {
                return Ok(ExcelValue::Error(ExcelError::Value));
            }
            let crit_val = self.eval_scalar(&args[i + 1], ctx)?;
            pairs.push((range, Criterion::parse(&crit_val)));
            i += 2;
        }

        let sheets: Vec<String> = pairs
            .iter()
            .map(|(r, _)| r.sheet.clone().unwrap_or_else(|| ctx.current_sheet.clone()))
            .collect();
        if sheets
            .iter()
            .any(|s| ctx.spec.workbook.sheet(Some(s)).is_err())
        {
            return Ok(ExcelValue::Error(ExcelError::Ref));
        }

        let height = first.row_count();
        let width = first.col_count();
        let mut count = 0u64;
        for dr in 0..height {
            for dc in 0..width {
                let mut ok = true;
                for ((range, criterion), sheet) in pairs.iter().zip(sheets.iter()) {
                    let addr = CellAddr::new(range.start.col + dc, range.start.row + dr);
                    let v = self.eval_cell(
                        &CellRef {
                            sheet: Some(sheet.clone()),
                            addr,
                        },
                        ctx,
                    )?;
                    if !criterion.matches(&v) {
                        ok = false;
                        break;
                    }
                }
                if ok {
                    count += 1;
                }
            }
        }
        Ok(ExcelValue::Number(count as f64))
    }

    fn fn_averageifs(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 3 || args.len() % 2 == 0 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let avg = match resolve_sumifs_range(&args[0], ctx) {
            Ok(r) => r,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let mut pairs: Vec<(RangeRef, Criterion)> = Vec::with_capacity(args.len() / 2);
        let mut i = 1;
        while i < args.len() {
            let range = match resolve_sumifs_range(&args[i], ctx) {
                Ok(r) => r,
                Err(e) => return Ok(ExcelValue::Error(e)),
            };
            if range.row_count() != avg.row_count() || range.col_count() != avg.col_count() {
                return Ok(ExcelValue::Error(ExcelError::Value));
            }
            let crit_val = self.eval_scalar(&args[i + 1], ctx)?;
            let criterion = match Criterion::compile(&crit_val) {
                Ok(c) => c,
                Err(e) => return Ok(ExcelValue::Error(e)),
            };
            pairs.push((range, criterion));
            i += 2;
        }

        let avg_sheet = avg
            .sheet
            .clone()
            .unwrap_or_else(|| ctx.current_sheet.clone());
        let height = avg.row_count();
        let width = avg.col_count();
        let mut sum = 0.0;
        let mut count = 0u64;
        for dr in 0..height {
            for dc in 0..width {
                let mut ok = true;
                for (range, criterion) in &pairs {
                    let sheet = range
                        .sheet
                        .clone()
                        .unwrap_or_else(|| ctx.current_sheet.clone());
                    let addr = CellAddr::new(range.start.col + dc, range.start.row + dr);
                    let v = self.eval_cell(
                        &CellRef {
                            sheet: Some(sheet),
                            addr,
                        },
                        ctx,
                    )?;
                    if !criterion.matches(&v) {
                        ok = false;
                        break;
                    }
                }
                if !ok {
                    continue;
                }
                let avg_addr = CellAddr::new(avg.start.col + dc, avg.start.row + dr);
                match self.eval_cell(
                    &CellRef {
                        sheet: Some(avg_sheet.clone()),
                        addr: avg_addr,
                    },
                    ctx,
                )? {
                    ExcelValue::Error(e) => return Ok(ExcelValue::Error(e)),
                    ExcelValue::Number(n) => {
                        sum += n;
                        count += 1;
                    }
                    _ => {}
                }
            }
        }
        if count == 0 {
            Ok(ExcelValue::Error(ExcelError::Div0))
        } else {
            Ok(ExcelValue::Number(sum / count as f64))
        }
    }

    fn fn_days360(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 || args.len() > 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let start = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let end = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let european = if args.len() >= 3 {
            match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
                Ok(n) => {
                    if !n.is_finite() {
                        return Ok(ExcelValue::Error(ExcelError::Num));
                    }
                    n != 0.0
                }
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            false
        };
        match days360(start, end, european, ctx.spec.options.date_system) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_yearfrac(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 || args.len() > 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let start = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let end = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let basis = if args.len() >= 3 {
            match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
                Ok(n) => {
                    if !n.is_finite() {
                        return Ok(ExcelValue::Error(ExcelError::Num));
                    }
                    let t = n.trunc();
                    if t < i32::MIN as f64 || t > i32::MAX as f64 {
                        return Ok(ExcelValue::Error(ExcelError::Num));
                    }
                    t as i32
                }
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            0
        };
        match yearfrac(start, end, basis, ctx.spec.options.date_system) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_sortby(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let array = self.eval_expr(&args[0], ctx)?;
        let mut owned: Vec<(ExcelValue, Option<ExcelValue>)> = Vec::new();
        let mut i = 1;
        while i < args.len() {
            let by = self.eval_expr(&args[i], ctx)?;
            i += 1;
            let order = if i < args.len() {
                let o = self.eval_scalar(&args[i], ctx)?;
                i += 1;
                Some(o)
            } else {
                None
            };
            owned.push((by, order));
        }
        if owned.len() > xlsx_engine_core::MAX_SORT_KEYS {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let refs: Vec<(&ExcelValue, Option<&ExcelValue>)> = owned
            .iter()
            .map(|(by, order)| (by, order.as_ref()))
            .collect();
        Ok(xlsx_engine_core::excel_sortby(&array, &refs))
    }

    fn fn_rate(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 3 || args.len() > 6 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let nper = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let pmt = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let pv = match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let fv = if args.len() >= 4 {
            match self.as_number(&self.eval_scalar(&args[3], ctx)?) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            0.0
        };
        let typ = if args.len() >= 5 {
            match self.as_number(&self.eval_scalar(&args[4], ctx)?) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            0.0
        };
        let guess = if args.len() >= 6 {
            match self.as_number(&self.eval_scalar(&args[5], ctx)?) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            0.1
        };
        match excel_rate(nper, pmt, pv, fv, typ, guess) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_tocol(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.is_empty() || args.len() > 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let array = self.eval_expr(&args[0], ctx)?;
        if let ExcelValue::Error(e) = array {
            return Ok(ExcelValue::Error(e));
        }
        let ignore = if args.len() >= 2 {
            match xlsx_engine_core::parse_tocol_ignore(&self.eval_scalar(&args[1], ctx)?) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            0
        };
        let scan_by_col = if args.len() >= 3 {
            match self.as_if_cond(&self.eval_scalar(&args[2], ctx)?) {
                Ok(b) => b,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            false
        };
        Ok(xlsx_engine_core::tocol_apply(&array, ignore, scan_by_col))
    }

    fn fn_torow(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.is_empty() || args.len() > 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let array = self.eval_expr(&args[0], ctx)?;
        let ignore = if args.len() >= 2 {
            Some(self.eval_scalar(&args[1], ctx)?)
        } else {
            None
        };
        let scan_by_col = if args.len() >= 3 {
            Some(self.eval_scalar(&args[2], ctx)?)
        } else {
            None
        };
        Ok(xlsx_engine_core::excel_torow(
            &array,
            ignore.as_ref(),
            scan_by_col.as_ref(),
        ))
    }

    fn fn_ipmt(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 4 || args.len() > 6 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let rate = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let per = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let nper = match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let pv = match self.as_number(&self.eval_scalar(&args[3], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let fv = if args.len() >= 5 {
            match self.as_number(&self.eval_scalar(&args[4], ctx)?) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            0.0
        };
        let typ = if args.len() >= 6 {
            match self.as_number(&self.eval_scalar(&args[5], ctx)?) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            0.0
        };
        match excel_ipmt(rate, per, nper, pv, fv, typ) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_xnpv(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let rate = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        if !rate.is_finite() {
            return Ok(ExcelValue::Error(ExcelError::Num));
        }
        let from_values = matches!(args[1], Expr::Range(_) | Expr::Cell(_) | Expr::Name(_));
        let values_v = self.eval_expr(&args[1], ctx)?;
        let values = match xlsx_engine_core::collect_xnpv_series(&values_v, from_values) {
            Ok(v) => v,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let from_dates = matches!(args[2], Expr::Range(_) | Expr::Cell(_) | Expr::Name(_));
        let dates_v = self.eval_expr(&args[2], ctx)?;
        let dates_raw = match xlsx_engine_core::collect_xnpv_series(&dates_v, from_dates) {
            Ok(v) => v,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        if values.len() != dates_raw.len() {
            return Ok(ExcelValue::Error(ExcelError::Num));
        }
        let system = ctx.spec.options.date_system;
        let mut dates = Vec::with_capacity(dates_raw.len());
        for n in dates_raw {
            match xlsx_engine_core::xnpv_date_serial_trunc(n, system) {
                Ok(s) => dates.push(s),
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        }
        match xlsx_engine_core::excel_xnpv(rate, &values, &dates) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_ppmt(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 4 || args.len() > 6 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let rate = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let per = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let nper = match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let pv = match self.as_number(&self.eval_scalar(&args[3], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let fv = if args.len() >= 5 {
            match self.as_number(&self.eval_scalar(&args[4], ctx)?) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            0.0
        };
        let typ = if args.len() >= 6 {
            match self.as_number(&self.eval_scalar(&args[5], ctx)?) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            0.0
        };
        match excel_ppmt(rate, per, nper, pv, fv, typ) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_workday_intl(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 || args.len() > 4 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let start_v = self.eval_scalar(&args[0], ctx)?;
        let days_v = self.eval_scalar(&args[1], ctx)?;
        let weekend_v = if args.len() >= 3 {
            Some(self.eval_scalar(&args[2], ctx)?)
        } else {
            None
        };
        let hol_v = if args.len() == 4 {
            Some(self.eval_expr(&args[3], ctx)?)
        } else {
            None
        };
        let start = match self.as_number(&start_v) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let days = match self.as_number(&days_v) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let weekend = match xlsx_engine_core::dates::parse_weekend_mask(weekend_v.as_ref()) {
            Ok(m) => m,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let mut holidays = Vec::new();
        if let Some(v) = hol_v {
            if let Some(e) = self.collect_holiday_serials(&v, &mut holidays) {
                return Ok(ExcelValue::Error(e));
            }
        }
        match xlsx_engine_core::workday_serial_intl(
            start,
            days,
            weekend,
            &holidays,
            ctx.spec.options.date_system,
        ) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_networkdays_intl(
        &self,
        args: &[Expr],
        ctx: &mut Ctx<'_>,
    ) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 || args.len() > 4 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let start_v = self.eval_scalar(&args[0], ctx)?;
        let end_v = self.eval_scalar(&args[1], ctx)?;
        let weekend_v = if args.len() >= 3 {
            Some(self.eval_scalar(&args[2], ctx)?)
        } else {
            None
        };
        let hol_v = if args.len() == 4 {
            Some(self.eval_expr(&args[3], ctx)?)
        } else {
            None
        };
        let start = match self.as_number(&start_v) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let end = match self.as_number(&end_v) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let mask = match weekend_v {
            None => WEEKEND_SAT_SUN,
            Some(v) => match self.parse_weekend_arg(&v) {
                Ok(m) => m,
                Err(e) => return Ok(ExcelValue::Error(e)),
            },
        };
        let mut holidays = Vec::new();
        if let Some(v) = hol_v {
            if let Some(e) = self.collect_holiday_serials(&v, &mut holidays) {
                return Ok(ExcelValue::Error(e));
            }
        }
        match networkdays_count_mask(start, end, mask, &holidays, ctx.spec.options.date_system) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_cumprinc(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 6 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let rate = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let nper = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let pv = match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let start = match self.as_number(&self.eval_scalar(&args[3], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let end = match self.as_number(&self.eval_scalar(&args[4], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let typ = match self.as_number(&self.eval_scalar(&args[5], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        match excel_cumprinc(rate, nper, pv, start, end, typ) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_cumipmt(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 6 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let rate = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let nper = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let pv = match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let start = match self.as_number(&self.eval_scalar(&args[3], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let end = match self.as_number(&self.eval_scalar(&args[4], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let typ = match self.as_number(&self.eval_scalar(&args[5], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        match excel_cumipmt(rate, nper, pv, start, end, typ) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_xirr(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 || args.len() > 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let values_ref = matches!(args[0], Expr::Range(_) | Expr::Cell(_) | Expr::Name(_));
        let values_v = self.eval_expr(&args[0], ctx)?;
        let values = match xlsx_engine_core::collect_xirr_series(&values_v, values_ref) {
            Ok(v) => v,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let dates_ref = matches!(args[1], Expr::Range(_) | Expr::Cell(_) | Expr::Name(_));
        let dates_v = self.eval_expr(&args[1], ctx)?;
        let dates_raw = match xlsx_engine_core::collect_xirr_series(&dates_v, dates_ref) {
            Ok(v) => v,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let system = ctx.spec.options.date_system;
        let mut dates = Vec::with_capacity(dates_raw.len());
        for n in dates_raw {
            match xlsx_engine_core::xirr_date_serial_trunc(n, system) {
                Ok(s) => dates.push(s),
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        }
        let guess = if args.len() == 3 {
            match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            0.1
        };
        match xlsx_engine_core::excel_xirr(&values, &dates, guess) {
            Some(r) => Ok(ExcelValue::Number(r)),
            None => Ok(ExcelValue::Error(ExcelError::Num)),
        }
    }

    fn fn_sequence(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.is_empty() || args.len() > 4 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let rows = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let columns = if args.len() >= 2 {
            match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            1.0
        };
        let start = if args.len() >= 3 {
            match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            1.0
        };
        let step = if args.len() >= 4 {
            match self.as_number(&self.eval_scalar(&args[3], ctx)?) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            1.0
        };
        Ok(xlsx_engine_core::excel_sequence(rows, columns, start, step))
    }

    fn fn_vstack(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.is_empty() {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            let v = self.eval_expr(arg, ctx)?;
            let v = match arg {
                Expr::Cell(_) | Expr::Range(_) | Expr::Array(_) | Expr::Name(_) => match v {
                    ExcelValue::Error(e) => ExcelValue::Array(vec![vec![ExcelValue::Error(e)]]),
                    other => other,
                },
                _ => v,
            };
            values.push(v);
        }
        Ok(xlsx_engine_core::excel_vstack_owned(values))
    }

    fn fn_wrapcols(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 || args.len() > 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let vector = self.eval_expr(&args[0], ctx)?;
        let wrap_count = self.eval_scalar(&args[1], ctx)?;
        let pad_with = if args.len() >= 3 {
            Some(self.eval_scalar(&args[2], ctx)?)
        } else {
            None
        };
        Ok(xlsx_engine_core::excel_wrapcols(
            &vector,
            &wrap_count,
            pad_with.as_ref(),
        ))
    }

    fn fn_wraprows(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 || args.len() > 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let vector = self.eval_expr(&args[0], ctx)?;
        let wrap_v = self.eval_scalar(&args[1], ctx)?;
        let pad = if args.len() >= 3 {
            self.eval_scalar(&args[2], ctx)?
        } else {
            ExcelValue::Error(ExcelError::Na)
        };
        if let ExcelValue::Error(e) = vector {
            return Ok(ExcelValue::Error(e));
        }
        let wrap_count = match self.as_number(&wrap_v) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        Ok(xlsx_engine_core::excel_wraprows(&vector, wrap_count, &pad))
    }

    fn fn_hstack(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.is_empty() {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            values.push(self.eval_expr(arg, ctx)?);
        }
        Ok(xlsx_engine_core::excel_hstack(&values))
    }

    fn fn_take(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.is_empty() || args.len() > 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let array = self.eval_expr(&args[0], ctx)?;
        let rows = if args.len() >= 2 {
            Some(self.eval_scalar(&args[1], ctx)?)
        } else {
            None
        };
        let cols = if args.len() >= 3 {
            Some(self.eval_scalar(&args[2], ctx)?)
        } else {
            None
        };
        Ok(xlsx_engine_core::excel_take(
            &array,
            rows.as_ref(),
            cols.as_ref(),
        ))
    }

    fn fn_choosecols(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let array = self.eval_expr(&args[0], ctx)?;
        let mut col_nums = Vec::with_capacity(args.len() - 1);
        for arg in &args[1..] {
            col_nums.push(self.eval_expr(arg, ctx)?);
        }
        Ok(xlsx_engine_core::excel_choosecols(&array, &col_nums))
    }

    fn fn_drop(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 || args.len() > 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        if let Expr::Range(range) = &args[0] {
            let sheet = range
                .sheet
                .clone()
                .unwrap_or_else(|| ctx.current_sheet.clone());
            if ctx.spec.workbook.sheet(Some(&sheet)).is_err() {
                return Ok(ExcelValue::Error(ExcelError::Ref));
            }
        }
        let array = self.eval_expr(&args[0], ctx)?;
        if let ExcelValue::Error(e) = array {
            return Ok(ExcelValue::Error(e));
        }
        let rows = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let cols = if args.len() >= 3 {
            match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
                Ok(n) => n,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            0.0
        };
        Ok(xlsx_engine_core::excel_drop(&array, rows, cols))
    }

    fn fn_expand(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.is_empty() || args.len() > 4 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let array = self.eval_expr(&args[0], ctx)?;
        let rows_v = if args.len() >= 2 {
            Some(self.eval_scalar(&args[1], ctx)?)
        } else {
            None
        };
        let cols_v = if args.len() >= 3 {
            Some(self.eval_scalar(&args[2], ctx)?)
        } else {
            None
        };
        let pad = if args.len() >= 4 {
            self.eval_scalar(&args[3], ctx)?
        } else {
            ExcelValue::Error(ExcelError::Na)
        };
        if let ExcelValue::Error(e) = array {
            return Ok(ExcelValue::Error(e));
        }
        let rows = match rows_v
            .as_ref()
            .map(xlsx_engine_core::expand_dim_from_value)
            .transpose()
        {
            Ok(n) => n.flatten(),
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let columns = match cols_v
            .as_ref()
            .map(xlsx_engine_core::expand_dim_from_value)
            .transpose()
        {
            Ok(n) => n.flatten(),
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        Ok(xlsx_engine_core::excel_expand(&array, rows, columns, &pad))
    }

    fn fn_chooserows(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let array = self.eval_expr(&args[0], ctx)?;
        let mut row_nums = Vec::with_capacity(args.len() - 1);
        for arg in &args[1..] {
            row_nums.push(self.eval_expr(arg, ctx)?);
        }
        Ok(xlsx_engine_core::excel_chooserows(&array, &row_nums))
    }

    fn fn_mirr(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let from_range = matches!(args[0], Expr::Range(_) | Expr::Cell(_) | Expr::Name(_));
        let values_v = self.eval_expr(&args[0], ctx)?;
        let flows = match collect_irr_cashflows(&values_v, from_range, self.semantics) {
            Ok(v) => v,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let finance_rate = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let reinvest_rate = match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        match xlsx_engine_core::excel_mirr(&flows, finance_rate, reinvest_rate) {
            Ok(r) => Ok(ExcelValue::Number(r)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_effect(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let nominal = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let npery = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        match excel_effect(nominal, npery) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_textsplit(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.is_empty() || args.len() > 6 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let text = self.eval_scalar(&args[0], ctx)?;
        let col = if args.len() >= 2 && !matches!(args[1], Expr::Missing) {
            Some(self.eval_expr(&args[1], ctx)?)
        } else {
            None
        };
        let row = if args.len() >= 3 && !matches!(args[2], Expr::Missing) {
            Some(self.eval_expr(&args[2], ctx)?)
        } else {
            None
        };
        let ignore = if args.len() >= 4 && !matches!(args[3], Expr::Missing) {
            Some(self.eval_scalar(&args[3], ctx)?)
        } else {
            None
        };
        let mode = if args.len() >= 5 && !matches!(args[4], Expr::Missing) {
            Some(self.eval_scalar(&args[4], ctx)?)
        } else {
            None
        };
        let pad = if args.len() >= 6 && !matches!(args[5], Expr::Missing) {
            Some(self.eval_expr(&args[5], ctx)?)
        } else {
            None
        };
        Ok(xlsx_engine_core::excel_textsplit_apply(
            &text,
            col.as_ref(),
            row.as_ref(),
            ignore.as_ref(),
            mode.as_ref(),
            pad.as_ref(),
        ))
    }

    fn fn_textafter(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 || args.len() > 6 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let text = match self.as_text(&self.eval_scalar(&args[0], ctx)?) {
            Ok(s) => s,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let delim_v = self.eval_expr(&args[1], ctx)?;
        let mut delims = Vec::new();
        if let Err(e) = collect_textafter_delims(&delim_v, &mut delims) {
            return Ok(ExcelValue::Error(e));
        }
        let delim_refs: Vec<&str> = delims.iter().map(String::as_str).collect();
        let instance_num = if args.len() >= 3 {
            match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
                Ok(n) => {
                    if !n.is_finite() {
                        return Ok(ExcelValue::Error(ExcelError::Value));
                    }
                    n.trunc() as i64
                }
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            1
        };
        let ignore_case = if args.len() >= 4 {
            match self.as_if_cond(&self.eval_scalar(&args[3], ctx)?) {
                Ok(b) => b,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            false
        };
        let match_end = if args.len() >= 5 {
            match self.as_if_cond(&self.eval_scalar(&args[4], ctx)?) {
                Ok(b) => b,
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            false
        };
        let if_not_found = if args.len() >= 6 {
            Some(self.eval_expr(&args[5], ctx)?)
        } else {
            None
        };
        match xlsx_engine_core::excel_textafter(
            &text,
            &delim_refs,
            instance_num,
            ignore_case,
            match_end,
        ) {
            Ok(s) => Ok(ExcelValue::Text(s)),
            Err(ExcelError::Na) => match if_not_found {
                Some(v) => Ok(v),
                None => Ok(ExcelValue::Error(ExcelError::Na)),
            },
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_textbefore(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 || args.len() > 6 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let text = match self.as_text(&self.eval_scalar(&args[0], ctx)?) {
            Ok(s) => s,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let delim_v = self.eval_expr(&args[1], ctx)?;
        let mut delims = Vec::new();
        if let Err(e) = flatten_textbefore_delims(&delim_v, &mut delims, self) {
            return Ok(ExcelValue::Error(e));
        }
        let instance_num = if args.len() >= 3 {
            match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
                Ok(n) => {
                    if !n.is_finite() {
                        return Ok(ExcelValue::Error(ExcelError::Value));
                    }
                    n.trunc() as i64
                }
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            1
        };
        let match_mode = if args.len() >= 4 {
            match self.as_number(&self.eval_scalar(&args[3], ctx)?) {
                Ok(n) => {
                    if !n.is_finite() {
                        return Ok(ExcelValue::Error(ExcelError::Value));
                    }
                    let t = n.trunc();
                    if t != 0.0 && t != 1.0 {
                        return Ok(ExcelValue::Error(ExcelError::Value));
                    }
                    t as i64
                }
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            0
        };
        let match_end = if args.len() >= 5 {
            match self.as_number(&self.eval_scalar(&args[4], ctx)?) {
                Ok(n) => {
                    if !n.is_finite() {
                        return Ok(ExcelValue::Error(ExcelError::Value));
                    }
                    let t = n.trunc();
                    if t != 0.0 && t != 1.0 {
                        return Ok(ExcelValue::Error(ExcelError::Value));
                    }
                    t as i64
                }
                Err(e) => return Ok(ExcelValue::Error(e)),
            }
        } else {
            0
        };
        let if_not_found = if args.len() >= 6 {
            Some(self.eval_expr(&args[5], ctx)?)
        } else {
            None
        };
        let delim_refs: Vec<&str> = delims.iter().map(String::as_str).collect();
        match xlsx_engine_core::excel_textbefore(
            &text,
            &delim_refs,
            instance_num,
            match_mode == 1,
            match_end == 1,
        ) {
            Ok(s) => Ok(ExcelValue::Text(s)),
            Err(ExcelError::Na) => Ok(if_not_found.unwrap_or(ExcelValue::Error(ExcelError::Na))),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_nominal(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let effect_rate = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let npery = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        match excel_nominal(effect_rate, npery) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_pduration(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let rate = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let pv = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let fv = match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        match excel_pduration(rate, pv, fv) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }

    fn fn_randarray(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() > 5 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let mut vals: [Option<ExcelValue>; 5] = [None, None, None, None, None];
        for (i, arg) in args.iter().enumerate() {
            vals[i] = Some(self.eval_scalar(arg, ctx)?);
        }
        Ok(xlsx_engine_core::excel_randarray(
            vals[0].as_ref(),
            vals[1].as_ref(),
            vals[2].as_ref(),
            vals[3].as_ref(),
            vals[4].as_ref(),
            &mut ctx.rng,
        ))
    }

    fn fn_makearray(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let rows_v = self.eval_scalar(&args[0], ctx)?;
        if let ExcelValue::Error(e) = rows_v {
            return Ok(ExcelValue::Error(e));
        }
        let cols_v = self.eval_scalar(&args[1], ctx)?;
        if let ExcelValue::Error(e) = cols_v {
            return Ok(ExcelValue::Error(e));
        }
        let (rows, cols) = match xlsx_engine_core::eval::makearray::dims(&rows_v, &cols_v) {
            Ok(d) => d,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let (row_p, col_p, body) = match resolve_seed_lambda(&args[2], ctx, 0) {
            Ok(l) => l,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let base = ctx.locals.len();
        ctx.locals
            .push(xlsx_engine_core::eval::makearray::Local::provided(
                row_p,
                ExcelValue::Number(1.0),
            ));
        ctx.locals
            .push(xlsx_engine_core::eval::makearray::Local::provided(
                col_p,
                ExcelValue::Number(1.0),
            ));
        let mut grid = Vec::with_capacity(rows);
        for r in 1..=rows {
            ctx.locals[base].value = ExcelValue::Number(r as f64);
            let mut row = Vec::with_capacity(cols);
            for c in 1..=cols {
                ctx.locals[base + 1].value = ExcelValue::Number(c as f64);
                let v = self.eval_expr(&body, ctx)?;
                row.push(match v {
                    ExcelValue::Array(_) => ExcelValue::Error(ExcelError::Calc),
                    other => other,
                });
            }
            grid.push(row);
        }
        ctx.locals.truncate(base);
        Ok(ExcelValue::Array(grid))
    }

    fn fn_map(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() < 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let (lambda_expr, array_exprs) = args.split_last().unwrap();
        let mut arrays = Vec::with_capacity(array_exprs.len());
        for expr in array_exprs {
            let v = self.eval_expr(expr, ctx)?;
            if let ExcelValue::Error(e) = v {
                return Ok(ExcelValue::Error(e));
            }
            arrays.push(v);
        }
        let (params, body) = match resolve_seed_lambda_any(lambda_expr, ctx, 0) {
            Ok(l) => l,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        if params.len() != arrays.len() {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let (rows, cols) = match xlsx_engine_core::eval::map::output_shape(&arrays) {
            Ok(s) => s,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let base = ctx.locals.len();
        for p in &params {
            ctx.locals
                .push(xlsx_engine_core::eval::makearray::Local::provided(
                    p.clone(),
                    ExcelValue::Empty,
                ));
        }
        let mut grid = Vec::with_capacity(rows);
        for r in 0..rows {
            let mut row = Vec::with_capacity(cols);
            for c in 0..cols {
                match xlsx_engine_core::eval::map::pair_at(&arrays, r, c) {
                    None => row.push(ExcelValue::Error(ExcelError::Na)),
                    Some(vals) => {
                        for (i, v) in vals.into_iter().enumerate() {
                            ctx.locals[base + i].value = v.clone();
                        }
                        let v = self.eval_expr(&body, ctx)?;
                        row.push(match v {
                            ExcelValue::Array(_) => ExcelValue::Error(ExcelError::Calc),
                            other => other,
                        });
                    }
                }
            }
            grid.push(row);
        }
        ctx.locals.truncate(base);
        Ok(ExcelValue::Array(grid))
    }

    fn fn_scan(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        let (initial_expr, array_expr, lambda_expr) = match args.len() {
            2 => (None, &args[0], &args[1]),
            3 if matches!(args[0], Expr::Missing) => (None, &args[1], &args[2]),
            3 => (Some(&args[0]), &args[1], &args[2]),
            _ => return Ok(ExcelValue::Error(ExcelError::Value)),
        };
        let initial = if let Some(expr) = initial_expr {
            let v = self.eval_scalar(expr, ctx)?;
            if let ExcelValue::Error(e) = v {
                return Ok(ExcelValue::Error(e));
            }
            Some(v)
        } else {
            None
        };
        let array = self.eval_expr(array_expr, ctx)?;
        let grid = match xlsx_engine_core::eval::scan::matrix(array) {
            Ok(g) => g,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let (acc_p, val_p, body) = match resolve_seed_lambda(lambda_expr, ctx, 0) {
            Ok(l) => l,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        if grid.is_empty() || grid[0].is_empty() {
            return Ok(ExcelValue::Error(ExcelError::Calc));
        }
        let rows = grid.len();
        let cols = grid[0].len();
        let base = ctx.locals.len();
        ctx.locals
            .push(xlsx_engine_core::eval::makearray::Local::provided(
                acc_p,
                ExcelValue::Empty,
            ));
        ctx.locals
            .push(xlsx_engine_core::eval::makearray::Local::provided(
                val_p,
                ExcelValue::Empty,
            ));
        let mut out = Vec::with_capacity(rows);
        let mut acc = initial;
        let omit_first = acc.is_none();
        let mut first = true;
        for r in 0..rows {
            let mut row = Vec::with_capacity(cols);
            for c in 0..cols {
                let val = grid[r][c].clone();
                let next = if omit_first && first {
                    val
                } else {
                    ctx.locals[base].value = acc.clone().unwrap_or(ExcelValue::Empty);
                    ctx.locals[base + 1].value = val;
                    match self.eval_expr(&body, ctx)? {
                        ExcelValue::Array(_) => ExcelValue::Error(ExcelError::Calc),
                        other => other,
                    }
                };
                first = false;
                acc = Some(next.clone());
                row.push(next);
            }
            out.push(row);
        }
        ctx.locals.truncate(base);
        Ok(ExcelValue::Array(out))
    }

    fn fn_byrow(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let array = self.eval_expr(&args[0], ctx)?;
        if let ExcelValue::Error(e) = array {
            return Ok(ExcelValue::Error(e));
        }
        let grid = match xlsx_engine_core::eval::byrow::to_grid(array) {
            Ok(g) => g,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        match resolve_seed_lambda_arity(&args[1], ctx, 1, 0) {
            Ok((params, body)) => {
                let base = ctx.locals.len();
                ctx.locals
                    .push(xlsx_engine_core::eval::makearray::Local::provided(
                        params[0].clone(),
                        ExcelValue::Array(vec![grid[0].clone()]),
                    ));
                let mut out = Vec::with_capacity(grid.len());
                for row in grid {
                    ctx.locals[base].value = ExcelValue::Array(vec![row]);
                    let v = self.eval_expr(&body, ctx)?;
                    out.push(vec![match v {
                        ExcelValue::Array(_) => ExcelValue::Error(ExcelError::Calc),
                        other => other,
                    }]);
                }
                ctx.locals.truncate(base);
                Ok(ExcelValue::Array(out))
            }
            Err(xlsx_engine_core::LambdaError::WrongArity) => {
                Ok(ExcelValue::Error(ExcelError::Value))
            }
            Err(xlsx_engine_core::LambdaError::NotLambda) => {
                if let Expr::Name(n) = &args[1] {
                    if let Some(kind) = xlsx_engine_core::eta_agg(n) {
                        return Ok(xlsx_engine_core::excel_byrow(
                            &grid,
                            &xlsx_engine_core::RowPlan::Agg(kind),
                        ));
                    }
                }
                let second = self.eval_expr(&args[1], ctx)?;
                if let ExcelValue::Error(e) = second {
                    return Ok(ExcelValue::Error(e));
                }
                Ok(ExcelValue::Error(ExcelError::Calc))
            }
        }
    }

    fn fn_reduce(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        let (initial_expr, array_expr, lambda_expr) = match args.len() {
            2 => (None, &args[0], &args[1]),
            3 => {
                let init = if matches!(args[0], Expr::Missing) {
                    None
                } else {
                    Some(&args[0])
                };
                (init, &args[1], &args[2])
            }
            _ => return Ok(ExcelValue::Error(ExcelError::Value)),
        };
        let initial = if let Some(expr) = initial_expr {
            let v = self.eval_expr(expr, ctx)?;
            if let ExcelValue::Error(e) = v {
                return Ok(ExcelValue::Error(e));
            }
            Some(v)
        } else {
            None
        };
        if let Expr::Range(range) = array_expr {
            let sheet = range
                .sheet
                .clone()
                .unwrap_or_else(|| ctx.current_sheet.clone());
            if ctx.spec.workbook.sheet(Some(&sheet)).is_err() {
                return Ok(ExcelValue::Error(ExcelError::Ref));
            }
        }
        let array = self.eval_expr(array_expr, ctx)?;
        if let ExcelValue::Error(e) = array {
            return Ok(ExcelValue::Error(e));
        }
        let (names, body) = match resolve_seed_lambda_n(lambda_expr, ctx, 2, 0) {
            Ok(l) => l,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let items = xlsx_engine_core::eval::reduce::flatten_row_major(&array);
        let had_initial = initial.is_some();
        let mut acc = match initial {
            Some(v) => v,
            None => match items.first() {
                Some(v) => v.clone(),
                None => return Ok(ExcelValue::Error(ExcelError::Calc)),
            },
        };
        let start = if had_initial { 0 } else { 1 };
        let base = ctx.locals.len();
        ctx.locals
            .push(xlsx_engine_core::eval::makearray::Local::provided(
                names[0].clone(),
                ExcelValue::Empty,
            ));
        ctx.locals
            .push(xlsx_engine_core::eval::makearray::Local::provided(
                names[1].clone(),
                ExcelValue::Empty,
            ));
        for v in &items[start..] {
            ctx.locals[base].value = acc;
            ctx.locals[base + 1].value = v.clone();
            acc = self.eval_expr(&body, ctx)?;
        }
        ctx.locals.truncate(base);
        Ok(acc)
    }

    fn fn_bycol(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 2 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        if let Ok((params, body)) = resolve_seed_lambda_n(&args[1], ctx, 1, 0) {
            let param = params.into_iter().next().unwrap();
            if let Some(op) = seed_bycol_lambda_op(&body, &param) {
                let array = self.eval_expr(&args[0], ctx)?;
                return Ok(apply_seed_bycol_fast(&array, &op));
            }
            let array = self.eval_expr(&args[0], ctx)?;
            if let ExcelValue::Error(e) = array {
                return Ok(ExcelValue::Error(e));
            }
            let owned;
            let rows: &[Vec<ExcelValue>] = match &array {
                ExcelValue::Array(rows) if !rows.is_empty() && !rows[0].is_empty() => rows,
                ExcelValue::Array(_) => return Ok(ExcelValue::Error(ExcelError::Value)),
                other => {
                    owned = vec![vec![other.clone()]];
                    &owned
                }
            };
            let ncols = rows[0].len();
            let base = ctx.locals.len();
            ctx.locals
                .push(xlsx_engine_core::eval::makearray::Local::provided(
                    param,
                    ExcelValue::Empty,
                ));
            let mut out = Vec::with_capacity(ncols);
            for c in 0..ncols {
                ctx.locals[base].value = xlsx_engine_core::eval::bycol::column_arg(rows, c);
                let v = self.eval_expr(&body, ctx)?;
                out.push(xlsx_engine_core::eval::bycol::scalar_result(v));
            }
            ctx.locals.truncate(base);
            return Ok(xlsx_engine_core::eval::bycol::row_result(out));
        }
        if let Some(op) = seed_bycol_eta(&args[1]) {
            let array = self.eval_expr(&args[0], ctx)?;
            return Ok(apply_seed_bycol_fast(&array, &op));
        }
        Ok(ExcelValue::Error(ExcelError::Value))
    }

    fn fn_let(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if !xlsx_engine_core::eval::excel_let::arity_ok(args.len()) {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let base = ctx.locals.len();
        let mut i = 0;
        while i + 1 < args.len() {
            let name = match &args[i] {
                Expr::Name(n) => match xlsx_engine_core::eval::excel_let::bind_name_str(n) {
                    Ok(n) => n,
                    Err(e) => {
                        ctx.locals.truncate(base);
                        return Ok(ExcelValue::Error(e));
                    }
                },
                _ => {
                    ctx.locals.truncate(base);
                    return Ok(ExcelValue::Error(ExcelError::Name));
                }
            };
            let value = match self.eval_expr(&args[i + 1], ctx) {
                Ok(v) => v,
                Err(e) => {
                    ctx.locals.truncate(base);
                    return Err(e);
                }
            };
            ctx.locals
                .push(xlsx_engine_core::eval::makearray::Local::provided(
                    name, value,
                ));
            i += 2;
        }
        let out = self.eval_expr(&args[i], ctx);
        ctx.locals.truncate(base);
        out
    }

    fn fn_rri(&self, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
        if args.len() != 3 {
            return Ok(ExcelValue::Error(ExcelError::Value));
        }
        let nper = match self.as_number(&self.eval_scalar(&args[0], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let pv = match self.as_number(&self.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        let fv = match self.as_number(&self.eval_scalar(&args[2], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        };
        match excel_rri(nper, pv, fv) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        }
    }
}

fn collect_irr_cashflows_into(
    v: &ExcelValue,
    from_range: bool,
    sem: Semantics,
    out: &mut Vec<f64>,
) -> Result<(), ExcelError> {
    match (v, from_range) {
        (ExcelValue::Array(rows), _) => {
            for row in rows {
                for c in row {
                    collect_irr_cashflows_into(c, true, sem, out)?;
                }
            }
            Ok(())
        }
        (ExcelValue::Error(e), _) => Err(*e),
        (ExcelValue::Number(n), _) => {
            if !n.is_finite() {
                return Err(ExcelError::Num);
            }
            out.push(*n);
            Ok(())
        }
        (ExcelValue::Empty | ExcelValue::Bool(_) | ExcelValue::Text(_), true) => Ok(()),
        (other, false) => {
            let n = match (sem, other) {
                (Semantics::ExcelSeed, ExcelValue::Empty) => 0.0,
                (Semantics::ExcelSeed, ExcelValue::Bool(true)) => 1.0,
                (Semantics::ExcelSeed, ExcelValue::Bool(false)) => 0.0,
                (Semantics::ExcelSeed, ExcelValue::Text(s)) => parse_excel_number(s)?,
                _ => return Err(ExcelError::Value),
            };
            if !n.is_finite() {
                return Err(ExcelError::Num);
            }
            out.push(n);
            Ok(())
        }
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

fn resolve_sumifs_range(expr: &Expr, ctx: &Ctx<'_>) -> Result<RangeRef, ExcelError> {
    match expr {
        Expr::Range(r) => Ok(r.clone()),
        Expr::Cell(c) => Ok(RangeRef::new(c.sheet.clone(), c.addr, c.addr)),
        Expr::Name(n) => {
            let def = ctx
                .spec
                .workbook
                .defined_name(n)
                .map_err(|_| ExcelError::Name)?;
            let refers = def.refers_to.trim();
            let body = refers.strip_prefix('=').unwrap_or(refers).trim();
            if let Ok(r) = RangeRef::parse(body) {
                return Ok(r);
            }
            if let Ok(c) = CellRef::parse(body) {
                return Ok(RangeRef::new(c.sheet, c.addr, c.addr));
            }
            Err(ExcelError::Value)
        }
        _ => Err(ExcelError::Value),
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

fn seed_sumif_range(expr: &Expr, ctx: &Ctx<'_>) -> Result<RangeRef, ExcelError> {
    seed_if_range(expr, ctx)
}

fn seed_if_range(expr: &Expr, ctx: &Ctx<'_>) -> Result<RangeRef, ExcelError> {
    match expr {
        Expr::Range(r) => Ok(r.clone()),
        Expr::Cell(c) => Ok(RangeRef::new(c.sheet.clone(), c.addr, c.addr)),
        Expr::Name(n) => {
            let def = ctx
                .spec
                .workbook
                .defined_name(n)
                .map_err(|_| ExcelError::Name)?;
            let refers = def.refers_to.trim();
            let body = refers.strip_prefix('=').unwrap_or(refers).trim();
            if let Ok(r) = RangeRef::parse(body) {
                return Ok(r);
            }
            if let Ok(c) = CellRef::parse(body) {
                return Ok(RangeRef::new(c.sheet, c.addr, c.addr));
            }
            Err(ExcelError::Value)
        }
        _ => Err(ExcelError::Value),
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

fn countif_range_sheet<'a>(expr: &'a Expr, ctx: &'a Ctx<'_>) -> Option<&'a str> {
    match expr {
        Expr::Range(r) => Some(r.sheet.as_deref().unwrap_or(ctx.current_sheet.as_str())),
        Expr::Cell(c) => Some(c.sheet.as_deref().unwrap_or(ctx.current_sheet.as_str())),
        _ => None,
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

fn npv_feed(
    v: &ExcelValue,
    from_range: bool,
    rate: f64,
    one: f64,
    factor: &mut f64,
    sum: &mut f64,
) -> Option<ExcelError> {
    match (v, from_range) {
        (ExcelValue::Array(rows), _) => {
            for row in rows {
                for c in row {
                    if let Some(e) = npv_feed(c, true, rate, one, factor, sum) {
                        return Some(e);
                    }
                }
            }
            None
        }
        (ExcelValue::Error(e), _) => Some(*e),
        (ExcelValue::Number(n), _) => npv_push(*n, rate, one, factor, sum),
        (ExcelValue::Empty, _) => None,
        (ExcelValue::Bool(b), false) => {
            npv_push(if *b { 1.0 } else { 0.0 }, rate, one, factor, sum)
        }
        (ExcelValue::Bool(_), true) => None,
        (ExcelValue::Text(s), false) => match parse_excel_number(s) {
            Ok(n) => npv_push(n, rate, one, factor, sum),
            Err(e) => Some(e),
        },
        (ExcelValue::Text(_), true) => None,
    }
}

fn npv_push(v: f64, rate: f64, one: f64, factor: &mut f64, sum: &mut f64) -> Option<ExcelError> {
    if rate == -1.0 {
        return Some(ExcelError::Div0);
    }
    *factor *= one;
    *sum += v / *factor;
    if !sum.is_finite() {
        Some(ExcelError::Num)
    } else {
        None
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

fn flatten_join_texts(
    v: &ExcelValue,
    out: &mut Vec<String>,
    interp: &Interpreter,
) -> Result<(), ExcelError> {
    match v {
        ExcelValue::Array(rows) => {
            for row in rows {
                for c in row {
                    flatten_join_texts(c, out, interp)?;
                }
            }
            Ok(())
        }
        other => {
            out.push(interp.as_text(other)?);
            Ok(())
        }
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

/// Excel `SUBSTITUTE` kernel (same semantics as `xlsx-engine-core`).
fn excel_substitute(
    text: &str,
    old_text: &str,
    new_text: &str,
    instance_num: Option<u32>,
) -> String {
    if old_text.is_empty() {
        return text.to_owned();
    }
    match instance_num {
        None => {
            if old_text == new_text {
                return text.to_owned();
            }
            let mut count = 0usize;
            let mut from = 0usize;
            while let Some(rel) = text[from..].find(old_text) {
                count += 1;
                from += rel + old_text.len();
            }
            if count == 0 {
                return text.to_owned();
            }
            let cap = text.len() + count * new_text.len() - count * old_text.len();
            let mut out = String::with_capacity(cap);
            from = 0;
            let mut last = 0usize;
            while let Some(rel) = text[from..].find(old_text) {
                let pos = from + rel;
                out.push_str(&text[last..pos]);
                out.push_str(new_text);
                last = pos + old_text.len();
                from = last;
            }
            out.push_str(&text[last..]);
            out
        }
        Some(n) => {
            let mut from = 0usize;
            let mut seen = 0u32;
            while let Some(rel) = text[from..].find(old_text) {
                let pos = from + rel;
                seen += 1;
                if seen == n {
                    let cap = text.len() + new_text.len() - old_text.len();
                    let mut out = String::with_capacity(cap);
                    out.push_str(&text[..pos]);
                    out.push_str(new_text);
                    out.push_str(&text[pos + old_text.len()..]);
                    return out;
                }
                from = pos + old_text.len();
            }
            text.to_owned()
        }
    }
}
fn sumproduct_number(v: &ExcelValue) -> Result<f64, ExcelError> {
    match v {
        ExcelValue::Number(n) => Ok(*n),
        ExcelValue::Empty | ExcelValue::Text(_) | ExcelValue::Bool(_) => Ok(0.0),
        ExcelValue::Error(e) => Err(*e),
        ExcelValue::Array(_) => Ok(0.0),
    }
}

fn sumproduct_shape(v: &ExcelValue) -> (usize, usize) {
    match v {
        ExcelValue::Array(rows) if rows.is_empty() => (0, 0),
        ExcelValue::Array(rows) => (rows.len(), rows[0].len()),
        _ => (1, 1),
    }
}

fn sumproduct_get(v: &ExcelValue, r: usize, c: usize, as_scalar: bool) -> &ExcelValue {
    match v {
        ExcelValue::Array(rows) if !as_scalar => rows
            .get(r)
            .and_then(|row| row.get(c))
            .unwrap_or(&ExcelValue::Empty),
        ExcelValue::Array(rows) => rows
            .first()
            .and_then(|row| row.first())
            .unwrap_or(&ExcelValue::Empty),
        other => other,
    }
}

fn sumproduct_product_sum(arrays: &[ExcelValue]) -> ExcelValue {
    if arrays.is_empty() {
        return ExcelValue::Error(ExcelError::Value);
    }
    let mut packed: Vec<Vec<f64>> = Vec::with_capacity(arrays.len());
    let mut dims: Option<(usize, usize)> = None;
    for a in arrays {
        let (rows, cols, nums) = match a {
            ExcelValue::Array(grid) => {
                if grid.is_empty() {
                    (0, 0, Vec::new())
                } else {
                    let cols = grid[0].len();
                    let mut out = Vec::with_capacity(grid.len() * cols);
                    for row in grid {
                        if row.len() != cols {
                            return ExcelValue::Error(ExcelError::Value);
                        }
                        for c in row {
                            match sumproduct_number(c) {
                                Ok(n) => out.push(n),
                                Err(e) => return ExcelValue::Error(e),
                            }
                        }
                    }
                    (grid.len(), cols, out)
                }
            }
            other => match sumproduct_number(other) {
                Ok(n) => (1, 1, vec![n]),
                Err(e) => return ExcelValue::Error(e),
            },
        };
        if let Some(d) = dims {
            if d != (rows, cols) {
                return ExcelValue::Error(ExcelError::Value);
            }
        } else {
            dims = Some((rows, cols));
        }
        packed.push(nums);
    }
    let acc = match packed.as_slice() {
        [] => 0.0,
        [a] => a.iter().copied().sum(),
        [a, b] => a
            .iter()
            .zip(b.iter())
            .fold(0.0, |acc, (x, y)| x.mul_add(*y, acc)),
        rest => {
            let n = rest[0].len();
            let mut acc = 0.0;
            for i in 0..n {
                let mut p = 1.0;
                for a in rest {
                    p *= a[i];
                }
                acc += p;
            }
            acc
        }
    };
    ExcelValue::Number(acc)
}
fn trunc_start_num(n: f64) -> Result<u64, ExcelError> {
    if !n.is_finite() {
        return Err(ExcelError::Value);
    }
    let t = n.trunc();
    if t < 1.0 {
        return Err(ExcelError::Value);
    }
    if t > u64::MAX as f64 {
        Ok(u64::MAX)
    } else {
        Ok(t as u64)
    }
}

fn trunc_num_chars(n: f64) -> Result<u64, ExcelError> {
    if !n.is_finite() {
        return Err(ExcelError::Value);
    }
    let t = n.trunc();
    if t < 0.0 {
        return Err(ExcelError::Value);
    }
    if t > u64::MAX as f64 {
        Ok(u64::MAX)
    } else {
        Ok(t as u64)
    }
}
fn unique_to_grid(v: ExcelValue) -> Result<Vec<Vec<ExcelValue>>, ExcelError> {
    match v {
        ExcelValue::Array(rows) => {
            if rows.is_empty() {
                return Ok(rows);
            }
            let cols = rows[0].len();
            if rows.iter().any(|r| r.len() != cols) {
                return Err(ExcelError::Value);
            }
            Ok(rows)
        }
        other => Ok(vec![vec![other]]),
    }
}

fn unique_number_key(n: f64) -> u64 {
    if n == 0.0 {
        0.0f64.to_bits()
    } else {
        excel_round_15(n).to_bits()
    }
}

fn unique_cell_key(v: &ExcelValue) -> String {
    match v {
        ExcelValue::Empty => "e".into(),
        ExcelValue::Number(n) => format!("n{}", unique_number_key(*n)),
        ExcelValue::Text(s) => format!("t{}", s.to_ascii_lowercase()),
        ExcelValue::Bool(b) => format!("b{b}"),
        ExcelValue::Error(e) => format!("r{}", e.short_id()),
        ExcelValue::Array(_) => "a".into(),
    }
}

fn unique_item_key(cells: &[ExcelValue]) -> String {
    cells
        .iter()
        .map(unique_cell_key)
        .collect::<Vec<_>>()
        .join("\u{1f}")
}

fn unique_apply_seed(grid: &[Vec<ExcelValue>], by_col: bool, exactly_once: bool) -> ExcelValue {
    if grid.is_empty() || grid[0].is_empty() {
        return ExcelValue::Error(ExcelError::Calc);
    }
    let cols = grid[0].len();
    if grid.iter().any(|r| r.len() != cols) {
        return ExcelValue::Error(ExcelError::Value);
    }
    let items: Vec<Vec<ExcelValue>> = if by_col {
        (0..cols)
            .map(|c| grid.iter().map(|row| row[c].clone()).collect())
            .collect()
    } else {
        grid.to_vec()
    };
    let mut first: HashMap<String, usize> = HashMap::new();
    let mut counts: Vec<usize> = Vec::new();
    let mut order: Vec<Vec<ExcelValue>> = Vec::new();
    for item in items {
        let key = unique_item_key(&item);
        if let Some(&idx) = first.get(&key) {
            counts[idx] += 1;
        } else {
            first.insert(key, order.len());
            counts.push(1);
            order.push(item);
        }
    }
    if exactly_once {
        order = order
            .into_iter()
            .enumerate()
            .filter_map(|(i, row)| if counts[i] == 1 { Some(row) } else { None })
            .collect();
    }
    if order.is_empty() {
        return ExcelValue::Error(ExcelError::Calc);
    }
    let out = if by_col {
        if order.is_empty() {
            Vec::new()
        } else {
            let height = order[0].len();
            (0..height)
                .map(|r| order.iter().map(|col| col[r].clone()).collect())
                .collect()
        }
    } else {
        order
    };
    ExcelValue::Array(out)
}
fn collect_irr_cashflows(
    v: &ExcelValue,
    from_range: bool,
    sem: Semantics,
) -> Result<Vec<f64>, ExcelError> {
    let mut out = Vec::new();
    collect_irr_cashflows_into(v, from_range, sem, &mut out)?;
    Ok(out)
}
fn collect_textafter_delims(v: &ExcelValue, out: &mut Vec<String>) -> Result<(), ExcelError> {
    match v {
        ExcelValue::Error(e) => Err(*e),
        ExcelValue::Array(rows) => {
            for row in rows {
                for cell in row {
                    collect_textafter_delims(cell, out)?;
                }
            }
            Ok(())
        }
        ExcelValue::Text(s) => {
            out.push(s.clone());
            Ok(())
        }
        ExcelValue::Empty => {
            out.push(String::new());
            Ok(())
        }
        ExcelValue::Bool(true) => {
            out.push("TRUE".into());
            Ok(())
        }
        ExcelValue::Bool(false) => {
            out.push("FALSE".into());
            Ok(())
        }
        ExcelValue::Number(n) => {
            out.push(format_plain(*n));
            Ok(())
        }
    }
}

fn flatten_textbefore_delims(
    v: &ExcelValue,
    out: &mut Vec<String>,
    interp: &Interpreter,
) -> Result<(), ExcelError> {
    match v {
        ExcelValue::Error(e) => Err(*e),
        ExcelValue::Array(rows) => {
            for row in rows {
                for cell in row {
                    flatten_textbefore_delims(cell, out, interp)?;
                }
            }
            Ok(())
        }
        other => {
            out.push(interp.as_text(other)?);
            Ok(())
        }
    }
}

fn lookup_local(
    locals: &[xlsx_engine_core::eval::makearray::Local],
    name: &str,
) -> Option<ExcelValue> {
    xlsx_engine_core::eval::makearray::lookup_binding(locals, name)
}

fn seed_is_omitted(arg: &Expr, locals: &[xlsx_engine_core::eval::makearray::Local]) -> bool {
    match arg {
        Expr::Missing => true,
        Expr::Name(n) => {
            xlsx_engine_core::eval::makearray::lookup_omitted(locals, n).unwrap_or(false)
        }
        _ => false,
    }
}

fn seed_lambda_name(name: &str) -> bool {
    xlsx_engine_core::eval::makearray::is_lambda_name(name)
}

fn resolve_seed_lambda_any(
    expr: &Expr,
    ctx: &Ctx<'_>,
    depth: usize,
) -> Result<(Vec<String>, Expr), ExcelError> {
    if depth > 16 {
        return Err(ExcelError::Value);
    }
    match expr {
        Expr::Call { name, args } if seed_lambda_name(name) => {
            if args.is_empty() {
                return Err(ExcelError::Value);
            }
            let body = args[args.len() - 1].clone();
            let mut params = Vec::with_capacity(args.len() - 1);
            for p in &args[..args.len() - 1] {
                match p {
                    Expr::Name(n) => {
                        params.push(xlsx_engine_core::eval::makearray::strip_xlpm(n).to_string())
                    }
                    _ => return Err(ExcelError::Value),
                }
            }
            Ok((params, body))
        }
        Expr::Name(n) => {
            let def = ctx
                .spec
                .workbook
                .defined_name(n)
                .map_err(|_| ExcelError::Value)?;
            let ast = parse(&def.refers_to).map_err(|_| ExcelError::Value)?;
            resolve_seed_lambda_any(&ast, ctx, depth + 1)
        }
        _ => Err(ExcelError::Value),
    }
}

fn resolve_seed_lambda_n(
    expr: &Expr,
    ctx: &Ctx<'_>,
    n: usize,
    depth: usize,
) -> Result<(Vec<String>, Expr), ExcelError> {
    let (params, body) = resolve_seed_lambda_any(expr, ctx, depth)?;
    if params.len() != n {
        return Err(ExcelError::Value);
    }
    Ok((params, body))
}

fn resolve_seed_lambda_arity(
    expr: &Expr,
    ctx: &Ctx<'_>,
    arity: usize,
    depth: usize,
) -> Result<(Vec<String>, Expr), xlsx_engine_core::LambdaError> {
    if depth > 16 {
        return Err(xlsx_engine_core::LambdaError::NotLambda);
    }
    match expr {
        Expr::Call { name, args } if seed_lambda_name(name) => {
            if args.len() != arity + 1 {
                return Err(xlsx_engine_core::LambdaError::WrongArity);
            }
            let mut params = Vec::with_capacity(arity);
            for p in &args[..arity] {
                match p {
                    Expr::Name(n) => {
                        params.push(xlsx_engine_core::eval::makearray::strip_xlpm(n).to_string())
                    }
                    _ => return Err(xlsx_engine_core::LambdaError::WrongArity),
                }
            }
            Ok((params, args[arity].clone()))
        }
        Expr::Name(n) => {
            let def = ctx
                .spec
                .workbook
                .defined_name(n)
                .map_err(|_| xlsx_engine_core::LambdaError::NotLambda)?;
            let ast =
                parse(&def.refers_to).map_err(|_| xlsx_engine_core::LambdaError::NotLambda)?;
            resolve_seed_lambda_arity(&ast, ctx, arity, depth + 1)
        }
        _ => Err(xlsx_engine_core::LambdaError::NotLambda),
    }
}

fn seed_bycol_eta(expr: &Expr) -> Option<xlsx_engine_core::BycolOp> {
    match expr {
        Expr::Name(n) => xlsx_engine_core::eval::bycol::eta_op(n),
        _ => None,
    }
}

/// `LAMBDA(c, SUM(c))` — same range-like fold as calc-core `classify`.
fn seed_bycol_lambda_op(body: &Expr, param: &str) -> Option<xlsx_engine_core::BycolOp> {
    match body {
        Expr::Call { name, args } if args.len() == 1 => {
            if let Expr::Name(n) = &args[0] {
                if xlsx_engine_core::eval::makearray::names_eq(n, param) {
                    return xlsx_engine_core::eval::bycol::eta_op(name);
                }
            }
            None
        }
        _ => None,
    }
}

fn apply_seed_bycol_fast(array: &ExcelValue, op: &xlsx_engine_core::BycolOp) -> ExcelValue {
    match array {
        ExcelValue::Error(e) => ExcelValue::Error(*e),
        ExcelValue::Array(rows) => xlsx_engine_core::excel_bycol(rows, op),
        other => xlsx_engine_core::excel_bycol(&[vec![other.clone()]], op),
    }
}

fn resolve_seed_lambda(
    expr: &Expr,
    ctx: &Ctx<'_>,
    depth: usize,
) -> Result<(String, String, Expr), ExcelError> {
    if depth > 16 {
        return Err(ExcelError::Value);
    }
    match expr {
        Expr::Call { name, args } if seed_lambda_name(name) => {
            if args.len() != 3 {
                return Err(ExcelError::Value);
            }
            let row = match &args[0] {
                Expr::Name(n) => xlsx_engine_core::eval::makearray::strip_xlpm(n).to_string(),
                _ => return Err(ExcelError::Value),
            };
            let col = match &args[1] {
                Expr::Name(n) => xlsx_engine_core::eval::makearray::strip_xlpm(n).to_string(),
                _ => return Err(ExcelError::Value),
            };
            Ok((row, col, args[2].clone()))
        }
        Expr::Name(n) => {
            let def = ctx
                .spec
                .workbook
                .defined_name(n)
                .map_err(|_| ExcelError::Value)?;
            let ast = parse(&def.refers_to).map_err(|_| ExcelError::Value)?;
            resolve_seed_lambda(&ast, ctx, depth + 1)
        }
        _ => Err(ExcelError::Value),
    }
}
