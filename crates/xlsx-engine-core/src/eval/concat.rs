//! Excel `CONCAT(text1, [text2], …)`.
//!
//! Semantics (Excel 2019 / Microsoft 365):
//! - Every argument is taken in **array** context (no implicit intersection).
//!   Ranges and array literals flatten row-major (left-to-right, then
//!   top-to-bottom). Argument order is preserved.
//! - Empty cells and `""` contribute nothing (there is no delimiter).
//!   A space, tab, or other whitespace is kept — `CONCAT` is not `TRIM`.
//! - Numbers / bools coerce like `&` (`TRUE` → `"TRUE"`, `12` → `"12"`).
//! - First error wins, left-to-right (including the first error inside a
//!   flattened range).
//! - Zero arguments is `#VALUE!`.
//!
//! ## Length cap (honest)
//!
//! The result cannot exceed **32,767 UTF-16 code units** — Excel’s stored
//! cell-content width, the same cap as `REPT` / `TEXTJOIN`. Microsoft’s
//! CONCAT page says “32767 characters”; that is this cell-width limit, **not**
//! this crate’s Compatibility Version 2 `LEN` (Unicode scalars).
//!
//! - `CONCAT(REPT("a", 32767))` is allowed; one extra ASCII unit is `#VALUE!`.
//! - A supplementary-plane scalar (`😀`) is **two** UTF-16 units, so
//!   `CONCAT(REPT("😀", 16383), "😀")` is `#VALUE!` even though Compat-v2
//!   `LEN` of 16383 copies is 16383.
//! - Overflow is detected while streaming; the builder never holds more than
//!   the cap. Cap success cases in the corpus use `LEN(CONCAT(…))` or a
//!   `#VALUE!` overflow — not 32k expected strings.
//!
//! Production path: walk occupied cells when the rectangle is much larger
//! than the sheet store, stream-append with ASCII-fast UTF-16 length. The
//! materializing `eval_expr` → 2-D `Array` path is kept as the Instant-bench
//! “before”.
//!
//! `CONCATENATE` and `TEXTJOIN` are out of scope.

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{CellAddr, CellRef, EvalError, ExcelError, ExcelValue, RangeRef};

/// Excel cell-content limit used by CONCAT (UTF-16 code units).
pub const CONCAT_MAX_CHARS: usize = 32767;

/// Which range walk `eval_concat_formula` / the bench should take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConcatWalk {
    /// `eval_expr` → 2-D `Array` → flatten (bench “before”).
    Materialize,
    /// Visit every address in the rectangle (HashMap / BTree lookup).
    Dense,
    /// Production: dense walk, or occupied-cell gather on large sparse ranges.
    Auto,
}

/// Production CONCAT (range walk, stream append, no 2-D materialize).
pub(crate) fn fn_concat(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    concat_eval(ev, args, ctx, ConcatWalk::Auto)
}

fn concat_eval(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
    walk: ConcatWalk,
) -> Result<ExcelValue, EvalError> {
    if args.is_empty() {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }

    let mut builder = ConcatBuilder::new();
    if let Some(hint) = reserve_hint(args) {
        builder.reserve(hint);
    }
    for arg in args {
        let r = if walk == ConcatWalk::Materialize {
            concat_feed_value(&mut builder, &ev.eval_expr(arg, ctx)?)
        } else {
            feed_arg(ev, ctx, arg, &mut builder, walk)
        };
        if let Err(e) = r {
            return Ok(ExcelValue::Error(e));
        }
    }
    Ok(ExcelValue::Text(builder.finish()))
}

/// Evaluate `formula` against `workbook` using a CONCAT walk
/// (used by the hill-climb bench).
pub fn eval_concat_formula(
    workbook: &xlsx_types::Workbook,
    formula: &str,
    walk: ConcatWalk,
) -> Result<ExcelValue, EvalError> {
    use xlsx_types::{EvalSpec, EvalTarget};
    let spec = EvalSpec {
        case_id: "concat-bench".into(),
        workbook: workbook.clone(),
        target: EvalTarget::formula(formula),
        options: Default::default(),
    };
    if walk == ConcatWalk::Auto {
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
                locals: Vec::new(),
            };
            concat_eval(&Evaluator::new(), &args, &mut ctx, walk)
        }
        _ => Evaluator::new().eval_spec(&spec),
    }
}

