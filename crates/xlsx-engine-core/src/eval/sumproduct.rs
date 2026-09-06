//! Excel `SUMPRODUCT`: element-wise multiply across arrays, then sum.
//!
//! Quirks implemented here (no golden-reading):
//! - Non-numeric entries (text, empty, **uncoerced** logicals) are 0.
//! - Arithmetic / `--` on arrays coerces TRUE→1, FALSE→0 (classic criteria form).
//! - Arguments must share dimensions; otherwise `#VALUE!`.
//! - A single array is summed (implicit factor of 1).
//! - Arguments are evaluated in **array context** so
//!   `SUMPRODUCT((A1:A3>1)*B1:B3)` and `SUMPRODUCT(--(A1:A3="x"), B1:B3)` work.

use super::{coerce, compare, concat, div, excel_pow, Ctx, Evaluator};
use crate::ast::{BinOp, Expr, UnaryOp};
use xlsx_types::{CellRef, EvalError, ExcelError, ExcelValue, RangeRef};

pub(crate) fn eval(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.is_empty() {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }
    match try_fast_ranges(ev, args, ctx) {
        FastRanges::Hit(n) => return Ok(ExcelValue::Number(n)),
        FastRanges::Excel(e) => return Ok(ExcelValue::Error(e)),
        FastRanges::Miss => {}
    }
    let mut grids = Vec::with_capacity(args.len());
    for arg in args {
        let v = ev.eval_array_ctx(arg, ctx)?;
        if let ExcelValue::Error(e) = v {
            return Ok(ExcelValue::Error(e));
        }
        grids.push(v);
    }
    Ok(product_sum(&grids))
}

enum FastRanges {
    Hit(f64),
    Excel(ExcelError),
    Miss,
}

/// Packed multiply-sum used by the evaluator and by benches.
pub fn product_sum(arrays: &[ExcelValue]) -> ExcelValue {
    if arrays.is_empty() {
        return ExcelValue::Error(ExcelError::Value);
    }
    let mut packed: Vec<Vec<f64>> = Vec::with_capacity(arrays.len());
    let mut dims: Option<(usize, usize)> = None;
    for a in arrays {
        match pack_array(a) {
            Ok((rows, cols, nums)) => {
                if let Some(d) = dims {
                    if d != (rows, cols) {
                        return ExcelValue::Error(ExcelError::Value);
                    }
                } else {
                    dims = Some((rows, cols));
                }
                packed.push(nums);
            }
            Err(e) => return ExcelValue::Error(e),
        }
    }
    ExcelValue::Number(product_sum_packed(&packed))
}

/// Reference walk used only to measure the packed-kernel delta.
pub fn product_sum_naive(arrays: &[ExcelValue]) -> ExcelValue {
    if arrays.is_empty() {
        return ExcelValue::Error(ExcelError::Value);
    }
    let mut grids: Vec<(usize, usize, Vec<Vec<ExcelValue>>)> = Vec::with_capacity(arrays.len());
    for a in arrays {
        let (rows, cols, g) = match flatten_grid(a) {
            Ok(t) => t,
            Err(e) => return ExcelValue::Error(e),
        };
        if let Some((r0, c0, _)) = grids.first() {
            if *r0 != rows || *c0 != cols {
                return ExcelValue::Error(ExcelError::Value);
            }
        }
        grids.push((rows, cols, g));
    }
    let (rows, cols, _) = &grids[0];
    let mut acc = 0.0;
    for r in 0..*rows {
        for c in 0..*cols {
            let mut prod = 1.0;
            for (_, _, g) in &grids {
                match sumproduct_number(&g[r][c]) {
                    Ok(n) => prod *= n,
                    Err(e) => return ExcelValue::Error(e),
                }
            }
            acc += prod;
        }
    }
    ExcelValue::Number(acc)
}

pub fn product_sum_packed(arrays: &[Vec<f64>]) -> f64 {
    match arrays {
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
    }
}

/// SUMPRODUCT's own numeric coercion: non-numerics (incl. logicals) are 0.
pub fn sumproduct_number(v: &ExcelValue) -> Result<f64, ExcelError> {
    match v {
        ExcelValue::Number(n) => Ok(*n),
        ExcelValue::Empty | ExcelValue::Text(_) | ExcelValue::Bool(_) => Ok(0.0),
        ExcelValue::Error(e) => Err(*e),
        ExcelValue::Array(_) => Ok(0.0),
    }
}

fn pack_array(v: &ExcelValue) -> Result<(usize, usize, Vec<f64>), ExcelError> {
    match v {
        ExcelValue::Array(rows) => {
            if rows.is_empty() {
                return Ok((0, 0, Vec::new()));
            }
            let cols = rows[0].len();
            let mut out = Vec::with_capacity(rows.len() * cols);
            for row in rows {
                if row.len() != cols {
                    return Err(ExcelError::Value);
                }
                for c in row {
                    out.push(sumproduct_number(c)?);
                }
            }
            Ok((rows.len(), cols, out))
        }
        other => Ok((1, 1, vec![sumproduct_number(other)?])),
    }
}

