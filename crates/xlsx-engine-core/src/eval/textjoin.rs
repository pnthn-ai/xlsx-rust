//! Excel `TEXTJOIN(delimiter, ignore_empty, text1, [text2], …)`.
//!
//! Semantics (Excel 2019 / Microsoft 365):
//! - `delimiter` may be a scalar or a flattened range/array that **cycles**
//!   between emitted pieces (`TEXTJOIN({",",";"}, TRUE, "a","b","c")` → `a,b;c`).
//! - `ignore_empty` is a logical. TRUE skips blank cells and `""`; FALSE keeps
//!   them (consecutive delimiters). A space is not empty.
//! - Text arguments stay in array context (no implicit intersection): a range
//!   is flattened row-major (left-to-right, then top-to-bottom).
//! - Numbers / bools coerce like `&` (`TRUE` → `"TRUE"`, `12` → `"12"`).
//! - First error wins, left-to-right (delimiter, then ignore_empty, then texts).
//! - Result longer than 32,767 UTF-16 code units is `#VALUE!`.

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{CellAddr, CellRef, EvalError, ExcelError, ExcelValue, RangeRef, Sheet};

/// Excel cell-content limit used by TEXTJOIN.
pub const TEXTJOIN_MAX_CHARS: usize = 32767;

/// Production TEXTJOIN (range walk, stream append, no 2-D materialize).
pub(crate) fn fn_textjoin(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    textjoin_eval(ev, args, ctx, false)
}

/// Materializing baseline kept as the Instant-bench “before”.
pub(crate) fn textjoin_materialized(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    textjoin_eval(ev, args, ctx, true)
}

fn textjoin_eval(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
    materialize: bool,
) -> Result<ExcelValue, EvalError> {
    if args.len() < 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }

    let delim_v = ev.eval_expr(&args[0], ctx)?;
    if let ExcelValue::Error(e) = delim_v {
        return Ok(ExcelValue::Error(e));
    }
    let delims = match collect_delims(&delim_v) {
        Ok(d) => d,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };

    let ie = ev.eval_scalar(&args[1], ctx)?;
    let ignore_empty = match coerce::to_logical(&ie) {
        Ok(b) => b,
        Err(e) => return Ok(ExcelValue::Error(e)),
    };

    let mut builder = TextJoinBuilder::new(delims);
    if let Some(hint) = reserve_hint(&args[2..]) {
        builder.reserve(hint);
    }
    for arg in &args[2..] {
        let r = if materialize {
            feed_value(&mut builder, &ev.eval_expr(arg, ctx)?, ignore_empty)
        } else {
            feed_arg(ev, ctx, arg, &mut builder, ignore_empty)
        };
        if let Err(e) = r {
            return Ok(ExcelValue::Error(e));
        }
    }
    Ok(ExcelValue::Text(builder.finish()))
}

/// Evaluate `formula` against `workbook` using the production TEXTJOIN path
/// when the formula is a TEXTJOIN call (used by the hill-climb bench).
pub fn eval_textjoin_formula(
    workbook: &xlsx_types::Workbook,
    formula: &str,
    materialized: bool,
) -> Result<ExcelValue, EvalError> {
    use xlsx_types::{EvalSpec, EvalTarget};
    let spec = EvalSpec {
        case_id: "textjoin-bench".into(),
        workbook: workbook.clone(),
        target: EvalTarget::formula(formula),
        options: Default::default(),
    };
    if !materialized {
        return Evaluator::new().eval_spec(&spec);
    }
    let ast = crate::parse::parse(formula)?;
    match ast {
        Expr::Call { name, args } if name.eq_ignore_ascii_case("TEXTJOIN") => {
            let current_sheet = spec.workbook.default_sheet_name().to_string();
            let mut ctx = Ctx {
                spec: &spec,
                current_sheet,
                depth: 0,
                visiting: Default::default(),
                host: spec.default_cell().addr,
                rng: super::randarray::XorShift64::from_eval_options(&spec.options),
                locals: Vec::new(),
            };
            textjoin_materialized(&Evaluator::new(), &args, &mut ctx)
        }
        _ => Evaluator::new().eval_spec(&spec),
    }
}

