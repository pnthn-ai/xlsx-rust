//! Worksheet functions implemented with Excel-compatible semantics.
//!
//! Unknown names return `#NAME?` (an Excel value, not [`EvalError`]).
//! Dedicated kernels live in sibling modules (`ifs`, `filter`, `sort`,
//! `xlookup`, `textsplit`, `xnpv`, `map`, `isomitted`, …). Financial TVM
//! kernels live in [`xlsx_types`] (`excel_pmt` / `excel_fv` / `excel_pv` / …).

use super::{coerce, compare, excel_pow, Ctx, Evaluator};
use crate::ast::Expr;
use crate::dates::{
    date_serial, eomonth_serial, isoweeknum, networkdays_count, networkdays_count_mask,
    parse_weekend_mask, serial_to_ymd, time_fraction, weekday, weekend_mask_from_code,
    weekend_mask_from_string, workday_serial, workday_serial_intl, yearfrac, WEEKEND_SAT_SUN,
};
use crate::text_format;
use xlsx_types::{
    count_matches, excel_ceiling, excel_ceiling_math, excel_cumipmt, excel_cumprinc, excel_effect,
    excel_floor, excel_floor_math, excel_fv, excel_ipmt, excel_nominal, excel_nper,
    excel_pduration, excel_pmt, excel_ppmt, excel_pv, excel_rate, excel_rri, Criterion, EvalError,
    ExcelError, ExcelValue,
};

