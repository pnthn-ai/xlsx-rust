//! Excel `TEXTSPLIT(text, col_delimiter, [row_delimiter], [ignore_empty], [match_mode], [pad_with])`.
//!
//! Inverse of `TEXTJOIN`. The result is always an [`ExcelValue::Array`]
//! (including 1×1). Pieces stay **text** — `"1,2"` splits to `"1"`, `"2"`,
//! not numbers.
//!
//! ## Semantics
//!
//! - `col_delimiter` omitted → no column split (row-only, a single column).
//! - `row_delimiter` omitted → no row split (column-only, a single row).
//! - Both omitted, or neither supplied → `#VALUE!`.
//! - An empty-string delimiter (literal `""` or a blank cell coerced to `""`)
//!   is `#VALUE!`. That is **not** the same as an omitted delimiter.
//! - Delimiters may be scalars or flattened arrays (`{",",";"}`). Earliest
//!   match wins; a longer delimiter wins a tie at the same byte index.
//! - `ignore_empty` (default FALSE): skip empty tokens created by consecutive
//!   / leading / trailing delimiters. A space is not empty. If no delimiter
//!   is found, the original text is returned even when it is `""`.
//! - After `ignore_empty`, no remaining row → `#CALC!` (Excel cannot return
//!   an empty array).
//! - `match_mode` 0 (default) is case-sensitive; 1 is ASCII case-insensitive
//!   (same casefold as `SEARCH` / `UNIQUE` in this crate). Other values after
//!   toward-zero truncate are `#VALUE!`.
//! - `pad_with` (default `#N/A`) fills short rows when **both** axes split and
//!   row widths differ. 1-D results are never padded.
//! - Numbers / bools coerce like `&` (`TRUE` → `"TRUE"`, `12` → `"12"`).
//!
//! ## Spill / pad / model limits
//!
//! - The engine returns an array **value**. The snippet workbook has no spill
//!   grid, so a blocked cell below/right of the host never yields `#SPILL!`.
//! - Scalar operators (`TEXTSPLIT(...)+1`) take the top-left element via
//!   `scalarize`. Consume with `INDEX` / `COUNTA` / `TYPE`.
//! - `IFNA` / `IFERROR` wrap a **scalar** error. They do **not** rewrite pad
//!   `#N/A` cells inside the array (Excel's dynamic-array `IFNA` does).
//! - `text` is evaluated as a scalar (implicit intersection / top-left).
//!   `TEXTSPLIT` of a range of strings (Excel's "array of arrays") is not
//!   modeled.
//! - Excel's ~1,048,576-row array cap is not enforced; size is memory-bounded.
//! - Matching uses Unicode scalars (UTF-8), like `LEN` / `FIND` here — not
//!   Excel UTF-16 code units. BMP text matches Excel.
//!
//! [`textsplit`] scans with `str::find` / a single-byte ASCII fast path.
//! [`textsplit_naive`] walks `Vec<char>` and tries every index — same
//! answers, more allocation. Used as the bench "before".

use super::{coerce, Ctx, Evaluator};
use crate::ast::Expr;
use xlsx_types::{EvalError, ExcelError, ExcelValue};

const DEFAULT_PAD: ExcelValue = ExcelValue::Error(ExcelError::Na);

#[derive(Clone, Copy)]
enum SplitMode {
    Fast,
    Naive,
}

/// Production TEXTSPLIT from already-normalized arguments.
pub fn textsplit(
    text: &str,
    col_delims: &[String],
    row_delims: &[String],
    ignore_empty: bool,
    case_insensitive: bool,
    pad_with: &ExcelValue,
) -> ExcelValue {
    split_grid(
        text,
        col_delims,
        row_delims,
        ignore_empty,
        case_insensitive,
        pad_with,
        SplitMode::Fast,
    )
}