fn feed_arg(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    arg: &Expr,
    builder: &mut TextJoinBuilder,
    ignore_empty: bool,
) -> Result<(), ExcelError> {
    match arg {
        Expr::Range(r) => feed_range(ev, ctx, r, builder, ignore_empty),
        Expr::Cell(c) => {
            let v = ev.eval_cell(c, ctx).map_err(|_| ExcelError::Value)?;
            feed_scalar(builder, &v, ignore_empty)
        }
        Expr::Name(n) => match named_as_range(n, ctx) {
            Ok(r) => feed_range(ev, ctx, &r, builder, ignore_empty),
            Err(ExcelError::Name) => Err(ExcelError::Name),
            Err(_) => {
                let v = ev.eval_expr(arg, ctx).map_err(|_| ExcelError::Value)?;
                feed_value(builder, &v, ignore_empty)
            }
        },
        other => {
            let v = ev.eval_expr(other, ctx).map_err(|_| ExcelError::Value)?;
            feed_value(builder, &v, ignore_empty)
        }
    }
}

fn feed_range(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    range: &RangeRef,
    builder: &mut TextJoinBuilder,
    ignore_empty: bool,
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
        let v = read_cell(ev, ctx, &sheet_name, addr, &mut a1)?;
        feed_scalar(builder, &v, ignore_empty)?;
    }
    Ok(())
}

