//! `SUMIF` with Excel range-walk semantics (not array-literal compatible).
//!
//! Fast path: compile criteria once, walk `range` / reshaped `sum_range`
//! without materializing 2-D arrays, and read stored values without the
//! circular-ref visiting set.

use super::{Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{
    CellAddr, CellRef, Criterion, EvalError, ExcelError, ExcelValue, RangeRef, Sheet,
};

pub(super) fn fn_sumif(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 || args.len() > 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let crit_val = ev.eval_scalar(&args[1], ctx)?;
    let criterion = match Criterion::compile(&crit_val) {
        Ok(c) => c,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let range = match resolve_sumif_range(&args[0], ctx) {
        Ok(r) => r,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let sum_origin = if args.len() == 3 {
        match resolve_sumif_range(&args[2], ctx) {
            Ok(r) => r,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        range.clone()
    };
    sumif_walk(ev, ctx, &range, &sum_origin, &criterion)
}

/// Materializing implementation kept for the Criterion microbench "before".
pub(crate) fn sumif_materialized(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 || args.len() > 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let crit_val = ev.eval_scalar(&args[1], ctx)?;
    let criterion = match Criterion::compile(&crit_val) {
        Ok(c) => c,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let range_v = ev.eval_expr(&args[0], ctx)?;
    if let ExcelValue::Error(e) = range_v {
        return Ok(ExcelValue::Error(e));
    }
    let sum_v = if args.len() == 3 {
        ev.eval_expr(&args[2], ctx)?
    } else {
        range_v.clone()
    };
    if let ExcelValue::Error(e) = sum_v {
        return Ok(ExcelValue::Error(e));
    }
    Ok(fold_arrays(&range_v, &sum_v, &criterion))
}

fn fold_arrays(range: &ExcelValue, sums: &ExcelValue, criterion: &Criterion) -> ExcelValue {
    let rrows = as_rows(range);
    let srows = as_rows(sums);
    if rrows.is_empty() {
        return ExcelValue::Number(0.0);
    }
    let height = rrows.len();
    let width = rrows[0].len();
    let mut acc = 0.0;
    for r in 0..height {
        for c in 0..width {
            let cell = rrows
                .get(r)
                .and_then(|row| row.get(c))
                .unwrap_or(&ExcelValue::Empty);
            if !criterion.matches(cell) {
                continue;
            }
            let add = srows
                .get(r)
                .and_then(|row| row.get(c))
                .unwrap_or(&ExcelValue::Empty);
            match add {
                ExcelValue::Error(e) => return ExcelValue::Error(*e),
                ExcelValue::Number(n) => acc += n,
                _ => {}
            }
        }
    }
    ExcelValue::Number(acc)
}

fn as_rows(v: &ExcelValue) -> Vec<Vec<ExcelValue>> {
    match v {
        ExcelValue::Array(rows) => rows.clone(),
        other => vec![vec![other.clone()]],
    }
}

fn resolve_sumif_range(expr: &Expr, ctx: &Ctx<'_>) -> Result<RangeRef, ExcelError> {
    match expr {
        Expr::Range(r) => Ok(r.clone()),
        Expr::Cell(c) => Ok(RangeRef::new(c.sheet.clone(), c.addr, c.addr)),
        Expr::Name(n) => named_as_range(n, ctx),
        _ => Err(ExcelError::Value),
    }
}

fn named_as_range(name: &str, ctx: &Ctx<'_>) -> Result<RangeRef, ExcelError> {
    let def = ctx
        .spec
        .workbook
        .defined_name(name)
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

fn sumif_walk(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    range: &RangeRef,
    sum_origin: &RangeRef,
    criterion: &Criterion,
) -> Result<ExcelValue, EvalError> {
    let crit_sheet = range
        .sheet
        .clone()
        .unwrap_or_else(|| ctx.current_sheet.clone());
    let sum_sheet = sum_origin
        .sheet
        .clone()
        .unwrap_or_else(|| ctx.current_sheet.clone());
    if ctx.spec.workbook.sheet(Some(&crit_sheet)).is_err()
        || ctx.spec.workbook.sheet(Some(&sum_sheet)).is_err()
    {
        return Ok(ExcelValue::Error(ExcelError::Ref));
    }

    let height = range.row_count();
    let width = range.col_count();
    let mut acc = 0.0;
    let mut a1 = String::with_capacity(8);

    for dr in 0..height {
        for dc in 0..width {
            let crit_addr = CellAddr::new(range.start.col + dc, range.start.row + dr);
            let crit_v = read_cell(ev, ctx, &crit_sheet, crit_addr, &mut a1)?;
            if !criterion.matches(&crit_v) {
                continue;
            }
            let sum_addr = CellAddr::new(sum_origin.start.col + dc, sum_origin.start.row + dr);
            let sum_v = read_cell(ev, ctx, &sum_sheet, sum_addr, &mut a1)?;
            match sum_v {
                ExcelValue::Error(e) => return Ok(ExcelValue::Error(e)),
                ExcelValue::Number(n) => acc += n,
                _ => {}
            }
        }
    }
    Ok(ExcelValue::Number(acc))
}

fn read_cell(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    sheet_name: &str,
    addr: CellAddr,
    a1_buf: &mut String,
) -> Result<ExcelValue, EvalError> {
    match stored_value(ctx, sheet_name, addr, a1_buf) {
        Stored::Value(v) => Ok(v),
        Stored::NeedsEval => ev.eval_cell(
            &CellRef {
                sheet: Some(sheet_name.to_string()),
                addr,
            },
            ctx,
        ),
        Stored::MissingSheet => Ok(ExcelValue::Error(ExcelError::Ref)),
    }
}

enum Stored {
    Value(ExcelValue),
    NeedsEval,
    MissingSheet,
}

fn stored_value(ctx: &Ctx<'_>, sheet_name: &str, addr: CellAddr, a1_buf: &mut String) -> Stored {
    let sheet = match ctx.spec.workbook.sheet(Some(sheet_name)) {
        Ok(s) => s,
        Err(_) => return Stored::MissingSheet,
    };
    lookup_stored(sheet, addr, a1_buf)
}

fn lookup_stored(sheet: &Sheet, addr: CellAddr, a1_buf: &mut String) -> Stored {
    a1_buf.clear();
    addr.write_a1(a1_buf);
    match sheet.cells.get(a1_buf) {
        Some(c) if c.formula.is_some() => Stored::NeedsEval,
        Some(c) => Stored::Value(c.value.clone().unwrap_or(ExcelValue::Empty)),
        None => Stored::Value(ExcelValue::Empty),
    }
}