fn flatten_grid(v: &ExcelValue) -> Result<(usize, usize, Vec<Vec<ExcelValue>>), ExcelError> {
    match v {
        ExcelValue::Array(rows) => {
            if rows.is_empty() {
                return Ok((0, 0, Vec::new()));
            }
            let cols = rows[0].len();
            if rows.iter().any(|r| r.len() != cols) {
                return Err(ExcelError::Value);
            }
            Ok((rows.len(), cols, rows.clone()))
        }
        other => Ok((1, 1, vec![vec![other.clone()]])),
    }
}

fn try_fast_ranges(ev: &Evaluator, args: &[Expr], ctx: &mut Ctx<'_>) -> FastRanges {
    let mut ranges: Vec<&RangeRef> = Vec::with_capacity(args.len());
    for arg in args {
        match arg {
            Expr::Range(r) => ranges.push(r),
            _ => return FastRanges::Miss,
        }
    }
    let rows = ranges[0].row_count();
    let cols = ranges[0].col_count();
    for r in &ranges[1..] {
        if r.row_count() != rows || r.col_count() != cols {
            return FastRanges::Excel(ExcelError::Value);
        }
    }
    let mut packed = Vec::with_capacity(ranges.len());
    for r in ranges {
        match pack_range(ev, r, ctx) {
            Ok(v) => packed.push(v),
            Err(FastPack::Excel(e)) => return FastRanges::Excel(e),
            Err(FastPack::Eval(e)) => {
                // Infrastructure failure — fall back to the general path so
                // circular / workbook errors still surface as Excel values.
                let _ = e;
                return FastRanges::Miss;
            }
        }
    }
    FastRanges::Hit(product_sum_packed(&packed))
}

enum FastPack {
    Excel(ExcelError),
    Eval(EvalError),
}

fn pack_range(ev: &Evaluator, range: &RangeRef, ctx: &mut Ctx<'_>) -> Result<Vec<f64>, FastPack> {
    let sheet_name = range
        .sheet
        .clone()
        .unwrap_or_else(|| ctx.current_sheet.clone());
    let n = (range.row_count() as usize).saturating_mul(range.col_count() as usize);
    let mut out = Vec::with_capacity(n);
    let mut a1 = [0u8; 16];
    for addr in range.cells() {
        let key = format_a1(addr, &mut a1);
        let packed = {
            let sheet = match ctx.spec.workbook.sheet(Some(&sheet_name)) {
                Ok(s) => s,
                Err(_) => return Err(FastPack::Excel(ExcelError::Ref)),
            };
            match sheet.cells.get(key) {
                Some(c) if c.formula.is_some() => None,
                Some(c) => Some(
                    sumproduct_number(c.value.as_ref().unwrap_or(&ExcelValue::Empty))
                        .map_err(FastPack::Excel)?,
                ),
                None => Some(0.0),
            }
        };
        match packed {
            Some(n) => out.push(n),
            None => {
                let v = ev
                    .eval_cell(
                        &CellRef {
                            sheet: Some(sheet_name.clone()),
                            addr,
                        },
                        ctx,
                    )
                    .map_err(FastPack::Eval)?;
                out.push(sumproduct_number(&v).map_err(FastPack::Excel)?);
            }
        }
    }
    Ok(out)
}

