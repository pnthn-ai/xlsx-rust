//! Excel `CONCAT(text1, [text2], …)`.
//!
//! Semantics (Excel 2019 / Microsoft 365):
//! - Every argument is taken in **array** context (no implicit intersection).
//!   Ranges and array literals flatten row-major (left-to-right, then
//!   top-to-bottom). Argument order is preserved.
//! - Empty cells and `""` contribute nothing (there is no delimiter).
//! - Numbers / bools coerce like `&` (`TRUE` → `"TRUE"`, `12` → `"12"`).
//! - First error wins, left-to-right (including the first error inside a
//!   flattened range).
//! - Zero arguments is `#VALUE!`.
//! - Result longer than 32,767 UTF-16 code units is `#VALUE!`.
//!
//! `CONCATENATE` and `TEXTJOIN` are out of scope.

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{CellAddr, CellRef, EvalError, ExcelError, ExcelValue, RangeRef};

/// Excel cell-content limit used by CONCAT.
pub const CONCAT_MAX_CHARS: usize = 32767;

/// Production CONCAT (range walk, stream append, no 2-D materialize).
pub(crate) fn fn_concat(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    concat_eval(ev, args, ctx, false)
}

/// Materializing baseline kept as the Instant-bench “before”.
pub(crate) fn concat_materialized(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    concat_eval(ev, args, ctx, true)
}

fn concat_eval(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
    materialize: bool,
) -> Result<ExcelValue, EvalError> {
    if args.is_empty() {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }

    let mut builder = ConcatBuilder::new();
    if let Some(hint) = reserve_hint(args) {
        builder.reserve(hint);
    }
    for arg in args {
        let r = if materialize {
            feed_value(&mut builder, &ev.eval_expr(arg, ctx)?)
        } else {
            feed_arg(ev, ctx, arg, &mut builder)
        };
        if let Err(e) = r {
            return Ok(ExcelValue::Error(e));
        }
    }
    Ok(ExcelValue::Text(builder.finish()))
}

/// Evaluate `formula` against `workbook` using the production CONCAT path
/// when the formula is a CONCAT call (used by the hill-climb bench).
pub fn eval_concat_formula(
    workbook: &xlsx_types::Workbook,
    formula: &str,
    materialized: bool,
) -> Result<ExcelValue, EvalError> {
    use xlsx_types::{EvalSpec, EvalTarget};
    let spec = EvalSpec {
        case_id: "concat-bench".into(),
        workbook: workbook.clone(),
        target: EvalTarget::formula(formula),
        options: Default::default(),
    };
    if !materialized {
        return Evaluator::new().eval_spec(&spec);
    }
    let ast = crate::parse::parse(formula)?;
    match ast {
        Expr::Call { name, args } if name.eq_ignore_ascii_case("CONCAT") => {
            let current_sheet = spec.workbook.default_sheet_name().to_string();
            let mut ctx = Ctx {
                spec: &spec,
                current_sheet,
                depth: 0,
                visiting: Default::default(),
                host: spec.default_cell().addr,
                rng: super::randarray::XorShift64::from_eval_options(&spec.options),
            };
            concat_materialized(&Evaluator::new(), &args, &mut ctx)
        }
        _ => Evaluator::new().eval_spec(&spec),
    }
}

fn feed_arg(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    arg: &Expr,
    builder: &mut ConcatBuilder,
) -> Result<(), ExcelError> {
    match arg {
        Expr::Range(r) => feed_range(ev, ctx, r, builder),
        Expr::Cell(c) => {
            let v = ev.eval_cell(c, ctx).map_err(|_| ExcelError::Value)?;
            feed_scalar(builder, &v)
        }
        Expr::Name(n) => match named_as_range(n, ctx) {
            Ok(r) => feed_range(ev, ctx, &r, builder),
            Err(ExcelError::Name) => Err(ExcelError::Name),
            Err(_) => {
                let v = ev.eval_expr(arg, ctx).map_err(|_| ExcelError::Value)?;
                feed_value(builder, &v)
            }
        },
        other => {
            let v = ev.eval_expr(other, ctx).map_err(|_| ExcelError::Value)?;
            feed_value(builder, &v)
        }
    }
}