/// `Vec<char>` / try-every-index baseline. Same Excel answers as [`textsplit`].
pub fn textsplit_naive(
    text: &str,
    col_delims: &[String],
    row_delims: &[String],
    ignore_empty: bool,
    case_insensitive: bool,
    pad_with: &ExcelValue,
) -> ExcelValue {
    split_grid(
        text,
        col_delims,
        row_delims,
        ignore_empty,
        case_insensitive,
        pad_with,
        SplitMode::Naive,
    )
}

pub(crate) fn fn_textsplit(
    ev: &Evaluator,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
) -> Result<ExcelValue, EvalError> {
    if args.is_empty() || args.len() > 6 {
        return Ok(ExcelValue::Error(ExcelError::Value));
    }

    let text_v = ev.eval_scalar(&args[0], ctx)?;
    let col = if args.len() >= 2 && !args[1].is_omitted() {
        Some(ev.eval_expr(&args[1], ctx)?)
    } else {
        None
    };
    let row = if args.len() >= 3 && !args[2].is_omitted() {
        Some(ev.eval_expr(&args[2], ctx)?)
    } else {
        None
    };
    let ignore = if args.len() >= 4 && !args[3].is_omitted() {
        Some(ev.eval_scalar(&args[3], ctx)?)
    } else {
        None
    };
    let mode = if args.len() >= 5 && !args[4].is_omitted() {
        Some(ev.eval_scalar(&args[4], ctx)?)
    } else {
        None
    };
    let pad = if args.len() >= 6 && !args[5].is_omitted() {
        Some(ev.eval_expr(&args[5], ctx)?)
    } else {
        None
    };

    Ok(apply_values(
        &text_v,
        col.as_ref(),
        row.as_ref(),
        ignore.as_ref(),
        mode.as_ref(),
        pad.as_ref(),
    ))
}

/// TEXTSPLIT from already-evaluated arguments. `None` means omitted.
pub fn apply_values(
    text: &ExcelValue,
    col_delim: Option<&ExcelValue>,
    row_delim: Option<&ExcelValue>,
    ignore_empty: Option<&ExcelValue>,
    match_mode: Option<&ExcelValue>,
    pad_with: Option<&ExcelValue>,
) -> ExcelValue {
    if let ExcelValue::Error(e) = text {
        return ExcelValue::Error(*e);
    }
    let text = match coerce::to_text(text) {
        Ok(s) => s,
        Err(e) => return ExcelValue::Error(e),
    };

    let col_delims = match col_delim {
        None => Vec::new(),
        Some(v) => match collect_delims(v) {
            Ok(d) => d,
            Err(e) => return ExcelValue::Error(e),
        },
    };
    let row_delims = match row_delim {
        None => Vec::new(),
        Some(v) => match collect_delims(v) {
            Ok(d) => d,
            Err(e) => return ExcelValue::Error(e),
        },
    };

    let ignore_empty = match ignore_empty {
        None => false,
        Some(v) => match coerce::to_logical(v) {
            Ok(b) => b,
            Err(e) => return ExcelValue::Error(e),
        },
    };
    let case_insensitive = match match_mode {
        None => false,
        Some(v) => match match_mode_flag(v) {
            Ok(b) => b,
            Err(e) => return ExcelValue::Error(e),
        },
    };
    let pad_with = match pad_with {
        None => DEFAULT_PAD,
        Some(ExcelValue::Array(rows)) => rows
            .first()
            .and_then(|r| r.first())
            .cloned()
            .unwrap_or(ExcelValue::Empty),
        Some(other) => other.clone(),
    };

    textsplit(
        &text,
        &col_delims,
        &row_delims,
        ignore_empty,
        case_insensitive,
        &pad_with,
    )
}

fn match_mode_flag(v: &ExcelValue) -> Result<bool, ExcelError> {
    let n = coerce::to_number(v)?;
    if !n.is_finite() {
        return Err(ExcelError::Value);
    }
    let t = n.trunc();
    match t as i64 {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(ExcelError::Value),
    }
}