fn read_cell(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    sheet_name: &str,
    addr: CellAddr,
    a1_buf: &mut String,
) -> Result<ExcelValue, ExcelError> {
    match stored_value(ctx, sheet_name, addr, a1_buf) {
        Stored::Value(v) => Ok(v),
        Stored::NeedsEval => ev
            .eval_cell(
                &CellRef {
                    sheet: Some(sheet_name.to_string()),
                    addr,
                },
                ctx,
            )
            .map_err(|_| ExcelError::Value),
        Stored::MissingSheet => Err(ExcelError::Ref),
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
    // Avoid `addr.a1()` allocation for the common A1..Z9999 band.
    write_a1(addr, a1_buf);
    match sheet.cells.get(a1_buf.as_str()) {
        Some(c) if c.formula.is_some() => Stored::NeedsEval,
        Some(c) => Stored::Value(c.value.clone().unwrap_or(ExcelValue::Empty)),
        None => Stored::Value(ExcelValue::Empty),
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

fn collect_delims(v: &ExcelValue) -> Result<Vec<String>, ExcelError> {
    let mut out = Vec::new();
    flatten_to_strings(v, &mut out)?;
    if out.is_empty() {
        out.push(String::new());
    }
    Ok(out)
}

fn flatten_to_strings(v: &ExcelValue, out: &mut Vec<String>) -> Result<(), ExcelError> {
    match v {
        ExcelValue::Array(rows) => {
            for row in rows {
                for c in row {
                    flatten_to_strings(c, out)?;
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

fn feed_value(
    builder: &mut TextJoinBuilder,
    v: &ExcelValue,
    ignore_empty: bool,
) -> Result<(), ExcelError> {
    match v {
        ExcelValue::Array(rows) => {
            for row in rows {
                for c in row {
                    feed_value(builder, c, ignore_empty)?;
                }
            }
            Ok(())
        }
        other => feed_scalar(builder, other, ignore_empty),
    }
}

fn feed_scalar(
    builder: &mut TextJoinBuilder,
    v: &ExcelValue,
    ignore_empty: bool,
) -> Result<(), ExcelError> {
    match v {
        ExcelValue::Error(e) => Err(*e),
        ExcelValue::Empty => builder.push("", ignore_empty),
        ExcelValue::Text(s) => builder.push(s, ignore_empty),
        ExcelValue::Bool(true) => builder.push("TRUE", ignore_empty),
        ExcelValue::Bool(false) => builder.push("FALSE", ignore_empty),
        ExcelValue::Number(n) => {
            let s = coerce::format_plain(*n);
            builder.push(&s, ignore_empty)
        }
        ExcelValue::Array(_) => feed_value(builder, v, ignore_empty),
    }
}

/// Streaming join with cycling delimiters and the 32,767-character cap.
pub struct TextJoinBuilder {
    delims: Vec<String>,
    out: String,
    utf16: usize,
    emitted: usize,
}

impl TextJoinBuilder {
    pub fn new(delims: Vec<String>) -> Self {
        let delims = if delims.is_empty() {
            vec![String::new()]
        } else {
            delims
        };
        Self {
            delims,
            out: String::new(),
            utf16: 0,
            emitted: 0,
        }
    }

    pub fn reserve(&mut self, additional: usize) {
        self.out.reserve(additional.min(TEXTJOIN_MAX_CHARS * 3));
    }

    pub fn push(&mut self, s: &str, ignore_empty: bool) -> Result<(), ExcelError> {
        if ignore_empty && s.is_empty() {
            return Ok(());
        }
        if self.emitted > 0 {
            self.push_delim()?;
        }
        self.push_raw(s)?;
        self.emitted += 1;
        Ok(())
    }

    fn push_delim(&mut self) -> Result<(), ExcelError> {
        let idx = (self.emitted - 1) % self.delims.len();
        let add = utf16_len(&self.delims[idx]);
        if self.utf16.saturating_add(add) > TEXTJOIN_MAX_CHARS {
            return Err(ExcelError::Value);
        }
        self.utf16 += add;
        self.out.push_str(&self.delims[idx]);
        Ok(())
    }

    fn push_raw(&mut self, s: &str) -> Result<(), ExcelError> {
        let add = utf16_len(s);
        if self.utf16.saturating_add(add) > TEXTJOIN_MAX_CHARS {
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

/// Naive collect-then-`join` used only as a microbench baseline for the
/// single-delimiter kernel (not the full evaluator).
pub fn textjoin_naive_join(
    delim: &str,
    parts: &[&str],
    ignore_empty: bool,
) -> Result<String, ExcelError> {
    let filtered: Vec<&str> = if ignore_empty {
        parts.iter().copied().filter(|s| !s.is_empty()).collect()
    } else {
        parts.to_vec()
    };
    let out = filtered.join(delim);
    if utf16_len(&out) > TEXTJOIN_MAX_CHARS {
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
    fn ignore_empty_and_cycle() {
        let mut b = TextJoinBuilder::new(vec![",".into(), ";".into()]);
        b.push("a", true).unwrap();
        b.push("", true).unwrap();
        b.push("b", true).unwrap();
        b.push("c", true).unwrap();
        assert_eq!(b.finish(), "a,b;c");
    }

    #[test]
    fn keep_empty_inserts_delimiter() {
        let mut b = TextJoinBuilder::new(vec![",".into()]);
        b.push("a", false).unwrap();
        b.push("", false).unwrap();
        b.push("b", false).unwrap();
        assert_eq!(b.finish(), "a,,b");
    }

    #[test]
    fn over_limit_is_value() {
        let mut b = TextJoinBuilder::new(vec![String::new()]);
        let chunk = "x".repeat(32767);
        b.push(&chunk, true).unwrap();
        assert!(b.push("y", true).is_err());
    }

    #[test]
    fn formula_matches_builder() {
        let wb = wb_col_a(&["a", "", "b"]);
        let v = eval_textjoin_formula(&wb, "=TEXTJOIN(\",\",TRUE,A1:A3)", false).unwrap();
        assert_eq!(v, ExcelValue::Text("a,b".into()));
        let v = eval_textjoin_formula(&wb, "=TEXTJOIN(\",\",FALSE,A1:A3)", false).unwrap();
        assert_eq!(v, ExcelValue::Text("a,,b".into()));
        let m = eval_textjoin_formula(&wb, "=TEXTJOIN(\",\",TRUE,A1:A3)", true).unwrap();
        assert_eq!(m, ExcelValue::Text("a,b".into()));
    }
}