pub(crate) fn dispatch(
    ev: &Evaluator,
    name: &str,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    match name.to_ascii_uppercase().as_str() {
        "SUM" => fn_agg(ev, args, ctx, AggKind::Sum),
        "SUMPRODUCT" => super::sumproduct::eval(ev, args, ctx),
        "PRODUCT" => fn_agg(ev, args, ctx, AggKind::Product),
        "AVERAGE" => fn_agg(ev, args, ctx, AggKind::Average),
        "MIN" => fn_agg(ev, args, ctx, AggKind::Min),
        "MAX" => fn_agg(ev, args, ctx, AggKind::Max),
        "COUNT" => fn_agg(ev, args, ctx, AggKind::Count),
        "COUNTA" => fn_agg(ev, args, ctx, AggKind::CountA),
        "COUNTBLANK" => fn_agg(ev, args, ctx, AggKind::CountBlank),
        "SUMIF" => super::sumif::fn_sumif(ev, args, ctx),
        "COUNTIF" => fn_countif(ev, args, ctx),
        "COUNTIFS" => super::countifs::fn_countifs(ev, args, ctx),
        "SUMIFS" => super::sumifs::fn_sumifs(ev, args, ctx),
        "AVERAGEIF" => super::averageif::fn_averageif(ev, args, ctx),
        "AVERAGEIFS" => super::averageifs::fn_averageifs(ev, args, ctx),
        "IF" => fn_if(ev, args, ctx),
        "IFS" => fn_ifs(ev, args, ctx),
        "IFERROR" => fn_iferror(ev, args, ctx),
        "IFNA" => fn_ifna(ev, args, ctx),
        "SWITCH" => fn_switch(ev, args, ctx),
        "AND" => fn_and_or(ev, args, ctx, AndOr::And),
        "OR" => fn_and_or(ev, args, ctx, AndOr::Or),
        "XOR" => fn_and_or(ev, args, ctx, AndOr::Xor),
        "NOT" => fn_not(ev, args, ctx),
        "VLOOKUP" => fn_vlookup(ev, args, ctx),
        "HLOOKUP" => fn_hlookup(ev, args, ctx),
        "XLOOKUP" => super::xlookup::eval(ev, args, ctx),
        "FILTER" => fn_filter(ev, args, ctx),
        "SORT" => super::sort::eval(ev, args, ctx),
        "SORTBY" => super::sortby::eval(ev, args, ctx),
        "SEQUENCE" => super::sequence::eval(ev, args, ctx),
        "VSTACK" => super::vstack::eval(ev, args, ctx),
        "HSTACK" => super::hstack::eval(ev, args, ctx),
        "TAKE" => fn_take(ev, args, ctx),
        "DROP" => super::drop::eval(ev, args, ctx),
        "CHOOSEROWS" => fn_chooserows(ev, args, ctx),
        "MAKEARRAY" | "_XLFN.MAKEARRAY" => fn_makearray(ev, args, ctx),
        "MAP" | "_XLFN.MAP" => super::map::eval(ev, args, ctx),
        "SCAN" | "_XLFN.SCAN" => super::scan::eval(ev, args, ctx),
        "BYROW" | "_XLFN.BYROW" => super::byrow::eval(ev, args, ctx),
        "REDUCE" | "_XLFN.REDUCE" => super::reduce::eval(ev, args, ctx),
        "BYCOL" | "_XLFN.BYCOL" => super::bycol::eval(ev, args, ctx),
        "LAMBDA" | "_XLFN.LAMBDA" => Ok(ExcelValue::Error(ExcelError::Calc)),
        "LET" | "_XLFN.LET" => fn_let(ev, args, ctx),
        "INDEX" => fn_index(ev, args, ctx),
        "MATCH" => fn_match(ev, args, ctx),
        "CHOOSE" => fn_choose(ev, args, ctx),
        "CHOOSECOLS" => super::choosecols::eval(ev, args, ctx),
        "ABS" => fn_unary_num(ev, args, ctx, |n| ExcelValue::Number(n.abs())),
        "SIGN" => fn_unary_num(ev, args, ctx, |n| {
            ExcelValue::Number(if n > 0.0 {
                1.0
            } else if n < 0.0 {
                -1.0
            } else {
                0.0
            })
        }),
        "INT" => fn_unary_num(ev, args, ctx, |n| ExcelValue::Number(n.floor())),
        "TRUNC" => fn_trunc(ev, args, ctx),
        "ROUND" => fn_round(ev, args, ctx),
        "ROUNDUP" => fn_round_dir(ev, args, ctx, RoundDir::Up),
        "ROUNDDOWN" => fn_round_dir(ev, args, ctx, RoundDir::Down),
        "FLOOR" => fn_floor_ceil(ev, args, ctx, FloorCeil::Floor),
        "CEILING" => fn_floor_ceil(ev, args, ctx, FloorCeil::Ceiling),
        "FLOOR.MATH" => fn_floor_ceil_math(ev, args, ctx, FloorCeil::Floor),
        "CEILING.MATH" => fn_floor_ceil_math(ev, args, ctx, FloorCeil::Ceiling),
        "MOD" => fn_mod(ev, args, ctx),
        "SQRT" => fn_unary_num(ev, args, ctx, |n| {
            if n < 0.0 {
                ExcelValue::Error(ExcelError::Num)
            } else {
                ExcelValue::Number(n.sqrt())
            }
        }),
        "POWER" => fn_power(ev, args, ctx),
        "PI" => Ok(ExcelValue::Number(std::f64::consts::PI)),
        "N" => fn_n(ev, args, ctx),
        "NA" => Ok(ExcelValue::Error(ExcelError::Na)),
        "TYPE" => fn_type(ev, args, ctx),
        "ERROR.TYPE" => fn_error_type(ev, args, ctx),
        "ISBLANK" => fn_is(ev, args, ctx, |v| matches!(v, ExcelValue::Empty)),
        "ISNUMBER" => fn_is(ev, args, ctx, |v| matches!(v, ExcelValue::Number(_))),
        "ISTEXT" => fn_is(ev, args, ctx, |v| matches!(v, ExcelValue::Text(_))),
        "ISLOGICAL" => fn_is(ev, args, ctx, |v| matches!(v, ExcelValue::Bool(_))),
        "ISNONTEXT" => fn_is(ev, args, ctx, |v| !matches!(v, ExcelValue::Text(_))),
        "ISERROR" => fn_is(ev, args, ctx, |v| matches!(v, ExcelValue::Error(_))),
        "ISERR" => fn_is(
            ev,
            args,
            ctx,
            |v| matches!(v, ExcelValue::Error(e) if *e != ExcelError::Na),
        ),
        "ISNA" => fn_is(ev, args, ctx, |v| {
            matches!(v, ExcelValue::Error(ExcelError::Na))
        }),
        "ISOMITTED" | "_XLFN.ISOMITTED" => fn_isomitted(args, ctx),
        "ISEVEN" => fn_even_odd(ev, args, ctx, true),
        "ISODD" => fn_even_odd(ev, args, ctx, false),
        "DATE" => fn_date(ev, args, ctx),
        "TIME" => fn_time(ev, args, ctx),
        "EOMONTH" => fn_eomonth(ev, args, ctx),
        "NETWORKDAYS" => fn_networkdays(ev, args, ctx),
        "NETWORKDAYS.INTL" => fn_networkdays_intl(ev, args, ctx),
        "WORKDAY" => fn_workday(ev, args, ctx),
        "WORKDAY.INTL" => fn_workday_intl(ev, args, ctx),
        "YEAR" => fn_ymd(ev, args, ctx, YmdPart::Year),
        "MONTH" => fn_ymd(ev, args, ctx, YmdPart::Month),
        "DAY" => fn_ymd(ev, args, ctx, YmdPart::Day),
        "WEEKDAY" => fn_weekday(ev, args, ctx),
        "ISOWEEKNUM" => fn_isoweeknum(ev, args, ctx),
        "YEARFRAC" => fn_yearfrac(ev, args, ctx),
        "LEFT" => fn_left_right(ev, args, ctx, true),
        "RIGHT" => fn_left_right(ev, args, ctx, false),
        "MID" => fn_mid(ev, args, ctx),
        "LEN" => fn_len(ev, args, ctx),
        "LOWER" => fn_lower(ev, args, ctx),
        "UPPER" => fn_upper(ev, args, ctx),
        "PROPER" => fn_proper(ev, args, ctx),
        "TRIM" => fn_trim(ev, args, ctx),
        "CLEAN" => fn_clean(ev, args, ctx),
        "EXACT" => fn_exact(ev, args, ctx),
        "FIND" => fn_find(ev, args, ctx),
        "SEARCH" => fn_search(ev, args, ctx),
        "VALUE" => fn_value(ev, args, ctx),
        "SUBSTITUTE" => fn_substitute(ev, args, ctx),
        "TEXT" => fn_text(ev, args, ctx),
        "REPLACE" => fn_replace(ev, args, ctx),
        "TEXTJOIN" => super::textjoin::fn_textjoin(ev, args, ctx),
        "TEXTSPLIT" => super::textsplit::fn_textsplit(ev, args, ctx),
        "TEXTAFTER" => fn_textafter(ev, args, ctx),
        "TEXTBEFORE" => fn_textbefore(ev, args, ctx),
        "CONCAT" => super::concat::fn_concat(ev, args, ctx),
        "REPT" => super::rept::fn_rept(ev, args, ctx),
        "NPV" => super::npv::eval(ev, args, ctx),
        "UNIQUE" => super::unique::eval(ev, args, ctx),
        "TOCOL" => super::tocol::eval(ev, args, ctx),
        "TOROW" => super::torow::eval(ev, args, ctx),
        "WRAPCOLS" => super::wrapcols::eval(ev, args, ctx),
        "WRAPROWS" => super::wraprows::eval(ev, args, ctx),
        "EXPAND" => super::expand::eval(ev, args, ctx),
        "RANDARRAY" => super::randarray::eval(ev, args, ctx),
        "IRR" => fn_irr(ev, args, ctx),
        "XNPV" => super::xnpv::eval(ev, args, ctx),
        "XIRR" => super::xirr::eval(ev, args, ctx),
        "MIRR" => fn_mirr(ev, args, ctx),
        "TRUE" => Ok(ExcelValue::Bool(true)),
        "FALSE" => Ok(ExcelValue::Bool(false)),
        "PMT" => fn_pmt(ev, args, ctx),
        "FV" => fn_fv(ev, args, ctx),
        "PV" => fn_pv(ev, args, ctx),
        "NPER" => fn_nper(ev, args, ctx),
        "RATE" => fn_rate(ev, args, ctx),
        "IPMT" => fn_ipmt(ev, args, ctx),
        "PPMT" => fn_ppmt(ev, args, ctx),
        "CUMPRINC" => fn_cumprinc(ev, args, ctx),
        "CUMIPMT" => fn_cumipmt(ev, args, ctx),
        "EFFECT" => fn_effect(ev, args, ctx),
        "NOMINAL" => fn_nominal(ev, args, ctx),
        "PDURATION" => fn_pduration(ev, args, ctx),
        "RRI" => fn_rri(ev, args, ctx),
        _ => apply_named_lambda(ev, name, args, ctx),
    }
}

fn fn_countif(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let crit_v = ev.eval_scalar(&args[1], ctx)?;
    let crit = Criterion::parse(&crit_v);
    match &args[0] {
        Expr::Range(r) => ev.countif_range(r, ctx, &crit),
        other => {
            let v = ev.eval_expr(other, ctx)?;
            Ok(ExcelValue::Number(count_matches(&v, &crit) as f64))
        }
    }
}

fn fn_agg(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
    kind: AggKind,
) -> Result<ExcelValue, EvalError> {
    let mut acc = AggAcc::new(kind);
    for arg in args {
        // LET / LAMBDA locals are values, not worksheet refs: SUM(TRUE) is 1.
        let from_range = arg.is_reference()
            && !matches!(arg, Expr::Name(n) if super::excel_let::is_bound(&ctx.locals, n));
        let v = ev.eval_expr(arg, ctx)?;
        if let Some(err) = acc.fold(&v, from_range) {
            return Ok(ExcelValue::Error(err));
        }
    }
    Ok(acc.finish())
}