fn feed_range(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    range: &RangeRef,
    builder: &mut ConcatBuilder,
) -> Result<(), ExcelError> {
    let sheet_name = range
        .sheet
        .clone()
        .unwrap_or_else(|| ctx.current_sheet.clone());
    if ctx.spec.workbook.sheet(Some(&sheet_name)).is_err() {
        return Err(ExcelError::Ref);
    }
    let mut a1 = String::with_capacity(8);
    builder.reserve((range.row_count() as usize).saturating_mul(range.col_count() as usize) * 2);
    for addr in range.cells() {
        feed_cell(ev, ctx, &sheet_name, addr, &mut a1, builder)?;
    }
    Ok(())
}

fn feed_cell(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    sheet_name: &str,
    addr: CellAddr,
    a1_buf: &mut String,
    builder: &mut ConcatBuilder,
) -> Result<(), ExcelError> {
    match peek_stored(ctx, sheet_name, addr, a1_buf, builder)? {
        Peek::Done => Ok(()),
        Peek::NeedsEval => {
            let v = ev
                .eval_cell(
                    &CellRef {
                        sheet: Some(sheet_name.to_string()),
                        addr,
                    },
                    ctx,
                )
                .map_err(|_| ExcelError::Value)?;
            feed_scalar(builder, &v)
        }
    }
}

enum Peek {
    Done,
    NeedsEval,
}

/// Read a stored cell by ref (no `ExcelValue` clone for text / empty) and
/// stream-append. Formula cells fall through to `eval_cell`.
fn peek_stored(
    ctx: &Ctx<'_>,
    sheet_name: &str,
    addr: CellAddr,
    a1_buf: &mut String,
    builder: &mut ConcatBuilder,
) -> Result<Peek, ExcelError> {
    let sheet = match ctx.spec.workbook.sheet(Some(sheet_name)) {
        Ok(s) => s,
        Err(_) => return Err(ExcelError::Ref),
    };
    a1_buf.clear();
    write_a1(addr, a1_buf);
    match sheet.cells.get(a1_buf.as_str()) {
        Some(c) if c.formula.is_some() => Ok(Peek::NeedsEval),
        Some(c) => {
            feed_stored_value(builder, c.value.as_ref())?;
            Ok(Peek::Done)
        }
        None => Ok(Peek::Done),
    }
}

fn feed_stored_value(
    builder: &mut ConcatBuilder,
    value: Option<&ExcelValue>,
) -> Result<(), ExcelError> {
    match value {
        None | Some(ExcelValue::Empty) => Ok(()),
        Some(ExcelValue::Text(s)) => builder.push(s),
        Some(ExcelValue::Bool(true)) => builder.push("TRUE"),
        Some(ExcelValue::Bool(false)) => builder.push("FALSE"),
        Some(ExcelValue::Number(n)) => {
            let s = coerce::format_plain(*n);
            builder.push(&s)
        }
        Some(ExcelValue::Error(e)) => Err(*e),
        Some(ExcelValue::Array(rows)) => {
            for row in rows {
                for c in row {
                    feed_value(builder, c)?;
                }
            }
            Ok(())
        }
    }
}

fn write_a1(addr: CellAddr, out: &mut String) {
    // Column: 0 → A, 25 → Z, 26 → AA.
    let mut col = addr.col + 1;
    let mut buf = [0u8; 4];
    let mut n = 0usize;
    while col > 0 {
        col -= 1;
        buf[n] = b'A' + (col % 26) as u8;
        n += 1;
        col /= 26;
    }
    for i in (0..n).rev() {
        out.push(buf[i] as char);
    }
    let mut row = addr.row + 1;
    let mut digits = [0u8; 10];
    let mut d = 0usize;
    while row > 0 {
        digits[d] = b'0' + (row % 10) as u8;
        d += 1;
        row /= 10;
    }
    for i in (0..d).rev() {
        out.push(digits[i] as char);
    }
}