fn collect_delims(v: &ExcelValue) -> Result<Vec<String>, ExcelError> {
    let mut out = Vec::new();
    flatten_delims(v, &mut out)?;
    Ok(out)
}

fn flatten_delims(v: &ExcelValue, out: &mut Vec<String>) -> Result<(), ExcelError> {
    match v {
        ExcelValue::Array(rows) => {
            for row in rows {
                for c in row {
                    flatten_delims(c, out)?;
                }
            }
            Ok(())
        }
        ExcelValue::Error(e) => Err(*e),
        other => {
            out.push(coerce::to_text(other)?);
            Ok(())
        }
    }
}

fn split_grid(
    text: &str,
    col_delims: &[String],
    row_delims: &[String],
    ignore_empty: bool,
    case_insensitive: bool,
    pad_with: &ExcelValue,
    mode: SplitMode,
) -> ExcelValue {
    let col = match normalize_delims(col_delims) {
        Ok(d) => d,
        Err(e) => return ExcelValue::Error(e),
    };
    let row = match normalize_delims(row_delims) {
        Ok(d) => d,
        Err(e) => return ExcelValue::Error(e),
    };
    if col.is_empty() && row.is_empty() {
        return ExcelValue::Error(ExcelError::Value);
    }

    let line_texts = if row.is_empty() {
        vec![text.to_string()]
    } else {
        split_parts(text, &row, ignore_empty, case_insensitive, mode)
    };

    let mut grid: Vec<Vec<ExcelValue>> = Vec::with_capacity(line_texts.len());
    let mut max_cols = 0usize;
    for line in line_texts {
        let cells = if col.is_empty() {
            vec![line]
        } else {
            split_parts(&line, &col, ignore_empty, case_insensitive, mode)
        };
        if cells.is_empty() {
            continue;
        }
        max_cols = max_cols.max(cells.len());
        grid.push(cells.into_iter().map(ExcelValue::Text).collect());
    }
    if grid.is_empty() {
        return ExcelValue::Error(ExcelError::Calc);
    }

    let both_axes = !col.is_empty() && !row.is_empty();
    if both_axes {
        for r in &mut grid {
            while r.len() < max_cols {
                r.push(pad_with.clone());
            }
        }
    }
    ExcelValue::Array(grid)
}

/// Empty strings in a delimiter array are skipped. An axis whose only
/// delimiter is `""` is `#VALUE!`. An omitted axis is an empty list (ok).
fn normalize_delims(delims: &[String]) -> Result<Vec<String>, ExcelError> {
    if delims.is_empty() {
        return Ok(Vec::new());
    }
    let kept: Vec<String> = delims.iter().filter(|d| !d.is_empty()).cloned().collect();
    if kept.is_empty() {
        Err(ExcelError::Value)
    } else {
        Ok(kept)
    }
}

fn split_parts(
    text: &str,
    delims: &[String],
    ignore_empty: bool,
    case_insensitive: bool,
    mode: SplitMode,
) -> Vec<String> {
    match mode {
        SplitMode::Fast => split_parts_fast(text, delims, ignore_empty, case_insensitive),
        SplitMode::Naive => split_parts_naive(text, delims, ignore_empty, case_insensitive),
    }
}

fn split_parts_fast(
    text: &str,
    delims: &[String],
    ignore_empty: bool,
    case_insensitive: bool,
) -> Vec<String> {
    if !case_insensitive && delims.len() == 1 {
        let d = &delims[0];
        if d.len() == 1 {
            let b = d.as_bytes()[0];
            if b.is_ascii() {
                return split_ascii_byte(text, b, ignore_empty);
            }
        }
        return split_str_find(text, d, ignore_empty);
    }
    let ascii_ci = case_insensitive
        && text.is_ascii()
        && delims.iter().all(|d| d.is_ascii());
    if ascii_ci && delims.len() == 1 {
        return split_ascii_ci_one(text, &delims[0], ignore_empty);
    }
    split_multi(text, delims, ignore_empty, case_insensitive, ascii_ci)
}

