//! `AVERAGEIF` with Excel range-walk semantics (not array-literal compatible).
//!
//! Fast path: compile criteria once, walk `range` / reshaped `average_range`
//! without materializing 2-D arrays, and read stored values without the
//! circular-ref visiting set. No matches (or no numeric average cells) → `#DIV/0!`.

use super::{Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{
    CellAddr, CellRef, Criterion, EvalError, ExcelError, ExcelValue, RangeRef, Sheet,
};

pub(super) fn fn_averageif(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 || args.len() > 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let crit_val = compile_averageif_criteria(ev.eval_scalar(&args[1], ctx)?);
    let criterion = match Criterion::compile(&crit_val) {
        Ok(c) => c,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let range = match resolve_if_range(&args[0], ctx) {
        Ok(r) => r,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let avg_origin = if args.len() == 3 {
        match resolve_if_range(&args[2], ctx) {
            Ok(r) => r,
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
    } else {
        range.clone()
    };
    averageif_walk(ev, ctx, &range, &avg_origin, &criterion)
}

/// Materializing implementation kept for the Criterion microbench "before".
pub(crate) fn averageif_materialized(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 || args.len() > 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let crit_val = compile_averageif_criteria(ev.eval_scalar(&args[1], ctx)?);
    let criterion = match Criterion::compile(&crit_val) {
        Ok(c) => c,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    let range_v = ev.eval_expr(&args[0], ctx)?;
    if let ExcelValue::Error(e) = range_v {
        return Ok(ExcelValue::Error(e));
    }
    let avg_v = if args.len() == 3 {
        ev.eval_expr(&args[2], ctx)?
    } else {
        range_v.clone()
    };
    if let ExcelValue::Error(e) = avg_v {
        return Ok(ExcelValue::Error(e));
    }
    Ok(fold_arrays(&range_v, &avg_v, &criterion))
}

/// Microsoft: an empty criteria *cell* is treated as the number 0.
/// Text `""` / `"="` still match blanks (handled by [`Criterion`]).
fn compile_averageif_criteria(v: ExcelValue) -> ExcelValue {
    match v {
        ExcelValue::Empty => ExcelValue::Number(0.0),
        other => other,
    }
}

fn fold_arrays(range: &ExcelValue, avgs: &ExcelValue, criterion: &Criterion) -> ExcelValue {
    let rrows = as_rows(range);
    let arows = as_rows(avgs);
    if rrows.is_empty() {
        return ExcelValue::Error(ExcelError::Div0);
    }
    let height = rrows.len();
    let width = rrows[0].len();
    let mut sum = 0.0;
    let mut count = 0u64;
    for r in 0..height {
        for c in 0..width {
            let cell = rrows
                .get(r)
                .and_then(|row| row.get(c))
                .unwrap_or(&ExcelValue::Empty);
            if !criterion.matches(cell) {
                continue;
            }
            let add = arows
                .get(r)
                .and_then(|row| row.get(c))
                .unwrap_or(&ExcelValue::Empty);
            match add {
                ExcelValue::Error(e) => return ExcelValue::Error(*e),
                ExcelValue::Number(n) => {
                    sum += n;
                    count += 1;
                }
                _ => {}
            }
        }
    }
    finish_average(sum, count)
}

fn finish_average(sum: f64, count: u64) -> ExcelValue {
    if count == 0 {
        ExcelValue::Error(ExcelError::Div0)
    } else {
        ExcelValue::Number(sum / count as f64)
    }
}

fn as_rows(v: &ExcelValue) -> Vec<Vec<ExcelValue>> {
    match v {
        ExcelValue::Array(rows) => rows.clone(),
        other => vec![vec![other.clone()]],
    }
}

fn resolve_if_range(expr: &Expr, ctx: &Ctx<'_>) -> Result<RangeRef, ExcelError> {
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

fn averageif_walk(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    range: &RangeRef,
    avg_origin: &RangeRef,
    criterion: &Criterion,
) -> Result<ExcelValue, EvalError> {
    let crit_sheet = range
        .sheet
        .clone()
        .unwrap_or_else(|| ctx.current_sheet.clone());
    let avg_sheet = avg_origin
        .sheet
        .clone()
        .unwrap_or_else(|| ctx.current_sheet.clone());
    if ctx.spec.workbook.sheet(Some(&crit_sheet)).is_err()
        || ctx.spec.workbook.sheet(Some(&avg_sheet)).is_err()
    {
        return Ok(ExcelValue::Error(ExcelError::Ref));
    }

    match walk_stored_only(ctx, &crit_sheet, &avg_sheet, range, avg_origin, criterion) {
        Ok(Some(v)) => return Ok(v),
        Ok(None) => {}
        Err(e) => return Ok(ExcelValue::Error(e)),
    }

    let height = range.row_count();
    let width = range.col_count();
    let mut sum = 0.0;
    let mut count = 0u64;
    let mut a1 = String::with_capacity(8);

    for dr in 0..height {
        for dc in 0..width {
            let crit_addr = CellAddr::new(range.start.col + dc, range.start.row + dr);
            let crit_v = read_cell(ev, ctx, &crit_sheet, crit_addr, &mut a1)?;
            if !criterion.matches(&crit_v) {
                continue;
            }
            let avg_addr = CellAddr::new(avg_origin.start.col + dc, avg_origin.start.row + dr);
            let avg_v = read_cell(ev, ctx, &avg_sheet, avg_addr, &mut a1)?;
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
    Ok(finish_average(sum, count))
}

/// Single-borrow walk when no cell in either window is a formula.
/// Returns `Ok(None)` so the caller can fall back to `eval_cell`.
fn walk_stored_only(
    ctx: &Ctx<'_>,
    crit_sheet: &str,
    avg_sheet: &str,
    range: &RangeRef,
    avg_origin: &RangeRef,
    criterion: &Criterion,
) -> Result<Option<ExcelValue>, ExcelError> {
    let crit_sh = ctx
        .spec
        .workbook
        .sheet(Some(crit_sheet))
        .map_err(|_| ExcelError::Ref)?;
    let avg_sh = ctx
        .spec
        .workbook
        .sheet(Some(avg_sheet))
        .map_err(|_| ExcelError::Ref)?;
    let height = range.row_count();
    let width = range.col_count();
    let mut sum = 0.0;
    let mut count = 0u64;
    let mut a1 = String::with_capacity(8);
    for dr in 0..height {
        for dc in 0..width {
            let crit_addr = CellAddr::new(range.start.col + dc, range.start.row + dr);
            match lookup_stored(crit_sh, crit_addr, &mut a1) {
                Stored::NeedsEval => return Ok(None),
                Stored::MissingSheet => return Err(ExcelError::Ref),
                Stored::Value(v) if !criterion.matches(&v) => continue,
                Stored::Value(_) => {}
            }
            let avg_addr = CellAddr::new(avg_origin.start.col + dc, avg_origin.start.row + dr);
            match lookup_stored(avg_sh, avg_addr, &mut a1) {
                Stored::NeedsEval => return Ok(None),
                Stored::MissingSheet => return Err(ExcelError::Ref),
                Stored::Value(ExcelValue::Error(e)) => return Ok(Some(ExcelValue::Error(e))),
                Stored::Value(ExcelValue::Number(n)) => {
                    sum += n;
                    count += 1;
                }
                Stored::Value(_) => {}
            }
        }
    }
    Ok(Some(finish_average(sum, count)))
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
