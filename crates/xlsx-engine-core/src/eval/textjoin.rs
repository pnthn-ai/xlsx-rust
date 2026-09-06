//! Excel `TEXTJOIN(delimiter, ignore_empty, text1, [text2], …)`.
//!
//! Semantics (Excel 2019 / Microsoft 365):
//! - `delimiter` may be a scalar or a flattened range/array that **cycles**
//!   between emitted pieces (`TEXTJOIN({",",";"}, TRUE, "a","b","c")` → `a,b;c`).
//!   Empty delimiter cells stay in the cycle as `""`.
//! - `ignore_empty` is a logical. TRUE skips blank cells and `""`; FALSE keeps
//!   them (consecutive delimiters). A space, tab, or other whitespace is not
//!   empty — `TEXTJOIN` is not `TRIM` / `CLEAN`.
//! - Text arguments stay in array context (no implicit intersection): a range
//!   is flattened row-major (left-to-right, then top-to-bottom). Argument
//!   order is preserved.
//! - Numbers / bools coerce like `&` (`TRUE` → `"TRUE"`, `12` → `"12"`).
//!   Unformatted date serials join as their number text.
//! - First error wins, left-to-right (delimiter, then ignore_empty, then texts,
//!   including the first error inside a flattened range).
//! - Fewer than three arguments is `#VALUE!`.
//!
//! ## Length cap (honest)
//!
//! The result cannot exceed **32,767 UTF-16 code units** — Excel’s stored
//! cell-content width, the same cap as `REPT` / `CONCAT`. Microsoft’s
//! TEXTJOIN page says “32767 characters”; that is this cell-width limit, **not**
//! this crate’s Compatibility Version 2 `LEN` (Unicode scalars).
//!
//! - `TEXTJOIN("", TRUE, REPT("a", 32767))` is allowed; one extra ASCII unit
//!   is `#VALUE!`.
//! - A supplementary-plane scalar (`😀`) is **two** UTF-16 units, so
//!   `TEXTJOIN("", TRUE, REPT("😀", 16383), "😀")` is `#VALUE!` even though
//!   Compat-v2 `LEN` of 16383 copies is 16383.
//! - Overflow is detected while streaming; the builder never holds more than
//!   the cap. Cap success cases in the corpus use `LEN(TEXTJOIN(…))` or a
//!   `#VALUE!` overflow — not 32k expected strings.
//!
//! Production path: when `ignore_empty` is TRUE and the rectangle is much
//! larger than the sheet store, gather occupied A1 keys, sort by `(row, col)`,
//! and skip missing cells (they would be ignored anyway). `ignore_empty` FALSE
//! always walks the rectangle — blanks emit delimiter slots. Stream-append
//! with ASCII-fast UTF-16 length. The materializing `eval_expr` → 2-D `Array`
//! path is kept as the Instant-bench “before”.
//!
//! `CONCAT` / `CONCATENATE` are out of scope.

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{CellAddr, CellRef, EvalError, ExcelError, ExcelValue, RangeRef};

/// Excel cell-content limit used by TEXTJOIN (UTF-16 code units).
pub const TEXTJOIN_MAX_CHARS: usize = 32767;

/// Which range walk `eval_textjoin_formula` / the bench should take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextJoinWalk {
    /// `eval_expr` → 2-D `Array` → flatten (bench “before”).
    Materialize,
    /// Visit every address in the rectangle (HashMap / BTree lookup).
    Dense,
    /// Production: dense walk, or occupied-cell gather on large sparse ranges
    /// when `ignore_empty` is TRUE.
    Auto,
}

/// Production TEXTJOIN (range walk, stream append, no 2-D materialize).
pub(crate) fn fn_textjoin(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    textjoin_eval(ev, args, ctx, TextJoinWalk::Auto)
}

