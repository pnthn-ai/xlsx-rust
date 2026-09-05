//! Excel `NPV(rate, value1, [value2], …)` kernel.
//!
//! Desktop Excel semantics (no golden-reading):
//! - Discount starts at period 1: `Σ value_i / (1+rate)^i`.
//! - Range / cell / name / array entries keep **numbers only**. Blanks, text
//!   (including numeric-looking text), and logicals are skipped and do **not**
//!   consume a period — later cash flows slide forward.
//! - Scalar (non-reference) arguments coerce like `SUM`: `TRUE`→1, `"100"`→100,
//!   non-numeric text → `#VALUE!`.
//! - Errors propagate left-to-right; they are not skipped.
//! - `rate = -1` with at least one kept cash flow is `#DIV/0!`.
//!
//! Production path streams numbers through a running discount factor (no
//! `pow` per period, no `Vec` of cash flows). The `powi` baseline lives beside
//! it so benches can print a before/after.

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{CellAddr, CellRef, EvalError, ExcelError, ExcelValue, RangeRef};

/// Horner / reverse-accumulate form used on packed slices.
///
/// Equivalent to `Σ values[i] / (1+rate)^(i+1)`.
pub fn npv(rate: f64, values: &[f64]) -> Result<f64, ExcelError> {
    if !rate.is_finite() {
        return Err(ExcelError::Num);
    }
    if values.is_empty() {
        return Ok(0.0);
    }
    if rate == -1.0 {
        return Err(ExcelError::Div0);
    }
    let one = 1.0 + rate;
    let mut acc = 0.0;
    for &v in values.iter().rev() {
        acc = (acc + v) / one;
        if !acc.is_finite() {
            return Err(ExcelError::Num);
        }
    }
    Ok(acc)
}

/// Quadratic-ish baseline: a `powi` per period. Same Excel formula as [`npv`].
pub fn npv_naive(rate: f64, values: &[f64]) -> Result<f64, ExcelError> {
    if !rate.is_finite() {
        return Err(ExcelError::Num);
    }
    if values.is_empty() {
        return Ok(0.0);
    }
    if rate == -1.0 {
        return Err(ExcelError::Div0);
    }
    let one = 1.0 + rate;
    let mut sum = 0.0;
    for (i, &v) in values.iter().enumerate() {
        let period = (i + 1) as i32;
        sum += v / one.powi(period);
        if !sum.is_finite() {
            return Err(ExcelError::Num);
        }
    }
    Ok(sum)
}

/// Streaming accumulator used by the evaluator (forward pass, no allocation).
pub struct NpvAcc {
    rate: f64,
    one_plus: f64,
    factor: f64,
    sum: f64,
}

impl NpvAcc {
    pub fn new(rate: f64) -> Self {
        Self {
            rate,
            one_plus: 1.0 + rate,
            factor: 1.0,
            sum: 0.0,
        }
    }

    pub fn push(&mut self, v: f64) -> Result<(), ExcelError> {
        if self.rate == -1.0 {
            return Err(ExcelError::Div0);
        }
        self.factor *= self.one_plus;
        self.sum += v / self.factor;
        if !self.sum.is_finite() {
            return Err(ExcelError::Num);
        }
        Ok(())
    }

    pub fn finish(self) -> Result<f64, ExcelError> {
        if !self.sum.is_finite() {
            return Err(ExcelError::Num);
        }
        Ok(self.sum)
    }
}

pub(crate) fn eval(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.len() < 2 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    let rate = match coerce::to_number(&ev.eval_scalar(&args[0], ctx)?) {
        Ok(n) => n,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };
    if !rate.is_finite() {
        return Ok(ExcelValue::Error(ExcelError::Num));
    }
    let mut acc = NpvAcc::new(rate);
    for arg in &args[1..] {
        if let Some(e) = feed_arg(ev, arg, ctx, &mut acc)? {
            return Ok(ExcelValue::Error(e));
        }
    }
    match acc.finish() {
        Ok(n) => Ok(ExcelValue::Number(n)),
        Err(e) => Ok(ExcelValue::Error(e)),
    }
}

fn feed_arg(
    ev: &Evaluator,
    arg: &Expr,
    ctx: &mut Ctx<'_>,
    acc: &mut NpvAcc,
) -> Result<Option<ExcelError>, EvalError> {
    match arg {
        Expr::Cell(c) => feed_cell(ev, c, ctx, acc),
        Expr::Range(r) => feed_range(ev, r, ctx, acc),
        other => {
            let from_range = other.is_reference();
            let v = ev.eval_expr(other, ctx)?;
            Ok(feed_value(&v, from_range, acc))
        }
    }
}

fn feed_range(
    ev: &Evaluator,
    range: &RangeRef,
    ctx: &mut Ctx<'_>,
    acc: &mut NpvAcc,
) -> Result<Option<ExcelError>, EvalError> {
    let sheet_name = range
        .sheet
        .clone()
        .unwrap_or_else(|| ctx.current_sheet.clone());
    let mut a1 = [0u8; 16];
    for addr in range.cells() {
        if let Some(e) = feed_sheet_cell(ev, &sheet_name, addr, ctx, acc, &mut a1)? {
            return Ok(Some(e));
        }
    }
    Ok(None)
}

fn feed_cell(
    ev: &Evaluator,
    cell: &CellRef,
    ctx: &mut Ctx<'_>,
    acc: &mut NpvAcc,
) -> Result<Option<ExcelError>, EvalError> {
    let sheet_name = cell
        .sheet
        .clone()
        .unwrap_or_else(|| ctx.current_sheet.clone());
    let mut a1 = [0u8; 16];
    feed_sheet_cell(ev, &sheet_name, cell.addr, ctx, acc, &mut a1)
}