fn feed_arg(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    arg: &Expr,
    builder: &mut ConcatBuilder,
    walk: ConcatWalk,
) -> Result<(), ExcelError> {
    match arg {
        Expr::Range(r) => feed_range(ev, ctx, r, builder, walk),
        Expr::Cell(c) => {
            let v = ev.eval_cell(c, ctx).map_err(|_| ExcelError::Value)?;
            feed_scalar(builder, &v)
        }
        Expr::Name(n) => match named_as_range(n, ctx) {
            Ok(r) => feed_range(ev, ctx, &r, builder, walk),
            Err(ExcelError::Name) => Err(ExcelError::Name),
            Err(_) => {
                let v = ev.eval_expr(arg, ctx).map_err(|_| ExcelError::Value)?;
                concat_feed_value(builder, &v)
            }
        },
        other => {
            let v = ev.eval_expr(other, ctx).map_err(|_| ExcelError::Value)?;
            concat_feed_value(builder, &v)
        }
    }
}

fn feed_range(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    range: &RangeRef,
    builder: &mut ConcatBuilder,
    walk: ConcatWalk,
) -> Result<(), ExcelError> {
    let sheet_name = range
        .sheet
        .clone()
        .unwrap_or_else(|| ctx.current_sheet.clone());
    let occupied = match ctx.spec.workbook.sheet(Some(&sheet_name)) {
        Ok(s) => s.cells.len() as u64,
        Err(_) => return Err(ExcelError::Ref),
    };
    if prefer_occupied(range, occupied, walk) {
        feed_range_occupied(ev, ctx, &sheet_name, range, builder)
    } else {
        feed_range_dense(ev, ctx, &sheet_name, range, builder)
    }
}

/// Occupied gather is O(n_sheet + n_hits log n_hits). Dense walk is
/// O(area × lookup). Use occupied when the rectangle is much larger than
/// the store — empty cells add nothing, so skipping missing keys is
/// equivalent. `Dense` forces the rectangle walk so the bench can show
/// the gather win on sparse columns.
fn prefer_occupied(range: &RangeRef, occupied: u64, walk: ConcatWalk) -> bool {
    if walk == ConcatWalk::Dense {
        return false;
    }
    let area = (range.row_count() as u64).saturating_mul(range.col_count() as u64);
    area > 32 && area > occupied.saturating_mul(2)
}

fn feed_range_dense(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    sheet_name: &str,
    range: &RangeRef,
    builder: &mut ConcatBuilder,
) -> Result<(), ExcelError> {
    let mut a1 = String::with_capacity(8);
    let area = (range.row_count() as usize).saturating_mul(range.col_count() as usize);
    builder.reserve(area.saturating_mul(2).min(CONCAT_MAX_CHARS));
    for addr in range.cells() {
        feed_cell(ev, ctx, sheet_name, addr, &mut a1, builder)?;
    }
    Ok(())
}

fn feed_range_occupied(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    sheet_name: &str,
    range: &RangeRef,
    builder: &mut ConcatBuilder,
) -> Result<(), ExcelError> {
    // Collect addresses only — do not hold `&Sheet` across `eval_cell`.
    let hits = gather_occupied(ctx, sheet_name, range)?;
    builder.reserve(hits.len().saturating_mul(4).min(CONCAT_MAX_CHARS));
    let mut a1 = String::with_capacity(8);
    for addr in hits {
        feed_cell(ev, ctx, sheet_name, addr, &mut a1, builder)?;
    }
    Ok(())
}

fn gather_occupied(
    ctx: &Ctx<'_>,
    sheet_name: &str,
    range: &RangeRef,
) -> Result<Vec<CellAddr>, ExcelError> {
    let sheet = match ctx.spec.workbook.sheet(Some(sheet_name)) {
        Ok(s) => s,
        Err(_) => return Err(ExcelError::Ref),
    };
    let mut hits = Vec::new();
    for key in sheet.cells.keys() {
        let Ok(addr) = CellAddr::parse(key) else {
            continue;
        };
        if in_range(range, addr) {
            hits.push(addr);
        }
    }
    // BTreeMap keys are A1 strings (`A1`, `A10`, `A2`, …) — not row-major.
    hits.sort_unstable_by(|a, b| a.row.cmp(&b.row).then(a.col.cmp(&b.col)));
    Ok(hits)
}

fn in_range(range: &RangeRef, addr: CellAddr) -> bool {
    addr.row >= range.start.row
        && addr.row <= range.end.row
        && addr.col >= range.start.col
        && addr.col <= range.end.col
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
    addr.write_a1(a1_buf);
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
                    concat_feed_value(builder, c)?;
                }
            }
            Ok(())
        }
    }
}