fn reserve_hint(args: &[Expr]) -> Option<usize> {
    let mut n = 0usize;
    for arg in args {
        match arg {
            Expr::Range(r) => {
                n = n.saturating_add(
                    (r.row_count() as usize).saturating_mul(r.col_count() as usize) * 2,
                );
            }
            Expr::Cell(_) | Expr::Text(_) => n = n.saturating_add(8),
            _ => {}
        }
    }
    (n > 0).then_some(n)
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

fn feed_value(builder: &mut ConcatBuilder, v: &ExcelValue) -> Result<(), ExcelError> {
    match v {
        ExcelValue::Array(rows) => {
            for row in rows {
                for c in row {
                    feed_value(builder, c)?;
                }
            }
            Ok(())
        }
        other => feed_scalar(builder, other),
    }
}

fn feed_scalar(builder: &mut ConcatBuilder, v: &ExcelValue) -> Result<(), ExcelError> {
    match v {
        ExcelValue::Error(e) => Err(*e),
        ExcelValue::Empty => Ok(()),
        ExcelValue::Text(s) => builder.push(s),
        ExcelValue::Bool(true) => builder.push("TRUE"),
        ExcelValue::Bool(false) => builder.push("FALSE"),
        ExcelValue::Number(n) => {
            let s = coerce::format_plain(*n);
            builder.push(&s)
        }
        ExcelValue::Array(_) => feed_value(builder, v),
    }
}

/// Streaming concat with the 32,767 UTF-16-code-unit cap.
pub struct ConcatBuilder {
    out: String,
    utf16: usize,
}

impl ConcatBuilder {
    pub fn new() -> Self {
        Self {
            out: String::new(),
            utf16: 0,
        }
    }

    pub fn reserve(&mut self, additional: usize) {
        self.out.reserve(additional.min(CONCAT_MAX_CHARS * 3));
    }

    pub fn push(&mut self, s: &str) -> Result<(), ExcelError> {
        if s.is_empty() {
            return Ok(());
        }
        let add = utf16_len(s);
        if self.utf16.saturating_add(add) > CONCAT_MAX_CHARS {
            return Err(ExcelError::Value);
        }
        self.utf16 += add;
        self.out.push_str(s);
        Ok(())
    }

    pub fn finish(self) -> String {
        self.out
    }
}

fn utf16_len(s: &str) -> usize {
    if s.is_ascii() {
        s.len()
    } else {
        s.encode_utf16().count()
    }
}

/// Naive collect-then-`concat` used only as a microbench baseline for the
/// kernel (not the full evaluator).
pub fn concat_naive_join(parts: &[&str]) -> Result<String, ExcelError> {
    let out = parts.concat();
    if utf16_len(&out) > CONCAT_MAX_CHARS {
        return Err(ExcelError::Value);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use xlsx_types::{Cell, Sheet, Workbook};

    fn wb_col_a(texts: &[&str]) -> Workbook {
        let mut sheet = Sheet::new("Sheet1");
        for (i, t) in texts.iter().enumerate() {
            sheet.insert(
                CellAddr::new(0, i as u32),
                Cell::value(ExcelValue::Text((*t).into())),
            );
        }
        Workbook {
            sheets: vec![sheet],
            names: vec![],
        }
    }

    #[test]
    fn blanks_add_nothing() {
        let mut b = ConcatBuilder::new();
        b.push("a").unwrap();
        b.push("").unwrap();
        b.push("b").unwrap();
        assert_eq!(b.finish(), "ab");
    }

    #[test]
    fn over_limit_is_value() {
        let mut b = ConcatBuilder::new();
        let chunk = "x".repeat(32767);
        b.push(&chunk).unwrap();
        assert!(b.push("y").is_err());
    }

    #[test]
    fn utf16_emoji_counts_two() {
        let mut b = ConcatBuilder::new();
        b.push(&"x".repeat(32766)).unwrap();
        assert!(b.push("😀").is_err());
    }

    #[test]
    fn formula_matches_builder() {
        let wb = wb_col_a(&["a", "", "b"]);
        let v = eval_concat_formula(&wb, "=CONCAT(A1:A3)", false).unwrap();
        assert_eq!(v, ExcelValue::Text("ab".into()));
        let m = eval_concat_formula(&wb, "=CONCAT(A1:A3)", true).unwrap();
        assert_eq!(m, ExcelValue::Text("ab".into()));
    }

    #[test]
    fn row_major_and_arg_order() {
        let mut sheet = Sheet::new("Sheet1");
        sheet.insert(
            CellAddr::new(0, 0),
            Cell::value(ExcelValue::Text("a".into())),
        );
        sheet.insert(
            CellAddr::new(1, 0),
            Cell::value(ExcelValue::Text("b".into())),
        );
        sheet.insert(
            CellAddr::new(0, 1),
            Cell::value(ExcelValue::Text("c".into())),
        );
        sheet.insert(
            CellAddr::new(1, 1),
            Cell::value(ExcelValue::Text("d".into())),
        );
        let wb = Workbook {
            sheets: vec![sheet],
            names: vec![],
        };
        let v = eval_concat_formula(&wb, "=CONCAT(A1:B2)", false).unwrap();
        assert_eq!(v, ExcelValue::Text("abcd".into()));
        let v = eval_concat_formula(&wb, "=CONCAT(A1:A2,B1:B2)", false).unwrap();
        assert_eq!(v, ExcelValue::Text("acbd".into()));
    }
}