fn fn_if(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.is_empty() {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let cond = ev.eval_scalar(&args[0], ctx)?;
    let truth = match coerce::to_logical(&cond) {
        Ok(b) => b,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    if args.len() == 1 {
        return Ok(ExcelValue::Bool(truth));
    }
    if truth {
        ev.eval_expr(&args[1], ctx)
    } else if args.len() >= 3 {
        ev.eval_expr(&args[2], ctx)
    } else {
        Ok(ExcelValue::Bool(false))
    }
}

/// Excel `IFS`: evaluate every pair (no short-circuit), first TRUE wins,
/// no match → `#N/A`. Odd / empty arity is `#VALUE!`.
fn fn_ifs(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.is_empty() || args.len() % 2 == 1 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let mut first_err = None;
    let mut first_true = None;
    let mut i = 0;
    while i + 1 < args.len() {
        let cond = ev.eval_scalar(&args[i], ctx)?;
        let val = ev.eval_expr(&args[i + 1], ctx)?;
        super::ifs::fold_pair(&cond, val, &mut first_err, &mut first_true);
        i += 2;
    }
    Ok(super::ifs::finish(first_err, first_true))
}

fn fn_iferror(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let v = ev.eval_expr(&args[0], ctx)?;
    if v.is_error() {
        ev.eval_expr(&args[1], ctx)
    } else {
        Ok(v)
    }
}

fn fn_ifna(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let v = ev.eval_expr(&args[0], ctx)?;
    if matches!(v, ExcelValue::Error(ExcelError::Na)) {
        ev.eval_expr(&args[1], ctx)
    } else {
        Ok(v)
    }
}

/// `SWITCH(expression, value1, result1, …, [default])`.
///
/// Exact `=` match, first hit wins, unused branches are not evaluated.
/// See [`super::switch`].
fn fn_switch(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() < 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let expr = ev.eval_scalar(&args[0], ctx)?;
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
        let value = ev.eval_scalar(&args[i], ctx)?;
        if let ExcelValue::Error(e) = value {
            return Ok(ExcelValue::Error(e));
        }
        if super::switch::matches(&expr, &value) {
            return ev.eval_expr(&args[i + 1], ctx);
        }
        i += 2;
    }
    if has_default {
        ev.eval_expr(&args[args.len() - 1], ctx)
    } else {
        Ok(ExcelValue::Error(ExcelError::Na))
    }
}

fn fn_and_or(
    ev: &Evaluator,
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
        let v = ev.eval_expr(arg, ctx)?;
        if let Some(err) = fold_logicals(&v, &mut seen, &mut true_count) {
            return Ok(ExcelValue::Error(err));
        }
    }
    Ok(ExcelValue::Bool(match kind {
        AndOr::And => true_count == seen,
        AndOr::Or => true_count > 0,
        AndOr::Xor => true_count % 2 == 1,
    }))
}