fn reserve_hint(args: &[Expr]) -> Option<usize> {
    let mut n = 0usize;
    for arg in args {
        match arg {
            Expr::Range(r) => {
                let area = (r.row_count() as usize).saturating_mul(r.col_count() as usize);
                // Do not pre-size to a giant empty rectangle; the cap is 32,767.
                n = n.saturating_add(area.min(CONCAT_MAX_CHARS));
            }
            Expr::Cell(_) | Expr::Text(_) => n = n.saturating_add(8),
            _ => {}
        }
    }
    (n > 0).then_some(n.min(CONCAT_MAX_CHARS))
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

/// Flatten an already-evaluated value into `builder` (arrays row-major).
/// Shared with `seed-compliant` so the UTF-16 cap stays one implementation.
pub fn concat_feed_value(builder: &mut ConcatBuilder, v: &ExcelValue) -> Result<(), ExcelError> {
    match v {
        ExcelValue::Array(rows) => {
            for row in rows {
                for c in row {
                    concat_feed_value(builder, c)?;
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
        ExcelValue::Array(_) => concat_feed_value(builder, v),
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

    /// UTF-16 units accepted so far (for tests / benches).
    pub fn utf16_len(&self) -> usize {
        self.utf16
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

    fn eval_all(wb: &Workbook, formula: &str) -> [ExcelValue; 3] {
        [
            eval_concat_formula(wb, formula, ConcatWalk::Auto).unwrap(),
            eval_concat_formula(wb, formula, ConcatWalk::Dense).unwrap(),
            eval_concat_formula(wb, formula, ConcatWalk::Materialize).unwrap(),
        ]
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
        assert_eq!(b.utf16_len(), 32767);
    }

    #[test]
    fn utf16_emoji_counts_two() {
        let mut b = ConcatBuilder::new();
        b.push(&"x".repeat(32766)).unwrap();
        assert!(b.push("😀").is_err());
    }

    #[test]
    fn precomposed_accent_is_one_unit() {
        let mut b = ConcatBuilder::new();
        b.push(&"x".repeat(32766)).unwrap();
        b.push("é").unwrap();
        assert_eq!(b.utf16_len(), 32767);
    }

    #[test]
    fn formula_matches_builder() {
        let wb = wb_col_a(&["a", "", "b"]);
        for v in eval_all(&wb, "=CONCAT(A1:A3)") {
            assert_eq!(v, ExcelValue::Text("ab".into()));
        }
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
        for v in eval_all(&wb, "=CONCAT(A1:B2)") {
            assert_eq!(v, ExcelValue::Text("abcd".into()));
        }
        for v in eval_all(&wb, "=CONCAT(A1:A2,B1:B2)") {
            assert_eq!(v, ExcelValue::Text("acbd".into()));
        }
    }

    #[test]
    fn occupied_walk_is_row_major_not_a1_key_order() {
        // A1-string BTree order is A1, A2, A10 — not row-major (A1, A2, A10
        // happens to match a single column, so use two columns + a far row).
        // Keys sort A1, A10, A2, B1, B10, B2; row-major is A1 B1 A2 B2 A10 B10.
        let mut sheet = Sheet::new("Sheet1");
        for (col, row, t) in [
            (0, 0, "a"),
            (1, 0, "b"),
            (0, 1, "c"),
            (1, 1, "d"),
            (0, 9, "e"),
            (1, 9, "f"),
        ] {
            sheet.insert(
                CellAddr::new(col, row),
                Cell::value(ExcelValue::Text(t.into())),
            );
        }
        let wb = Workbook {
            sheets: vec![sheet],
            names: vec![],
        };
        // area = 20, occupied = 6 → Auto uses occupied gather.
        for v in eval_all(&wb, "=CONCAT(A1:B10)") {
            assert_eq!(v, ExcelValue::Text("abcdef".into()));
        }
    }

    #[test]
    fn sparse_far_cells_skip_gaps() {
        let mut sheet = Sheet::new("Sheet1");
        sheet.insert(
            CellAddr::new(0, 0),
            Cell::value(ExcelValue::Text("a".into())),
        );
        sheet.insert(
            CellAddr::new(0, 99),
            Cell::value(ExcelValue::Text("b".into())),
        );
        let wb = Workbook {
            sheets: vec![sheet],
            names: vec![],
        };
        for v in eval_all(&wb, "=CONCAT(A1:A100)") {
            assert_eq!(v, ExcelValue::Text("ab".into()));
        }
    }

    #[test]
    fn prefer_occupied_on_tall_sparse_column() {
        let range = RangeRef::parse("A1:A50000").unwrap();
        assert!(prefer_occupied(&range, 1, ConcatWalk::Auto));
        assert!(!prefer_occupied(&range, 1, ConcatWalk::Dense));
        let dense = RangeRef::parse("A1:A1").unwrap();
        assert!(!prefer_occupied(&dense, 1, ConcatWalk::Auto));
    }
}