/// Case-insensitive split on one ASCII delimiter. `is_ascii` is checked once
/// by the caller — do not re-walk the haystack on every hit.
fn split_ascii_ci_one(text: &str, delim: &str, ignore_empty: bool) -> Vec<String> {
    if delim.is_empty() {
        return vec![text.to_string()];
    }
    let hay = text.as_bytes();
    let needle = delim.as_bytes();
    if needle.len() > hay.len() {
        return vec![text.to_string()];
    }
    let mut out = Vec::new();
    let mut start = 0usize;
    let dlen = needle.len();
    let mut i = 0usize;
    let mut found = false;
    while i + dlen <= hay.len() {
        if bytes_eq_ci(&hay[i..i + dlen], needle) {
            found = true;
            if !(ignore_empty && i == start) {
                out.push(text[start..i].to_string());
            }
            start = i + dlen;
            i = start;
        } else {
            i += 1;
        }
    }
    if !found {
        return vec![text.to_string()];
    }
    if !(ignore_empty && start == hay.len()) {
        out.push(text[start..].to_string());
    }
    out
}

fn find_bytes_ci(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.len() > hay.len() {
        return None;
    }
    let last = hay.len() - needle.len();
    for i in 0..=last {
        if bytes_eq_ci(&hay[i..i + needle.len()], needle) {
            return Some(i);
        }
    }
    None
}

fn bytes_eq_ci(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.eq_ignore_ascii_case(y))
}

fn split_ascii_byte(text: &str, delim: u8, ignore_empty: bool) -> Vec<String> {
    let bytes = text.as_bytes();
    if !bytes.contains(&delim) {
        return vec![text.to_string()];
    }
    let mut out = Vec::new();
    let mut start = 0usize;
    for i in 0..bytes.len() {
        if bytes[i] == delim {
            if !(ignore_empty && i == start) {
                // ASCII delimiter cannot sit inside a multibyte UTF-8 scalar.
                out.push(text[start..i].to_string());
            }
            start = i + 1;
        }
    }
    if !(ignore_empty && start == bytes.len()) {
        out.push(text[start..].to_string());
    }
    out
}

fn split_str_find(text: &str, delim: &str, ignore_empty: bool) -> Vec<String> {
    if delim.is_empty() || text.find(delim).is_none() {
        return vec![text.to_string()];
    }
    let mut out = Vec::new();
    let mut start = 0usize;
    let dlen = delim.len();
    while let Some(rel) = text[start..].find(delim) {
        let end = start + rel;
        if !(ignore_empty && end == start) {
            out.push(text[start..end].to_string());
        }
        start = end + dlen;
    }
    if !(ignore_empty && start == text.len()) {
        out.push(text[start..].to_string());
    }
    out
}

fn split_multi(
    text: &str,
    delims: &[String],
    ignore_empty: bool,
    case_insensitive: bool,
    ascii_ci: bool,
) -> Vec<String> {
    let first = next_match(text, delims, case_insensitive, ascii_ci);
    if first.is_none() {
        return vec![text.to_string()];
    }
    let mut out = Vec::new();
    let mut start = 0usize;
    while let Some((rel, dlen)) = next_match(&text[start..], delims, case_insensitive, ascii_ci) {
        let end = start + rel;
        if !(ignore_empty && end == start) {
            out.push(text[start..end].to_string());
        }
        start = end + dlen;
        if dlen == 0 {
            break;
        }
    }
    if !(ignore_empty && start == text.len()) {
        out.push(text[start..].to_string());
    }
    out
}

