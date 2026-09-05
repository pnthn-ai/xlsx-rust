//! `AVERAGEIFS` with Excel multi-criteria AND and same-shape range rules.
//!
//! Fast path: compile each criterion once, walk `average_range` and every
//! `criteria_range` without materializing 2-D arrays, and read stored values
//! without the circular-ref visiting set. Unlike `AVERAGEIF`, ranges must
//! share dimensions — Excel does not reshape from the top-left.
//!
//! No matches (or no numeric average cells) → `#DIV/0!` (unlike `SUMIFS`).

use super::{Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{
    CellAddr, CellRef, Criterion, EvalError, ExcelError, ExcelValue, RangeRef, Sheet,
};

pub(super) fn fn_averageifs(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    match prepare_averageifs(ev, args, ctx) {
        Ok(prepared) => averageifs_walk(ev, ctx, &prepared),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

/// Materializing implementation kept for the Criterion microbench "before".
pub(crate) fn averageifs_materialized(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() < 3 || args.len() % 2 == 0 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let mut criteria = Vec::with_capacity(args.len() / 2);
    let mut i = 1;
    while i < args.len() {
        let crit_val = ev.eval_scalar(&args[i + 1], ctx)?;
        match Criterion::compile(&crit_val) {
            Ok(c) => criteria.push(c),
            Err(e) => return Ok(ExcelValue::Error(e)),
        }
        i += 2;
    }
    let avg_v = ev.eval_expr(&args[0], ctx)?;
    if let ExcelValue::Error(e) = avg_v {
        return Ok(ExcelValue::Error(e));
    }
    let mut crit_vals = Vec::with_capacity(criteria.len());
    let mut i = 1;
    while i < args.len() {
        let v = ev.eval_expr(&args[i], ctx)?;
        if let ExcelValue::Error(e) = v {
            return Ok(ExcelValue::Error(e));
        }
        crit_vals.push(v);
        i += 2;
    }
    Ok(fold_arrays(&avg_v, &crit_vals, &criteria))
}

struct Prepared {
    avg: RangeRef,
    pairs: Vec<(RangeRef, Criterion)>,
}

fn prepare_averageifs(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<Prepared, ExcelError> {
    if args.len() < 3 || args.len() % 2 == 0 {
        return Err(ExcelError::Value);
    }
    let avg = resolve_averageifs_range(&args[0], ctx)?;
    let mut pairs = Vec::with_capacity(args.len() / 2);
    let mut i = 1;
    while i < args.len() {
        let range = resolve_averageifs_range(&args[i], ctx)?;
        if range.row_count() != avg.row_count() || range.col_count() != avg.col_count() {
            return Err(ExcelError::Value);
        }
        let crit_val = ev
            .eval_scalar(&args[i + 1], ctx)
            .map_err(|_| ExcelError::Value)?;
        if let ExcelValue::Error(e) = &crit_val {
            return Err(*e);
        }
        let criterion = Criterion::compile(&crit_val)?;
        pairs.push((range, criterion));
        i += 2;
    }
    Ok(Prepared { avg, pairs })
}

fn fold_arrays(avg: &ExcelValue, crits: &[ExcelValue], criteria: &[Criterion]) -> ExcelValue {
    let arows = as_rows(avg);
    if arows.is_empty() {
        return ExcelValue::Error(ExcelError::Div0);
    }
    let height = arows.len();
    let width = arows[0].len();
    let crit_rows: Vec<Vec<Vec<ExcelValue>>> = crits.iter().map(as_rows).collect();
    for rows in &crit_rows {
        if rows.len() != height || rows.first().map(|r| r.len()).unwrap_or(0) != width {
            return ExcelValue::Error(ExcelError::Value);
        }
    }
    let mut sum = 0.0;
    let mut count = 0u64;
    for r in 0..height {
        for c in 0..width {
            let mut ok = true;
            for (rows, criterion) in crit_rows.iter().zip(criteria.iter()) {
                let cell = rows
                    .get(r)
                    .and_then(|row| row.get(c))
                    .unwrap_or(&ExcelValue::Empty);
                if !criterion.matches(cell) {
                    ok = false;
                    break;
                }
            }
            if !ok {
                continue;
            }
            match arows
                .get(r)
                .and_then(|row| row.get(c))
                .unwrap_or(&ExcelValue::Empty)
            {
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

fn resolve_averageifs_range(expr: &Expr, ctx: &Ctx<'_>) -> Result<RangeRef, ExcelError> {
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

fn averageifs_walk(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    prepared: &Prepared,
) -> Result<ExcelValue, EvalError> {
    let avg_sheet = prepared
        .avg
        .sheet
        .clone()
        .unwrap_or_else(|| ctx.current_sheet.clone());
    let crit_sheets: Vec<String> = prepared
        .pairs
        .iter()
        .map(|(r, _)| {
            r.sheet
                .clone()
                .unwrap_or_else(|| ctx.current_sheet.clone())
        })
        .collect();
    if ctx.spec.workbook.sheet(Some(&avg_sheet)).is_err()
        || crit_sheets
            .iter()
            .any(|s| ctx.spec.workbook.sheet(Some(s)).is_err())
    {
        return Ok(ExcelValue::Error(ExcelError::Ref));
    }

    match walk_stored_only(ctx, &avg_sheet, &crit_sheets, prepared) {
        Ok(Some(v)) => return Ok(v),
        Ok(None) => {}
        Err(e) => return Ok(ExcelValue::Error(e)),
    }

    let height = prepared.avg.row_count();
    let width = prepared.avg.col_count();
    let mut sum = 0.0;
    let mut count = 0u64;
    let mut a1 = String::with_capacity(8);

    for dr in 0..height {
        for dc in 0..width {
            let mut ok = true;
            for ((range, criterion), sheet) in prepared.pairs.iter().zip(crit_sheets.iter()) {
                let addr = CellAddr::new(range.start.col + dc, range.start.row + dr);
                let v = read_cell(ev, ctx, sheet, addr, &mut a1)?;
                if !criterion.matches(&v) {
                    ok = false;
                    break;
                }
            }
            if !ok {
                continue;
            }
            let avg_addr = CellAddr::new(prepared.avg.start.col + dc, prepared.avg.start.row + dr);
            match read_cell(ev, ctx, &avg_sheet, avg_addr, &mut a1)? {
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

/// Single-borrow walk when no cell in any window is a formula.
/// Returns `Ok(None)` so the caller can fall back to `eval_cell`.
fn walk_stored_only(
    ctx: &Ctx<'_>,
    avg_sheet: &str,
    crit_sheets: &[String],
    prepared: &Prepared,
) -> Result<Option<ExcelValue>, ExcelError> {
    let avg_sh = ctx
        .spec
        .workbook
        .sheet(Some(avg_sheet))
        .map_err(|_| ExcelError::Ref)?;
    let crit_sh: Vec<&Sheet> = crit_sheets
        .iter()
        .map(|s| {
            ctx.spec
                .workbook
                .sheet(Some(s))
                .map_err(|_| ExcelError::Ref)
        })
        .collect::<Result<_, _>>()?;

    let height = prepared.avg.row_count();
    let width = prepared.avg.col_count();
    let mut sum = 0.0;
    let mut count = 0u64;
    let mut a1 = String::with_capacity(8);
    for dr in 0..height {
        for dc in 0..width {
            let mut ok = true;
            for ((range, criterion), sheet) in prepared.pairs.iter().zip(crit_sh.iter()) {
                let addr = CellAddr::new(range.start.col + dc, range.start.row + dr);
                match lookup_stored(sheet, addr, &mut a1) {
                    Stored::NeedsEval => return Ok(None),
                    Stored::MissingSheet => return Err(ExcelError::Ref),
                    Stored::Value(v) if !criterion.matches(&v) => {
                        ok = false;
                        break;
                    }
                    Stored::Value(_) => {}
                }
            }
            if !ok {
                continue;
            }
            let avg_addr = CellAddr::new(prepared.avg.start.col + dc, prepared.avg.start.row + dr);
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