fn feed_sheet_cell(
    ev: &Evaluator,
    sheet_name: &str,
    addr: CellAddr,
    ctx: &mut Ctx<'_>,
    acc: &mut NpvAcc,
    a1: &mut [u8; 16],
) -> Result<Option<ExcelError>, EvalError> {
    let key = format_a1(addr, a1);
    let peeked = {
        let sheet = match ctx.spec.workbook.sheet(Some(sheet_name)) {
            Ok(s) => s,
            Err(_) => return Ok(Some(ExcelError::Ref)),
        };
        match sheet.cells.get(key) {
            Some(c) if c.formula.is_some() => None,
            Some(c) => Some(c.value.clone().unwrap_or(ExcelValue::Empty)),
            None => Some(ExcelValue::Empty),
        }
    };
    let v = match peeked {
        Some(v) => v,
        None => ev.eval_cell(
            &CellRef {
                sheet: Some(sheet_name.to_string()),
                addr,
            },
            ctx,
        )?,
    };
    Ok(feed_value(&v, true, acc))
}

fn feed_value(v: &ExcelValue, from_range: bool, acc: &mut NpvAcc) -> Option<ExcelError> {
    match classify(v, from_range) {
        Collect::Take(n) => acc.push(n).err(),
        Collect::Skip => None,
        Collect::Error(e) => Some(e),
        Collect::Array(rows) => {
            for row in rows {
                for c in row {
                    if let Some(e) = feed_value(c, true, acc) {
                        return Some(e);
                    }
                }
            }
            None
        }
    }
}

enum Collect<'a> {
    Take(f64),
    Skip,
    Error(ExcelError),
    Array(&'a [Vec<ExcelValue>]),
}

fn classify<'a>(v: &'a ExcelValue, from_range: bool) -> Collect<'a> {
    match (v, from_range) {
        (ExcelValue::Array(rows), _) => Collect::Array(rows),
        (ExcelValue::Error(e), _) => Collect::Error(*e),
        (ExcelValue::Number(n), _) => Collect::Take(*n),
        (ExcelValue::Empty, _) => Collect::Skip,
        (ExcelValue::Bool(b), false) => Collect::Take(if *b { 1.0 } else { 0.0 }),
        (ExcelValue::Bool(_), true) => Collect::Skip,
        (ExcelValue::Text(s), false) => match coerce::parse_numeric_text(s) {
            Ok(n) => Collect::Take(n),
            Err(e) => Collect::Error(e),
        },
        (ExcelValue::Text(_), true) => Collect::Skip,
    }
}

fn format_a1(addr: CellAddr, buf: &mut [u8; 16]) -> &str {
    let mut col = addr.col + 1;
    let mut tmp = [0u8; 4];
    let mut n = 0usize;
    while col > 0 {
        col -= 1;
        tmp[n] = b'A' + (col % 26) as u8;
        col /= 26;
        n += 1;
    }
    let mut i = 0usize;
    for k in (0..n).rev() {
        buf[i] = tmp[k];
        i += 1;
    }
    let mut row = addr.row + 1;
    let mut digits = [0u8; 10];
    let mut d = 0usize;
    if row == 0 {
        digits[0] = b'0';
        d = 1;
    } else {
        while row > 0 {
            digits[d] = b'0' + (row % 10) as u8;
            row /= 10;
            d += 1;
        }
    }
    for k in (0..d).rev() {
        buf[i] = digits[k];
        i += 1;
    }
    std::str::from_utf8(&buf[..i]).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use xlsx_types::{Cell, Sheet, Workbook};

    fn close(a: f64, b: f64) {
        let scale = a.abs().max(b.abs()).max(1.0);
        assert!((a - b).abs() / scale < 1e-12, "npv mismatch: {a} vs {b}");
    }

    #[test]
    fn microsoft_example_1() {
        let v = [-10000.0, 3000.0, 4200.0, 6800.0];
        close(npv(0.1, &v).unwrap(), 1188.44341233522);
        close(npv_naive(0.1, &v).unwrap(), npv(0.1, &v).unwrap());
    }

    #[test]
    fn empty_and_rate_neg_one() {
        assert_eq!(npv(0.1, &[]).unwrap(), 0.0);
        assert_eq!(npv(-1.0, &[100.0]), Err(ExcelError::Div0));
        assert_eq!(npv(-1.0, &[]).unwrap(), 0.0);
    }

    #[test]
    fn streaming_matches_horner() {
        let vals = [100.0, 0.0, -50.0, 200.0];
        let packed = npv(0.08, &vals).unwrap();
        let mut acc = NpvAcc::new(0.08);
        for v in vals {
            acc.push(v).unwrap();
        }
        close(acc.finish().unwrap(), packed);
        close(npv_naive(0.08, &vals).unwrap(), packed);
    }

    #[test]
    fn workbook_range_skips_blank_and_text() {
        let mut sheet = Sheet::new("Sheet1");
        sheet
            .cells
            .insert("A1".into(), Cell::value(ExcelValue::Number(100.0)));
        sheet
            .cells
            .insert("A2".into(), Cell::value(ExcelValue::Text("x".into())));
        sheet
            .cells
            .insert("A3".into(), Cell::value(ExcelValue::Number(200.0)));
        let wb = Workbook {
            sheets: vec![sheet],
            names: vec![],
        };
        let skipped = crate::eval::eval_formula_in(&wb, "=NPV(0.1,A1:A3)").unwrap();
        let compact = crate::eval::eval_formula_in(&wb, "=NPV(0.1,100,200)").unwrap();
        assert_eq!(skipped, compact);
        match (skipped, compact) {
            (ExcelValue::Number(a), ExcelValue::Number(b)) => close(a, b),
            other => panic!("expected numbers, got {other:?}"),
        }
    }
}