/// Earliest byte index, then longest delimiter, then first listed.
fn next_match(
    hay: &str,
    delims: &[String],
    case_insensitive: bool,
    ascii_ci: bool,
) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    for d in delims {
        if d.is_empty() {
            continue;
        }
        let Some(i) = find_delim(hay, d, case_insensitive, ascii_ci) else {
            continue;
        };
        let len = d.len();
        match best {
            None => best = Some((i, len)),
            Some((bi, bl)) => {
                if i < bi || (i == bi && len > bl) {
                    best = Some((i, len));
                }
            }
        }
    }
    best
}

fn find_delim(hay: &str, needle: &str, case_insensitive: bool, ascii_ci: bool) -> Option<usize> {
    if !case_insensitive {
        return hay.find(needle);
    }
    if ascii_ci {
        return find_bytes_ci(hay.as_bytes(), needle.as_bytes());
    }
    find_chars_ci(hay, needle)
}

fn find_chars_ci(hay: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    let nchars: Vec<char> = needle.chars().collect();
    for (byte_idx, _) in hay.char_indices() {
        let mut it = hay[byte_idx..].chars();
        let mut ok = true;
        for &need in &nchars {
            match it.next() {
                Some(got) if got.eq_ignore_ascii_case(&need) => {}
                _ => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            return Some(byte_idx);
        }
    }
    None
}

fn split_parts_naive(
    text: &str,
    delims: &[String],
    ignore_empty: bool,
    case_insensitive: bool,
) -> Vec<String> {
    let hay: Vec<char> = text.chars().collect();
    let needles: Vec<Vec<char>> = delims
        .iter()
        .filter(|d| !d.is_empty())
        .map(|d| d.chars().collect())
        .collect();
    if needles.is_empty() {
        return vec![text.to_string()];
    }

    let mut hits: Vec<(usize, usize)> = Vec::new();
    let mut i = 0usize;
    while i < hay.len() {
        let mut best: Option<(usize, usize)> = None;
        for n in &needles {
            if n.is_empty() || i + n.len() > hay.len() {
                continue;
            }
            let matched = if case_insensitive {
                hay[i..i + n.len()]
                    .iter()
                    .zip(n.iter())
                    .all(|(a, b)| a.eq_ignore_ascii_case(b))
            } else {
                hay[i..i + n.len()] == n[..]
            };
            if matched {
                let nlen = n.len();
                match best {
                    None => best = Some((i, nlen)),
                    Some((_, bl)) if nlen > bl => best = Some((i, nlen)),
                    _ => {}
                }
            }
        }
        if let Some((at, nlen)) = best {
            hits.push((at, nlen));
            i = at + nlen;
        } else {
            i += 1;
        }
    }
    if hits.is_empty() {
        return vec![text.to_string()];
    }

    let mut out = Vec::new();
    let mut start = 0usize;
    for (at, nlen) in hits {
        if !(ignore_empty && at == start) {
            out.push(hay[start..at].iter().collect());
        }
        start = at + nlen;
    }
    if !(ignore_empty && start == hay.len()) {
        out.push(hay[start..].iter().collect());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> ExcelValue {
        ExcelValue::Text(s.into())
    }

    fn row(parts: &[&str]) -> Vec<ExcelValue> {
        parts.iter().map(|s| t(s)).collect()
    }

    fn arr_row(parts: &[&str]) -> ExcelValue {
        ExcelValue::Array(vec![row(parts)])
    }

    fn arr_col(parts: &[&str]) -> ExcelValue {
        ExcelValue::Array(parts.iter().map(|s| vec![t(s)]).collect())
    }

    fn split(
        text: &str,
        col: &[&str],
        row: &[&str],
        ignore: bool,
        ci: bool,
        pad: &ExcelValue,
    ) -> ExcelValue {
        let cols: Vec<String> = col.iter().map(|s| (*s).to_string()).collect();
        let rows: Vec<String> = row.iter().map(|s| (*s).to_string()).collect();
        let a = textsplit(text, &cols, &rows, ignore, ci, pad);
        let b = textsplit_naive(text, &cols, &rows, ignore, ci, pad);
        assert_eq!(a, b, "fast vs naive mismatch on {text:?}");
        a
    }

    #[test]
    fn cols_basic() {
        assert_eq!(
            split("a,b,c", &[","], &[], false, false, &DEFAULT_PAD),
            arr_row(&["a", "b", "c"])
        );
    }

    #[test]
    fn rows_basic() {
        assert_eq!(
            split("a,b,c", &[], &[","], false, false, &DEFAULT_PAD),
            arr_col(&["a", "b", "c"])
        );
    }

    #[test]
    fn both_axes_pad_na() {
        assert_eq!(
            split("a,b;c", &[","], &[";"], false, false, &DEFAULT_PAD),
            ExcelValue::Array(vec![row(&["a", "b"]), vec![t("c"), DEFAULT_PAD]])
        );
    }

    #[test]
    fn pad_with_empty_string() {
        assert_eq!(
            split("a,b;c", &[","], &[";"], false, false, &t("")),
            ExcelValue::Array(vec![row(&["a", "b"]), row(&["c", ""])])
        );
    }

    #[test]
    fn ignore_empty_drops_consecutive() {
        assert_eq!(
            split("a,,b", &[","], &[], true, false, &DEFAULT_PAD),
            arr_row(&["a", "b"])
        );
        assert_eq!(
            split("a,,b", &[","], &[], false, false, &DEFAULT_PAD),
            arr_row(&["a", "", "b"])
        );
    }

    #[test]
    fn no_delim_returns_original() {
        assert_eq!(
            split("apple orange", &["."], &[], false, false, &DEFAULT_PAD),
            arr_row(&["apple orange"])
        );
        assert_eq!(
            split("", &[","], &[], true, false, &DEFAULT_PAD),
            arr_row(&[""])
        );
    }

    #[test]
    fn all_empty_after_ignore_is_calc() {
        assert_eq!(
            split(",", &[","], &[], true, false, &DEFAULT_PAD),
            ExcelValue::Error(ExcelError::Calc)
        );
    }

    #[test]
    fn match_mode_ascii_ci() {
        assert_eq!(
            split("axbXc", &["X"], &[], false, true, &DEFAULT_PAD),
            arr_row(&["a", "b", "c"])
        );
        assert_eq!(
            split("axbXc", &["X"], &[], false, false, &DEFAULT_PAD),
            arr_row(&["axb", "c"])
        );
    }

    #[test]
    fn multi_delim_earliest() {
        assert_eq!(
            split("a,b;c", &[",", ";"], &[], false, false, &DEFAULT_PAD),
            arr_row(&["a", "b", "c"])
        );
    }

    #[test]
    fn empty_delim_is_value() {
        assert_eq!(
            split("abc", &[""], &[], false, false, &DEFAULT_PAD),
            ExcelValue::Error(ExcelError::Value)
        );
        assert_eq!(
            split("abc", &[], &[], false, false, &DEFAULT_PAD),
            ExcelValue::Error(ExcelError::Value)
        );
    }

    #[test]
    fn longest_delim_wins_tie() {
        assert_eq!(
            split("axxb", &["x", "xx"], &[], false, false, &DEFAULT_PAD),
            arr_row(&["a", "b"])
        );
    }

    #[test]
    fn unicode_scalar_delim() {
        assert_eq!(
            split("α,β,γ", &[","], &[], false, false, &DEFAULT_PAD),
            arr_row(&["α", "β", "γ"])
        );
    }

    #[test]
    fn both_axes_ignore_empty_row() {
        assert_eq!(
            split("a,b;;c,d", &[","], &[";"], true, false, &DEFAULT_PAD),
            ExcelValue::Array(vec![row(&["a", "b"]), row(&["c", "d"])])
        );
    }
}