fn fn_not(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 1 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let v = ev.eval_scalar(&args[0], ctx)?;
    match coerce::to_logical(&v) {
        Ok(b) => Ok(ExcelValue::Bool(!b)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn fn_vlookup(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() < 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let lookup = ev.eval_scalar(&args[0], ctx)?;
    if let ExcelValue::Error(e) = lookup {
        return Ok(ExcelValue::Error(e));
    }
    let table = ev.eval_expr(&args[1], ctx)?;
    let col = ev.eval_scalar(&args[2], ctx)?;
    let approx = if args.len() >= 4 {
        match coerce::to_logical(&ev.eval_scalar(&args[3], ctx)?) {
            Ok(b) => b,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        true
    };
    let col_n = match coerce::to_number(&col) {
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
            if lookup_key_match(&lookup, &row[0]) {
                return Ok(row[col_idx - 1].clone());
            }
        }
        return Ok(ExcelValue::Error(ExcelError::Na));
    }
    match approx_upper_bound(&rows, &lookup) {
        Some(i) => Ok(rows[i][col_idx - 1].clone()),
        None => Ok(ExcelValue::Error(ExcelError::Na)),
    }
}

fn fn_hlookup(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() < 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let lookup = ev.eval_scalar(&args[0], ctx)?;
    if let ExcelValue::Error(e) = lookup {
        return Ok(ExcelValue::Error(e));
    }
    let table = ev.eval_expr(&args[1], ctx)?;
    let row = ev.eval_scalar(&args[2], ctx)?;
    let approx = if args.len() >= 4 {
        match coerce::to_logical(&ev.eval_scalar(&args[3], ctx)?) {
            Ok(b) => b,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        true
    };
    let row_n = match coerce::to_number(&row) {
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
    let header: Vec<ExcelValue> = (0..width).map(|c| rows[0][c].clone()).collect();
    let keys: Vec<Vec<ExcelValue>> = header.into_iter().map(|k| vec![k]).collect();
    let col = if !approx {
        keys.iter().position(|k| lookup_key_match(&lookup, &k[0]))
    } else {
        approx_upper_bound(&keys, &lookup)
    };
    match col {
        Some(c) => Ok(rows[row_idx - 1][c].clone()),
        None => Ok(ExcelValue::Error(ExcelError::Na)),
    }
}

/// Excel `LET(name1, value1, [name2, value2, …], calculation)`.
///
/// Name arguments are identifiers (not evaluated). Values bind onto the
/// shared locals stack. See [`super::excel_let`].
fn fn_let(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    super::excel_let::apply(ev, args, ctx)
}

/// Excel `MAKEARRAY(rows, cols, LAMBDA(r, c, body))`.
///
/// Third argument is inspected as a LAMBDA (inline or defined name), not
/// evaluated as a worksheet value. See [`super::makearray`]. Shared LAMBDA
/// resolve is reused by MAP / SCAN / BYROW / REDUCE / BYCOL.
fn fn_makearray(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let rows_v = ev.eval_scalar(&args[0], ctx)?;
    if let ExcelValue::Error(e) = rows_v {
        return Ok(ExcelValue::Error(e));
    }
    let cols_v = ev.eval_scalar(&args[1], ctx)?;
    if let ExcelValue::Error(e) = cols_v {
        return Ok(ExcelValue::Error(e));
    }
    let (rows, cols) = match super::makearray::dims(&rows_v, &cols_v) {
        Ok(d) => d,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let (row_p, col_p, body) = match super::makearray::resolve_lambda(&args[2], ctx) {
        Ok(l) => l,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    super::makearray::apply(ev, ctx, rows, cols, &row_p, &col_p, &body)
}

/// Excel `FILTER(array, include, [if_empty])`.
///
/// Arity 2 or 3. Omitted `if_empty` + no matches → `#CALC!`.
/// Excel `TAKE(array, rows, [cols])`. Counts evaluate as scalars; the
/// array is not implicit-intersected. See [`super::take`].
fn fn_take(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.is_empty() || args.len() > 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let array = ev.eval_expr(&args[0], ctx)?;
    let rows = if args.len() >= 2 {
        Some(ev.eval_scalar(&args[1], ctx)?)
    } else {
        None
    };
    let cols = if args.len() >= 3 {
        Some(ev.eval_scalar(&args[2], ctx)?)
    } else {
        None
    };
    Ok(super::take::take(&array, rows.as_ref(), cols.as_ref()))
}

fn fn_filter(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 || args.len() > 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let array = ev.eval_expr(&args[0], ctx)?;
    let include = ev.eval_expr(&args[1], ctx)?;
    let if_empty = if args.len() >= 3 {
        Some(ev.eval_expr(&args[2], ctx)?)
    } else {
        None
    };
    Ok(super::filter::select(&array, &include, if_empty.as_ref()))
}

/// Excel `CHOOSEROWS(array, row_num1, [row_num2], ...)`.
///
/// Arity ≥ 2. Negative indices count from the end. `0` / out-of-range → `#VALUE!`.
fn fn_chooserows(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let array = ev.eval_expr(&args[0], ctx)?;
    let mut row_nums = Vec::with_capacity(args.len() - 1);
    for arg in &args[1..] {
        row_nums.push(ev.eval_expr(arg, ctx)?);
    }
    Ok(super::chooserows::select(&array, &row_nums))
}

fn fn_index(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.is_empty() {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let array = match ev.eval_expr(&args[0], ctx)? {
        ExcelValue::Array(rows) => rows,
        other => vec![vec![other]],
    };
    if array.is_empty() || array[0].is_empty() {
        return Ok(ExcelValue::Error(ExcelError::Ref));
    }
    let nrows = array.len();
    let ncols = array[0].len();
    let row_n = if args.len() >= 2 {
        match coerce::to_number(&ev.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        1.0
    };
    let col_n = if args.len() >= 3 {
        match coerce::to_number(&ev.eval_scalar(&args[2], ctx)?) {
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

fn fn_match(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let lookup = ev.eval_scalar(&args[0], ctx)?;
    if let ExcelValue::Error(e) = lookup {
        return Ok(ExcelValue::Error(e));
    }
    let vec = flatten_vector(ev.eval_expr(&args[1], ctx)?);
    let match_type = if args.len() >= 3 {
        match coerce::to_number(&ev.eval_scalar(&args[2], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        1.0
    };
    let keys: Vec<Vec<ExcelValue>> = vec.into_iter().map(|k| vec![k]).collect();
    if match_type == 0.0 {
        for (i, row) in keys.iter().enumerate() {
            if lookup_key_match(&lookup, &row[0]) {
                return Ok(ExcelValue::Number((i + 1) as f64));
            }
        }
        return Ok(ExcelValue::Error(ExcelError::Na));
    }
    if match_type > 0.0 {
        return Ok(match approx_upper_bound(&keys, &lookup) {
            Some(i) => ExcelValue::Number((i + 1) as f64),
            None => ExcelValue::Error(ExcelError::Na),
        });
    }
    for (i, row) in keys.iter().enumerate() {
        if excel_geq(&row[0], &lookup) {
            return Ok(ExcelValue::Number((i + 1) as f64));
        }
    }
    Ok(ExcelValue::Error(ExcelError::Na))
}

fn fn_choose(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let idx = match coerce::to_number(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let i = idx.trunc() as i64;
    if i < 1 || i as usize >= args.len() {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    ev.eval_expr(&args[i as usize], ctx)
}

fn fn_unary_num(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
    f: impl Fn(f64) -> ExcelValue,
) -> Result<ExcelValue, EvalError> {
    if args.len() != 1 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    match coerce::to_number(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(n) => Ok(f(n)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn fn_trunc(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.is_empty() {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let n = match coerce::to_number(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let digits = if args.len() >= 2 {
        match coerce::to_number(&ev.eval_scalar(&args[1], ctx)?) {
            Ok(d) => d.trunc() as i32,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        0
    };
    Ok(ExcelValue::Number(excel_trunc(n, digits)))
}

#[derive(Clone, Copy)]
enum FloorCeil {
    Floor,
    Ceiling,
}

fn fn_floor_ceil(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
    kind: FloorCeil,
) -> Result<ExcelValue, EvalError> {
    if args.len() != 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let n = match coerce::to_number(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let s = match coerce::to_number(&ev.eval_scalar(&args[1], ctx)?) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let r = match kind {
        FloorCeil::Floor => excel_floor(n, s),
        FloorCeil::Ceiling => excel_ceiling(n, s),
    };
    Ok(match r {
        Ok(v) => ExcelValue::Number(v),
        Err(e) => ExcelValue::Error(e),
    })
}

fn fn_floor_ceil_math(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
    kind: FloorCeil,
) -> Result<ExcelValue, EvalError> {
    if args.is_empty() || args.len() > 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let n = match coerce::to_number(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let s = if args.len() >= 2 {
        match coerce::to_number(&ev.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        1.0
    };
    let mode = if args.len() >= 3 {
        match coerce::to_number(&ev.eval_scalar(&args[2], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        0.0
    };
    let r = match kind {
        FloorCeil::Floor => excel_floor_math(n, s, mode),
        FloorCeil::Ceiling => excel_ceiling_math(n, s, mode),
    };
    Ok(match r {
        Ok(v) => ExcelValue::Number(v),
        Err(e) => ExcelValue::Error(e),
    })
}

fn fn_round(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let n = match coerce::to_number(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let digits = match coerce::to_number(&ev.eval_scalar(&args[1], ctx)?) {
        Ok(d) => d.trunc() as i32,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    Ok(ExcelValue::Number(excel_round_half_away(n, digits)))
}

#[derive(Clone, Copy)]
enum RoundDir {
    Up,
    Down,
}

fn fn_round_dir(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
    dir: RoundDir,
) -> Result<ExcelValue, EvalError> {
    if args.len() != 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let n = match coerce::to_number(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let digits = match coerce::to_number(&ev.eval_scalar(&args[1], ctx)?) {
        Ok(d) => d.trunc() as i32,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let out = match dir {
        RoundDir::Up => super::round::roundup(n, digits),
        RoundDir::Down => super::round::rounddown(n, digits),
    };
    Ok(ExcelValue::Number(out))
}

fn fn_mod(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let n = match coerce::to_number(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let d = match coerce::to_number(&ev.eval_scalar(&args[1], ctx)?) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    if d == 0.0 {
        return Ok(ExcelValue::Error(ExcelError::Div0));
    }
    Ok(ExcelValue::Number(n - d * (n / d).floor()))
}

fn fn_power(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let l = ev.eval_scalar(&args[0], ctx)?;
    let r = ev.eval_scalar(&args[1], ctx)?;
    Ok(excel_pow(&l, &r))
}

fn fn_n(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 1 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let v = ev.eval_scalar(&args[0], ctx)?;
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

fn fn_type(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 1 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    // TYPE of an array is 64 even in scalar context — do not implicit-intersect.
    let v = ev.eval_expr(&args[0], ctx)?;
    Ok(ExcelValue::Number(match v {
        ExcelValue::Number(_) => 1.0,
        ExcelValue::Text(_) => 2.0,
        ExcelValue::Bool(_) => 4.0,
        ExcelValue::Error(_) => 16.0,
        ExcelValue::Array(_) => 64.0,
        ExcelValue::Empty => 1.0,
    }))
}

fn fn_error_type(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() != 1 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let v = ev.eval_scalar(&args[0], ctx)?;
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

fn fn_isomitted(args: &[Expr], ctx: &Ctx<'_>) -> Result<ExcelValue, EvalError> {
    match super::isomitted::eval(args, &ctx.locals) {
        Ok(b) => Ok(ExcelValue::Bool(b)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

/// `MyFn(args)` when `MyFn` is a defined name that refers to a LAMBDA.
fn apply_named_lambda(
    ev: &Evaluator,
    name: &str,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if ctx.spec.workbook.defined_name(name).is_err() {
        return Ok(ExcelValue::Error(ExcelError::Name));
    }
    super::makearray::apply_callee(ev, &Expr::Name(name.to_string()), args, ctx)
}

fn fn_is(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
    pred: impl Fn(&ExcelValue) -> bool,
) -> Result<ExcelValue, EvalError> {
    if args.len() != 1 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let v = ev.eval_scalar(&args[0], ctx)?;
    Ok(ExcelValue::Bool(pred(&v)))
}

fn fn_even_odd(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
    even: bool,
) -> Result<ExcelValue, EvalError> {
    if args.len() != 1 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    match coerce::to_number(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(n) => {
            let t = n.trunc() as i64;
            Ok(ExcelValue::Bool(if even { t % 2 == 0 } else { t % 2 != 0 }))
        }
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn fn_date(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let y = match coerce::to_number(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(n) => n.trunc() as i32,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let m = match coerce::to_number(&ev.eval_scalar(&args[1], ctx)?) {
        Ok(n) => n.trunc() as i32,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let d = match coerce::to_number(&ev.eval_scalar(&args[2], ctx)?) {
        Ok(n) => n.trunc() as i32,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    match date_serial(y, m, d, ctx.spec.options.date_system) {
        Ok(n) => Ok(ExcelValue::Number(n)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn fn_time(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let h = match coerce::to_number(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let m = match coerce::to_number(&ev.eval_scalar(&args[1], ctx)?) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let s = match coerce::to_number(&ev.eval_scalar(&args[2], ctx)?) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    match time_fraction(h, m, s) {
        Ok(n) => Ok(ExcelValue::Number(n)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn fn_eomonth(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let start = match coerce::to_number(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let months = match coerce::to_number(&ev.eval_scalar(&args[1], ctx)?) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    match eomonth_serial(start, months, ctx.spec.options.date_system) {
        Ok(n) => Ok(ExcelValue::Number(n)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn fn_networkdays(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 || args.len() > 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let start_v = ev.eval_scalar(&args[0], ctx)?;
    let end_v = ev.eval_scalar(&args[1], ctx)?;
    let hol_v = if args.len() == 3 {
        Some(ev.eval_expr(&args[2], ctx)?)
    } else {
        None
    };
    let start = match coerce::to_number(&start_v) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let end = match coerce::to_number(&end_v) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let mut holidays = Vec::new();
    if let Some(v) = hol_v {
        if let Some(e) = collect_holiday_serials(&v, &mut holidays) {
            return Ok(ExcelValue::Error(e));
        }
    }
    match networkdays_count(start, end, &holidays, ctx.spec.options.date_system) {
        Ok(n) => Ok(ExcelValue::Number(n)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn fn_networkdays_intl(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 || args.len() > 4 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let start_v = ev.eval_scalar(&args[0], ctx)?;
    let end_v = ev.eval_scalar(&args[1], ctx)?;
    let weekend_v = if args.len() >= 3 {
        Some(ev.eval_scalar(&args[2], ctx)?)
    } else {
        None
    };
    let hol_v = if args.len() == 4 {
        Some(ev.eval_expr(&args[3], ctx)?)
    } else {
        None
    };
    let start = match coerce::to_number(&start_v) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let end = match coerce::to_number(&end_v) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let mask = match weekend_v {
        None => WEEKEND_SAT_SUN,
        Some(v) => match parse_weekend_arg(&v) {
            Ok(m) => m,
            Err(e) => return Ok(ExcelValue::Error(e)),
        },
    };
    let mut holidays = Vec::new();
    if let Some(v) = hol_v {
        if let Some(e) = collect_holiday_serials(&v, &mut holidays) {
            return Ok(ExcelValue::Error(e));
        }
    }
    match networkdays_count_mask(start, end, mask, &holidays, ctx.spec.options.date_system) {
        Ok(n) => Ok(ExcelValue::Number(n)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

/// Weekend number (1–7 / 11–17) or a 7-character `0`/`1` string (Mon→Sun).
///
/// Text is always a weekend string — `"1"` is `#VALUE!`, not code 1 — so
/// `"0000011"` cannot be misread as numeric 11.
fn parse_weekend_arg(v: &ExcelValue) -> Result<u8, ExcelError> {
    match v {
        ExcelValue::Error(e) => Err(*e),
        ExcelValue::Text(s) => weekend_mask_from_string(s),
        other => {
            let n = coerce::to_number(other)?;
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

fn fn_workday(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 || args.len() > 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let start_v = ev.eval_scalar(&args[0], ctx)?;
    let days_v = ev.eval_scalar(&args[1], ctx)?;
    let hol_v = if args.len() == 3 {
        Some(ev.eval_expr(&args[2], ctx)?)
    } else {
        None
    };
    let start = match coerce::to_number(&start_v) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let days = match coerce::to_number(&days_v) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let mut holidays = Vec::new();
    if let Some(v) = hol_v {
        if let Some(e) = collect_holiday_serials(&v, &mut holidays) {
            return Ok(ExcelValue::Error(e));
        }
    }
    match workday_serial(start, days, &holidays, ctx.spec.options.date_system) {
        Ok(n) => Ok(ExcelValue::Number(n)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn fn_workday_intl(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 || args.len() > 4 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let start_v = ev.eval_scalar(&args[0], ctx)?;
    let days_v = ev.eval_scalar(&args[1], ctx)?;
    let weekend_v = if args.len() >= 3 {
        Some(ev.eval_scalar(&args[2], ctx)?)
    } else {
        None
    };
    let hol_v = if args.len() == 4 {
        Some(ev.eval_expr(&args[3], ctx)?)
    } else {
        None
    };
    let start = match coerce::to_number(&start_v) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let days = match coerce::to_number(&days_v) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let weekend = match parse_weekend_mask(weekend_v.as_ref()) {
        Ok(m) => m,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let mut holidays = Vec::new();
    if let Some(v) = hol_v {
        if let Some(e) = collect_holiday_serials(&v, &mut holidays) {
            return Ok(ExcelValue::Error(e));
        }
    }
    match workday_serial_intl(
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

fn fn_weekday(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.is_empty() || args.len() > 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let serial = match coerce::to_number(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let return_type = if args.len() >= 2 {
        match coerce::to_number(&ev.eval_scalar(&args[1], ctx)?) {
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

fn fn_isoweeknum(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() != 1 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let serial = match coerce::to_number(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    match isoweeknum(serial, ctx.spec.options.date_system) {
        Ok(n) => Ok(ExcelValue::Number(n)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn fn_yearfrac(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 || args.len() > 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let start = match coerce::to_number(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let end = match coerce::to_number(&ev.eval_scalar(&args[1], ctx)?) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let basis = if args.len() >= 3 {
        match coerce::to_number(&ev.eval_scalar(&args[2], ctx)?) {
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

fn collect_holiday_serials(v: &ExcelValue, out: &mut Vec<f64>) -> Option<ExcelError> {
    match v {
        ExcelValue::Array(rows) => {
            for row in rows {
                for c in row {
                    if let Some(e) = collect_holiday_serials(c, out) {
                        return Some(e);
                    }
                }
            }
            None
        }
        ExcelValue::Empty => None,
        ExcelValue::Error(e) => Some(*e),
        other => match coerce::to_number(other) {
            Ok(n) => {
                out.push(n);
                None
            }
            Err(e) => Some(e),
        },
    }
}

fn fn_ymd(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
    part: YmdPart,
) -> Result<ExcelValue, EvalError> {
    if args.len() != 1 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let n = match coerce::to_number(&ev.eval_scalar(&args[0], ctx)?) {
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
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
    left: bool,
) -> Result<ExcelValue, EvalError> {
    if args.is_empty() {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let s = match coerce::to_text(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(s) => s,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let n = if args.len() >= 2 {
        match coerce::to_number(&ev.eval_scalar(&args[1], ctx)?) {
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

fn fn_mid(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let s = match coerce::to_text(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(s) => s,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let start = match coerce::to_number(&ev.eval_scalar(&args[1], ctx)?) {
        Ok(n) => n.trunc() as i64,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let len = match coerce::to_number(&ev.eval_scalar(&args[2], ctx)?) {
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

fn fn_len(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 1 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    match coerce::to_text(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(s) => Ok(ExcelValue::Number(s.chars().count() as f64)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn fn_proper(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 1 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    match coerce::to_text(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(s) => Ok(ExcelValue::Text(super::proper::proper(&s))),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn fn_lower(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 1 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    match coerce::to_text(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(s) => Ok(ExcelValue::Text(super::lower::lower_owned(s))),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn fn_upper(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 1 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    match coerce::to_text(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(s) => Ok(ExcelValue::Text(super::upper::upper(&s))),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn fn_trim(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 1 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    match coerce::to_text(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(s) => Ok(ExcelValue::Text(super::trim::trim(&s))),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn fn_clean(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 1 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    match coerce::to_text(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(s) => Ok(ExcelValue::Text(super::clean::clean_owned(s))),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn fn_exact(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let a = ev.eval_scalar(&args[0], ctx)?;
    let b = ev.eval_scalar(&args[1], ctx)?;
    match super::exact::exact(&a, &b) {
        Ok(eq) => Ok(ExcelValue::Bool(eq)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn fn_substitute(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() < 3 || args.len() > 4 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let text = match coerce::to_text(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(s) => s,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let old_text = match coerce::to_text(&ev.eval_scalar(&args[1], ctx)?) {
        Ok(s) => s,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let new_text = match coerce::to_text(&ev.eval_scalar(&args[2], ctx)?) {
        Ok(s) => s,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let instance = if args.len() == 4 {
        match coerce::to_number(&ev.eval_scalar(&args[3], ctx)?) {
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
    Ok(ExcelValue::Text(super::substitute::substitute(
        &text, &old_text, &new_text, instance,
    )))
}

fn fn_find(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 || args.len() > 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let find_text = match coerce::to_text(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(s) => s,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let within_text = match coerce::to_text(&ev.eval_scalar(&args[1], ctx)?) {
        Ok(s) => s,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let start_num = if args.len() == 3 {
        match coerce::to_number(&ev.eval_scalar(&args[2], ctx)?) {
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
    match super::find::find(&find_text, &within_text, start_num) {
        Ok(pos) => Ok(ExcelValue::Number(pos)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn fn_search(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 || args.len() > 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let find_text = match coerce::to_text(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(s) => s,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let within_text = match coerce::to_text(&ev.eval_scalar(&args[1], ctx)?) {
        Ok(s) => s,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let start_num = if args.len() == 3 {
        match coerce::to_number(&ev.eval_scalar(&args[2], ctx)?) {
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
    match super::search::search(&find_text, &within_text, start_num) {
        Ok(pos) => Ok(ExcelValue::Number(pos)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn fn_text(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let value = ev.eval_scalar(&args[0], ctx)?;
    if let ExcelValue::Error(e) = value {
        return Ok(ExcelValue::Error(e));
    }
    let fmt_v = ev.eval_scalar(&args[1], ctx)?;
    if let ExcelValue::Error(e) = fmt_v {
        return Ok(ExcelValue::Error(e));
    }
    let fmt = match coerce::to_text(&fmt_v) {
        Ok(s) => s,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    match text_format::apply(&value, &fmt, ctx.spec.options.date_system) {
        Ok(s) => Ok(ExcelValue::Text(s)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

/// Excel `TEXTAFTER(text, delimiter, [instance_num], [match_mode], [match_end], [if_not_found])`.
///
/// Arity 2..=6. `if_not_found` is evaluated when supplied but used only on a
/// miss (`#N/A` path). `#VALUE!` from `instance_num` is not replaced.
fn fn_textafter(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 || args.len() > 6 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let text = match coerce::to_text(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(s) => s,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let delim_v = ev.eval_expr(&args[1], ctx)?;
    let mut delims = Vec::new();
    if let Err(e) = collect_delim_texts(&delim_v, &mut delims) {
        return Ok(ExcelValue::Error(e));
    }
    let delim_refs: Vec<&str> = delims.iter().map(String::as_str).collect();
    let instance_num = if args.len() >= 3 {
        match coerce::to_number(&ev.eval_scalar(&args[2], ctx)?) {
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
        match coerce::to_logical(&ev.eval_scalar(&args[3], ctx)?) {
            Ok(b) => b,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        false
    };
    let match_end = if args.len() >= 5 {
        match coerce::to_logical(&ev.eval_scalar(&args[4], ctx)?) {
            Ok(b) => b,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        false
    };
    let if_not_found = if args.len() >= 6 {
        Some(ev.eval_expr(&args[5], ctx)?)
    } else {
        None
    };
    match super::textafter::textafter(&text, &delim_refs, instance_num, ignore_case, match_end) {
        Ok(s) => Ok(ExcelValue::Text(s)),
        Err(ExcelError::Na) => match if_not_found {
            Some(v) => Ok(v),
            None => Ok(ExcelValue::Error(ExcelError::Na)),
        },
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

/// Excel `TEXTBEFORE(text, delimiter, [instance_num], [match_mode], [match_end], [if_not_found])`.
///
/// Arity 2–6. Arguments evaluate left-to-right. `if_not_found` is returned
/// as-is on a miss (`#N/A` when omitted). `match_end` is applied inside the
/// kernel and wins over `if_not_found` when it supplies the virtual delimiter.
fn fn_textbefore(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 || args.len() > 6 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let text = match coerce::to_text(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(s) => s,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let delim_v = ev.eval_expr(&args[1], ctx)?;
    let mut delims = Vec::new();
    if let Err(e) = flatten_text_args(&delim_v, &mut delims) {
        return Ok(ExcelValue::Error(e));
    }
    let instance_num = if args.len() >= 3 {
        match coerce::to_number(&ev.eval_scalar(&args[2], ctx)?) {
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
        match coerce::to_number(&ev.eval_scalar(&args[3], ctx)?) {
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
        match coerce::to_number(&ev.eval_scalar(&args[4], ctx)?) {
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
        Some(ev.eval_expr(&args[5], ctx)?)
    } else {
        None
    };
    let delim_refs: Vec<&str> = delims.iter().map(String::as_str).collect();
    match super::textbefore::textbefore(
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

fn collect_delim_texts(v: &ExcelValue, out: &mut Vec<String>) -> Result<(), ExcelError> {
    match v {
        ExcelValue::Error(e) => Err(*e),
        ExcelValue::Array(rows) => {
            for row in rows {
                for cell in row {
                    collect_delim_texts(cell, out)?;
                }
            }
            Ok(())
        }
        other => {
            out.push(coerce::to_text(other)?);
            Ok(())
        }
    }
}

fn flatten_text_args(v: &ExcelValue, out: &mut Vec<String>) -> Result<(), ExcelError> {
    match v {
        ExcelValue::Error(e) => Err(*e),
        ExcelValue::Array(rows) => {
            for row in rows {
                for cell in row {
                    flatten_text_args(cell, out)?;
                }
            }
            Ok(())
        }
        other => {
            out.push(coerce::to_text(other)?);
            Ok(())
        }
    }
}

fn fn_replace(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 4 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let old_text = match coerce::to_text(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(s) => s,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let start_num = match coerce::to_number(&ev.eval_scalar(&args[1], ctx)?) {
        Ok(n) => match trunc_start_num(n) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        },
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let num_chars = match coerce::to_number(&ev.eval_scalar(&args[2], ctx)?) {
        Ok(n) => match trunc_num_chars(n) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        },
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let new_text = match coerce::to_text(&ev.eval_scalar(&args[3], ctx)?) {
        Ok(s) => s,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    Ok(ExcelValue::Text(super::replace::replace(
        &old_text, start_num, num_chars, &new_text,
    )))
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

fn fn_pmt(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    match tvm5(ev, args, ctx)? {
        Ok((rate, nper, pv, fv, typ)) => match excel_pmt(rate, nper, pv, fv, typ) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        },
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn fn_pv(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    match tvm5(ev, args, ctx)? {
        Ok((rate, nper, pmt, fv, typ)) => match excel_pv(rate, nper, pmt, fv, typ) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        },
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

/// Shared `rate, nper, third, [fv], [type]` coerce for `PMT` / `PV`.
fn tvm5(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<Result<(f64, f64, f64, f64, f64), ExcelError>, EvalError> {
    if args.len() < 3 || args.len() > 5 {
        return Ok(Err(ExcelError::Value));
    }
    let rate = match coerce_num(ev, &args[0], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(Err(e)),
    };
    let nper = match coerce_num(ev, &args[1], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(Err(e)),
    };
    let third = match coerce_num(ev, &args[2], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(Err(e)),
    };
    let fv = if args.len() >= 4 {
        match coerce_num(ev, &args[3], ctx)? {
            Ok(n) => n,
            Err(e) => return Ok(Err(e)),
        }
    } else {
        0.0
    };
    let typ = if args.len() >= 5 {
        match coerce_num(ev, &args[4], ctx)? {
            Ok(n) => n,
            Err(e) => return Ok(Err(e)),
        }
    } else {
        0.0
    };
    Ok(Ok((rate, nper, third, fv, typ)))
}

fn fn_fv(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() < 3 || args.len() > 5 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let rate = match coerce_num(ev, &args[0], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let nper = match coerce_num(ev, &args[1], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let pmt = match coerce_num(ev, &args[2], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let pv = if args.len() >= 4 {
        match coerce_num(ev, &args[3], ctx)? {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        0.0
    };
    let typ = if args.len() >= 5 {
        match coerce_num(ev, &args[4], ctx)? {
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

fn fn_nper(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() < 3 || args.len() > 5 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let rate = match coerce_num(ev, &args[0], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let pmt = match coerce_num(ev, &args[1], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let pv = match coerce_num(ev, &args[2], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let fv = if args.len() >= 4 {
        match coerce_num(ev, &args[3], ctx)? {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        0.0
    };
    let typ = if args.len() >= 5 {
        match coerce_num(ev, &args[4], ctx)? {
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

fn fn_rate(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() < 3 || args.len() > 6 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let nper = match coerce_num(ev, &args[0], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let pmt = match coerce_num(ev, &args[1], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let pv = match coerce_num(ev, &args[2], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let fv = if args.len() >= 4 {
        match coerce_num(ev, &args[3], ctx)? {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        0.0
    };
    let typ = if args.len() >= 5 {
        match coerce_num(ev, &args[4], ctx)? {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        0.0
    };
    let guess = if args.len() >= 6 {
        match coerce_num(ev, &args[5], ctx)? {
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

fn fn_ipmt(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() < 4 || args.len() > 6 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let rate = match coerce_num(ev, &args[0], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let per = match coerce_num(ev, &args[1], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let nper = match coerce_num(ev, &args[2], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let pv = match coerce_num(ev, &args[3], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let fv = if args.len() >= 5 {
        match coerce_num(ev, &args[4], ctx)? {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        0.0
    };
    let typ = if args.len() >= 6 {
        match coerce_num(ev, &args[5], ctx)? {
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

fn fn_ppmt(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() < 4 || args.len() > 6 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let rate = match coerce_num(ev, &args[0], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let per = match coerce_num(ev, &args[1], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let nper = match coerce_num(ev, &args[2], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let pv = match coerce_num(ev, &args[3], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let fv = if args.len() >= 5 {
        match coerce_num(ev, &args[4], ctx)? {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        0.0
    };
    let typ = if args.len() >= 6 {
        match coerce_num(ev, &args[5], ctx)? {
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

fn fn_cumprinc(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 6 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let rate = match coerce_num(ev, &args[0], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let nper = match coerce_num(ev, &args[1], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let pv = match coerce_num(ev, &args[2], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let start = match coerce_num(ev, &args[3], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let end = match coerce_num(ev, &args[4], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let typ = match coerce_num(ev, &args[5], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    match excel_cumprinc(rate, nper, pv, start, end, typ) {
        Ok(n) => Ok(ExcelValue::Number(n)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn fn_cumipmt(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 6 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let rate = match coerce_num(ev, &args[0], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let nper = match coerce_num(ev, &args[1], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let pv = match coerce_num(ev, &args[2], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let start = match coerce_num(ev, &args[3], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let end = match coerce_num(ev, &args[4], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let typ = match coerce_num(ev, &args[5], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    match excel_cumipmt(rate, nper, pv, start, end, typ) {
        Ok(n) => Ok(ExcelValue::Number(n)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn fn_effect(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let nominal = match coerce_num(ev, &args[0], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let npery = match coerce_num(ev, &args[1], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    match excel_effect(nominal, npery) {
        Ok(n) => Ok(ExcelValue::Number(n)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn fn_nominal(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let effect_rate = match coerce_num(ev, &args[0], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let npery = match coerce_num(ev, &args[1], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    match excel_nominal(effect_rate, npery) {
        Ok(n) => Ok(ExcelValue::Number(n)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn fn_pduration(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let rate = match coerce_num(ev, &args[0], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let pv = match coerce_num(ev, &args[1], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let fv = match coerce_num(ev, &args[2], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    match excel_pduration(rate, pv, fv) {
        Ok(n) => Ok(ExcelValue::Number(n)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn fn_rri(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let nper = match coerce_num(ev, &args[0], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let pv = match coerce_num(ev, &args[1], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let fv = match coerce_num(ev, &args[2], ctx)? {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    match excel_rri(nper, pv, fv) {
        Ok(n) => Ok(ExcelValue::Number(n)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn coerce_num(
    ev: &Evaluator,
    expr: &Expr,
    ctx: &mut Ctx<'_>,
) -> Result<Result<f64, ExcelError>, EvalError> {
    Ok(coerce::to_number(&ev.eval_scalar(expr, ctx)?))
}
fn fn_irr(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.is_empty() || args.len() > 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let from_range = args[0].is_reference();
    let values_v = ev.eval_expr(&args[0], ctx)?;
    let flows = match collect_cashflows(&values_v, from_range) {
        Ok(v) => v,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let guess = if args.len() == 2 {
        match coerce::to_number(&ev.eval_scalar(&args[1], ctx)?) {
            Ok(n) => n,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        0.1
    };
    match super::irr::irr(&flows, guess) {
        Some(r) => Ok(ExcelValue::Number(r)),
        None => Ok(ExcelValue::Error(ExcelError::Num)),
    }
}

fn fn_mirr(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let from_range = args[0].is_reference();
    let values_v = ev.eval_expr(&args[0], ctx)?;
    let flows = match collect_cashflows(&values_v, from_range) {
        Ok(v) => v,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let finance_rate = match coerce::to_number(&ev.eval_scalar(&args[1], ctx)?) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let reinvest_rate = match coerce::to_number(&ev.eval_scalar(&args[2], ctx)?) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    match super::mirr::mirr(&flows, finance_rate, reinvest_rate) {
        Ok(r) => Ok(ExcelValue::Number(r)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn collect_cashflows(v: &ExcelValue, from_range: bool) -> Result<Vec<f64>, ExcelError> {
    let mut out = Vec::new();
    collect_cashflows_into(v, from_range, &mut out)?;
    Ok(out)
}

fn collect_cashflows_into(
    v: &ExcelValue,
    from_range: bool,
    out: &mut Vec<f64>,
) -> Result<(), ExcelError> {
    match (v, from_range) {
        (ExcelValue::Array(rows), _) => {
            for row in rows {
                for c in row {
                    collect_cashflows_into(c, true, out)?;
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
        (other, false) => match coerce::to_number(other) {
            Ok(n) if n.is_finite() => {
                out.push(n);
                Ok(())
            }
            Ok(_) => Err(ExcelError::Num),
            Err(e) => Err(e),
        },
    }
}
fn fn_value(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> Result<ExcelValue, EvalError> {
    if args.len() != 1 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let v = ev.eval_scalar(&args[0], ctx)?;
    match v {
        ExcelValue::Number(n) => Ok(ExcelValue::Number(n)),
        ExcelValue::Bool(true) => Ok(ExcelValue::Number(1.0)),
        ExcelValue::Bool(false) => Ok(ExcelValue::Number(0.0)),
        ExcelValue::Empty => Ok(ExcelValue::Number(0.0)),
        ExcelValue::Text(s) => match coerce::parse_numeric_text(&s) {
            Ok(n) => Ok(ExcelValue::Number(n)),
            Err(e) => Ok(ExcelValue::Error(e)),
        },
        ExcelValue::Error(e) => Ok(ExcelValue::Error(e)),
        ExcelValue::Array(_) => Ok(ExcelValue::Error(ExcelError::Value)),
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

    fn fold(&mut self, v: &ExcelValue, from_range: bool) -> Option<ExcelError> {
        match (v, from_range) {
            (ExcelValue::Array(rows), _) => {
                for row in rows {
                    for c in row {
                        if let Some(e) = self.fold(c, true) {
                            return Some(e);
                        }
                    }
                }
                None
            }
            (ExcelValue::Error(e), _) => match self.kind {
                AggKind::CountA => {
                    self.counta += 1;
                    None
                }
                AggKind::Count | AggKind::CountBlank => None,
                _ => Some(*e),
            },
            (ExcelValue::Number(n), _) => {
                self.add_number(*n);
                self.counta += 1;
                None
            }
            (ExcelValue::Empty, _) => {
                self.countblank += 1;
                None
            }
            (ExcelValue::Bool(b), false) => {
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
            (ExcelValue::Bool(_), true) => {
                if matches!(self.kind, AggKind::CountA) {
                    self.counta += 1;
                }
                None
            }
            (ExcelValue::Text(s), false) => match self.kind {
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
                    if coerce::parse_numeric_text(s).is_ok() {
                        self.count += 1;
                    }
                    None
                }
                _ => match coerce::parse_numeric_text(s) {
                    Ok(n) => {
                        self.add_number(n);
                        None
                    }
                    Err(e) => Some(e),
                },
            },
            (ExcelValue::Text(s), true) => {
                match self.kind {
                    AggKind::CountA => self.counta += 1,
                    AggKind::CountBlank if s.is_empty() => self.countblank += 1,
                    _ => {}
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

fn fold_logicals(v: &ExcelValue, seen: &mut usize, true_count: &mut usize) -> Option<ExcelError> {
    match v {
        ExcelValue::Array(rows) => {
            for row in rows {
                for c in row {
                    if let Some(e) = fold_logicals(c, seen, true_count) {
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
        ExcelValue::Text(_) => Some(ExcelError::Value),
    }
}

fn lookup_key_match(lookup: &ExcelValue, key: &ExcelValue) -> bool {
    if let ExcelValue::Text(pat) = lookup {
        if looks_like_wildcard(pat) {
            let key_text = match key {
                ExcelValue::Text(s) => s.clone(),
                ExcelValue::Number(n) => coerce::format_plain(*n),
                ExcelValue::Bool(true) => "TRUE".into(),
                ExcelValue::Bool(false) => "FALSE".into(),
                ExcelValue::Empty => String::new(),
                _ => return false,
            };
            return excel_wildcard(pat, &key_text);
        }
    }
    compare::equal(lookup, key)
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

fn approx_upper_bound(rows: &[Vec<ExcelValue>], lookup: &ExcelValue) -> Option<usize> {
    if rows.is_empty() {
        return None;
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
    compare::ordered(key, lookup, std::cmp::Ordering::Greater, true)
}

fn excel_geq(key: &ExcelValue, lookup: &ExcelValue) -> bool {
    compare::ordered(key, lookup, std::cmp::Ordering::Less, true)
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