fn textjoin_eval(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
    walk: TextJoinWalk,
) -> Result<ExcelValue, EvalError> {
    if args.len() < 3 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }

    let delim_v = ev.eval_expr(&args[0], ctx)?;
    if let ExcelValue::Error(e) = delim_v {
        return Ok(ExcelValue::Error(e));
    }
    let delims = match textjoin_collect_delims(&delim_v) {
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
        let r = if walk == TextJoinWalk::Materialize {
            textjoin_feed_value(&mut builder, &ev.eval_expr(arg, ctx)?, ignore_empty)
        } else {
            feed_arg(ev, ctx, arg, &mut builder, ignore_empty, walk)
        };
        if let Err(e) = r {
            return Ok(ExcelValue::Error(e));
        }
    }
    Ok(ExcelValue::Text(builder.finish()))
}

/// Evaluate `formula` against `workbook` using a TEXTJOIN walk
/// (used by the hill-climb bench).
pub fn eval_textjoin_formula(
    workbook: &xlsx_types::Workbook,
    formula: &str,
    walk: TextJoinWalk,
) -> Result<ExcelValue, EvalError> {
    use xlsx_types::{EvalSpec, EvalTarget};
    let spec = EvalSpec {
        case_id: "textjoin-bench".into(),
        workbook: workbook.clone(),
        target: EvalTarget::formula(formula),
        options: Default::default(),
    };
    if walk == TextJoinWalk::Auto {
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
            textjoin_eval(&Evaluator::new(), &args, &mut ctx, walk)
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
    walk: TextJoinWalk,
) -> Result<(), ExcelError> {
    match arg {
        Expr::Range(r) => feed_range(ev, ctx, r, builder, ignore_empty, walk),
        Expr::Cell(c) => {
            let v = ev.eval_cell(c, ctx).map_err(|_| ExcelError::Value)?;
            feed_scalar(builder, &v, ignore_empty)
        }
        Expr::Name(n) => match named_as_range(n, ctx) {
            Ok(r) => feed_range(ev, ctx, &r, builder, ignore_empty, walk),
            Err(ExcelError::Name) => Err(ExcelError::Name),
            Err(_) => {
                let v = ev.eval_expr(arg, ctx).map_err(|_| ExcelError::Value)?;
                textjoin_feed_value(builder, &v, ignore_empty)
            }
        },
        other => {
            let v = ev.eval_expr(other, ctx).map_err(|_| ExcelError::Value)?;
            textjoin_feed_value(builder, &v, ignore_empty)
        }
    }
}

fn feed_range(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    range: &RangeRef,
    builder: &mut TextJoinBuilder,
    ignore_empty: bool,
    walk: TextJoinWalk,
) -> Result<(), ExcelError> {
    let sheet_name = range
        .sheet
        .clone()
        .unwrap_or_else(|| ctx.current_sheet.clone());
    let occupied = match ctx.spec.workbook.sheet(Some(&sheet_name)) {
        Ok(s) => s.cells.len() as u64,
        Err(_) => return Err(ExcelError::Ref),
    };
    if prefer_occupied(range, occupied, walk, ignore_empty) {
        feed_range_occupied(ev, ctx, &sheet_name, range, builder, ignore_empty)
    } else {
        feed_range_dense(ev, ctx, &sheet_name, range, builder, ignore_empty)
    }
}

/// Occupied gather is O(n_sheet + n_hits log n_hits). Dense walk is
/// O(area × lookup). Use occupied only when blanks are ignorable — they add
/// nothing — and the rectangle is much larger than the store.
/// `ignore_empty` FALSE must visit every address (blanks emit delimiters).
/// `Dense` forces the rectangle walk so the bench can show the gather win.
fn prefer_occupied(
    range: &RangeRef,
    occupied: u64,
    walk: TextJoinWalk,
    ignore_empty: bool,
) -> bool {
    if walk == TextJoinWalk::Dense || !ignore_empty {
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
    builder: &mut TextJoinBuilder,
    ignore_empty: bool,
) -> Result<(), ExcelError> {
    let mut a1 = String::with_capacity(8);
    let area = (range.row_count() as usize).saturating_mul(range.col_count() as usize);
    builder.reserve(area.saturating_mul(2).min(TEXTJOIN_MAX_CHARS));
    for addr in range.cells() {
        feed_cell(ev, ctx, sheet_name, addr, &mut a1, builder, ignore_empty)?;
    }
    Ok(())
}

fn feed_range_occupied(
    ev: &Evaluator,
    ctx: &mut Ctx<'_>,
    sheet_name: &str,
    range: &RangeRef,
    builder: &mut TextJoinBuilder,
    ignore_empty: bool,
) -> Result<(), ExcelError> {
    // Collect addresses only — do not hold `&Sheet` across `eval_cell`.
    let hits = gather_occupied(ctx, sheet_name, range)?;
    builder.reserve(hits.len().saturating_mul(4).min(TEXTJOIN_MAX_CHARS));
    let mut a1 = String::with_capacity(8);
    for addr in hits {
        feed_cell(ev, ctx, sheet_name, addr, &mut a1, builder, ignore_empty)?;
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
    builder: &mut TextJoinBuilder,
    ignore_empty: bool,
) -> Result<(), ExcelError> {
    match peek_stored(ctx, sheet_name, addr, a1_buf, builder, ignore_empty)? {
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
            feed_scalar(builder, &v, ignore_empty)
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
    builder: &mut TextJoinBuilder,
    ignore_empty: bool,
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
            feed_stored_value(builder, c.value.as_ref(), ignore_empty)?;
            Ok(Peek::Done)
        }
        None => {
            builder.push("", ignore_empty)?;
            Ok(Peek::Done)
        }
    }
}

fn feed_stored_value(
    builder: &mut TextJoinBuilder,
    value: Option<&ExcelValue>,
    ignore_empty: bool,
) -> Result<(), ExcelError> {
    match value {
        None | Some(ExcelValue::Empty) => builder.push("", ignore_empty),
        Some(ExcelValue::Text(s)) => builder.push(s, ignore_empty),
        Some(ExcelValue::Bool(true)) => builder.push("TRUE", ignore_empty),
        Some(ExcelValue::Bool(false)) => builder.push("FALSE", ignore_empty),
        Some(ExcelValue::Number(n)) => {
            let s = coerce::format_plain(*n);
            builder.push(&s, ignore_empty)
        }
        Some(ExcelValue::Error(e)) => Err(*e),
        Some(ExcelValue::Array(rows)) => {
            for row in rows {
                for c in row {
                    textjoin_feed_value(builder, c, ignore_empty)?;
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
                n = n.saturating_add(area.min(TEXTJOIN_MAX_CHARS));
            }
            Expr::Cell(_) | Expr::Text(_) => n = n.saturating_add(8),
            _ => {}
        }
    }
    (n > 0).then_some(n.min(TEXTJOIN_MAX_CHARS))
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

/// Flatten a delimiter value into cycling pieces. Shared with `seed-compliant`.
pub fn textjoin_collect_delims(v: &ExcelValue) -> Result<Vec<String>, ExcelError> {
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

/// Flatten an already-evaluated value into `builder` (arrays row-major).
/// Shared with `seed-compliant` so the UTF-16 cap / ignore_empty stay one impl.
pub fn textjoin_feed_value(
    builder: &mut TextJoinBuilder,
    v: &ExcelValue,
    ignore_empty: bool,
) -> Result<(), ExcelError> {
    match v {
        ExcelValue::Array(rows) => {
            for row in rows {
                for c in row {
                    textjoin_feed_value(builder, c, ignore_empty)?;
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
        ExcelValue::Array(_) => textjoin_feed_value(builder, v, ignore_empty),
    }
}

/// Streaming join with cycling delimiters and the 32,767 UTF-16-code-unit cap.
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

    fn eval_all(wb: &Workbook, formula: &str) -> [ExcelValue; 3] {
        [
            eval_textjoin_formula(wb, formula, TextJoinWalk::Auto).unwrap(),
            eval_textjoin_formula(wb, formula, TextJoinWalk::Dense).unwrap(),
            eval_textjoin_formula(wb, formula, TextJoinWalk::Materialize).unwrap(),
        ]
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
        assert_eq!(b.utf16_len(), 32767);
    }

    #[test]
    fn utf16_emoji_counts_two() {
        let mut b = TextJoinBuilder::new(vec![String::new()]);
        b.push(&"x".repeat(32766), true).unwrap();
        assert!(b.push("😀", true).is_err());
    }

    #[test]
    fn precomposed_accent_is_one_unit() {
        let mut b = TextJoinBuilder::new(vec![String::new()]);
        b.push(&"x".repeat(32766), true).unwrap();
        b.push("é", true).unwrap();
        assert_eq!(b.utf16_len(), 32767);
    }

    #[test]
    fn formula_matches_builder() {
        let wb = wb_col_a(&["a", "", "b"]);
        for v in eval_all(&wb, "=TEXTJOIN(\",\",TRUE,A1:A3)") {
            assert_eq!(v, ExcelValue::Text("a,b".into()));
        }
        for v in eval_all(&wb, "=TEXTJOIN(\",\",FALSE,A1:A3)") {
            assert_eq!(v, ExcelValue::Text("a,,b".into()));
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
        for v in eval_all(&wb, "=TEXTJOIN(\",\",TRUE,A1:B2)") {
            assert_eq!(v, ExcelValue::Text("a,b,c,d".into()));
        }
        for v in eval_all(&wb, "=TEXTJOIN(\",\",TRUE,A1:A2,B1:B2)") {
            assert_eq!(v, ExcelValue::Text("a,c,b,d".into()));
        }
    }

    #[test]
    fn occupied_walk_is_row_major_not_a1_key_order() {
        // A1-string BTree order is A1, A10, A2, B1, B10, B2; row-major is
        // A1 B1 A2 B2 A10 B10.
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
        // area = 20, occupied = 6 → Auto uses occupied gather (ignore TRUE).
        for v in eval_all(&wb, "=TEXTJOIN(\"\",TRUE,A1:B10)") {
            assert_eq!(v, ExcelValue::Text("abcdef".into()));
        }
    }

    #[test]
    fn sparse_far_cells_skip_gaps_when_ignore() {
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
        for v in eval_all(&wb, "=TEXTJOIN(\",\",TRUE,A1:A100)") {
            assert_eq!(v, ExcelValue::Text("a,b".into()));
        }
        // FALSE must keep the 98 blank slots (99 commas).
        for v in eval_all(&wb, "=TEXTJOIN(\",\",FALSE,A1:A100)") {
            let expected = format!("a{}b", ",".repeat(99));
            assert_eq!(v, ExcelValue::Text(expected));
        }
    }

    #[test]
    fn prefer_occupied_only_when_ignore_empty() {
        let range = RangeRef::parse("A1:A50000").unwrap();
        assert!(prefer_occupied(&range, 1, TextJoinWalk::Auto, true));
        assert!(!prefer_occupied(&range, 1, TextJoinWalk::Auto, false));
        assert!(!prefer_occupied(&range, 1, TextJoinWalk::Dense, true));
        let dense = RangeRef::parse("A1:A1").unwrap();
        assert!(!prefer_occupied(&dense, 1, TextJoinWalk::Auto, true));
    }

    #[test]
    fn collect_delims_keeps_blank_cycle_slots() {
        let v = ExcelValue::Array(vec![vec![
            ExcelValue::Text(",".into()),
            ExcelValue::Empty,
            ExcelValue::Text(";".into()),
        ]]);
        assert_eq!(
            textjoin_collect_delims(&v).unwrap(),
            vec![",".to_string(), String::new(), ";".to_string()]
        );
    }
}