fn format_a1(addr: xlsx_types::CellAddr, buf: &mut [u8; 16]) -> &str {
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

impl Evaluator {
    /// Evaluate `expr` without implicit-intersecting ranges (SUMPRODUCT args).
    pub(crate) fn eval_array_ctx(
        &self,
        expr: &Expr,
        ctx: &mut Ctx<'_>,
    ) -> Result<ExcelValue, EvalError> {
        match expr {
            Expr::Range(r) => self.eval_range(r, ctx),
            Expr::Unary { op, expr } => {
                let v = self.eval_array_ctx(expr, ctx)?;
                Ok(map_unary(*op, v))
            }
            Expr::Binary { op, left, right } => {
                if *op == BinOp::Intersect {
                    return self.eval_intersect(left, right, ctx);
                }
                let l = self.eval_array_ctx(left, ctx)?;
                let r = self.eval_array_ctx(right, ctx)?;
                Ok(zip_binary(*op, l, r))
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
}

fn map_unary(op: UnaryOp, v: ExcelValue) -> ExcelValue {
    match v {
        ExcelValue::Array(rows) => ExcelValue::Array(
            rows.into_iter()
                .map(|row| row.into_iter().map(|c| apply_unary(op, c)).collect())
                .collect(),
        ),
        other => apply_unary(op, other),
    }
}

fn apply_unary(op: UnaryOp, v: ExcelValue) -> ExcelValue {
    if let ExcelValue::Error(e) = v {
        return ExcelValue::Error(e);
    }
    match coerce::to_number(&v) {
        Ok(n) => ExcelValue::Number(match op {
            UnaryOp::Plus => n,
            UnaryOp::Minus => -n,
            UnaryOp::Percent => n / 100.0,
        }),
        Err(e) => ExcelValue::Error(e),
    }
}

fn zip_binary(op: BinOp, l: ExcelValue, r: ExcelValue) -> ExcelValue {
    if let ExcelValue::Error(e) = l {
        return ExcelValue::Error(e);
    }
    if let ExcelValue::Error(e) = r {
        return ExcelValue::Error(e);
    }
    let (lr, lc) = shape(&l);
    let (rr, rc) = shape(&r);
    let l_scalar = is_scalar_shape(lr, lc, &l);
    let r_scalar = is_scalar_shape(rr, rc, &r);
    if l_scalar && r_scalar {
        return apply_bin(op, &l, &r);
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
            let lv = get_cell(&l, i, j, l_scalar);
            let rv = get_cell(&r, i, j, r_scalar);
            row.push(apply_bin(op, lv, rv));
        }
        out.push(row);
    }
    ExcelValue::Array(out)
}

fn is_scalar_shape(rows: usize, cols: usize, v: &ExcelValue) -> bool {
    !matches!(v, ExcelValue::Array(_)) || (rows == 1 && cols == 1)
}

fn shape(v: &ExcelValue) -> (usize, usize) {
    match v {
        ExcelValue::Array(rows) if rows.is_empty() => (0, 0),
        ExcelValue::Array(rows) => (rows.len(), rows[0].len()),
        _ => (1, 1),
    }
}

fn get_cell(v: &ExcelValue, r: usize, c: usize, as_scalar: bool) -> &ExcelValue {
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

fn apply_bin(op: BinOp, l: &ExcelValue, r: &ExcelValue) -> ExcelValue {
    if let ExcelValue::Error(e) = l {
        return ExcelValue::Error(*e);
    }
    if let ExcelValue::Error(e) = r {
        return ExcelValue::Error(*e);
    }
    match op {
        BinOp::Add => arith(l, r, |a, b| a + b),
        BinOp::Sub => arith(l, r, |a, b| a - b),
        BinOp::Mul => arith(l, r, |a, b| a * b),
        BinOp::Div => div(l, r),
        BinOp::Pow => excel_pow(l, r),
        BinOp::Concat => concat(l, r),
        BinOp::Eq => ExcelValue::Bool(compare::equal(l, r)),
        BinOp::Ne => ExcelValue::Bool(!compare::equal(l, r)),
        BinOp::Lt => ExcelValue::Bool(compare::ordered(l, r, std::cmp::Ordering::Less, false)),
        BinOp::Gt => ExcelValue::Bool(compare::ordered(l, r, std::cmp::Ordering::Greater, false)),
        BinOp::Le => ExcelValue::Bool(compare::ordered(l, r, std::cmp::Ordering::Greater, true)),
        BinOp::Ge => ExcelValue::Bool(compare::ordered(l, r, std::cmp::Ordering::Less, true)),
        BinOp::Intersect => ExcelValue::Error(ExcelError::Value),
    }
}

fn arith(l: &ExcelValue, r: &ExcelValue, f: impl Fn(f64, f64) -> f64) -> ExcelValue {
    match (coerce::to_number(l), coerce::to_number(r)) {
        (Ok(a), Ok(b)) => ExcelValue::Number(f(a, b)),
        (Err(e), _) | (_, Err(e)) => ExcelValue::Error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num_row(vals: &[f64]) -> ExcelValue {
        ExcelValue::Array(vec![vals.iter().copied().map(ExcelValue::Number).collect()])
    }

    #[test]
    fn two_arrays_product_sum() {
        let a = num_row(&[1.0, 2.0, 3.0]);
        let b = num_row(&[4.0, 5.0, 6.0]);
        assert_eq!(product_sum(&[a, b]), ExcelValue::Number(32.0));
    }

    #[test]
    fn packed_matches_naive() {
        let a = ExcelValue::Array(vec![
            vec![ExcelValue::Number(1.0), ExcelValue::Text("x".into())],
            vec![ExcelValue::Bool(true), ExcelValue::Empty],
        ]);
        let b = ExcelValue::Array(vec![
            vec![ExcelValue::Number(10.0), ExcelValue::Number(20.0)],
            vec![ExcelValue::Number(30.0), ExcelValue::Number(40.0)],
        ]);
        let packed = product_sum(&[a.clone(), b.clone()]);
        let naive = product_sum_naive(&[a, b]);
        assert_eq!(packed, naive);
        assert_eq!(packed, ExcelValue::Number(10.0));
    }

    #[test]
    fn mismatch_is_value() {
        let a = num_row(&[1.0, 2.0]);
        let b = num_row(&[1.0, 2.0, 3.0]);
        assert_eq!(product_sum(&[a, b]), ExcelValue::Error(ExcelError::Value));
    }
}
